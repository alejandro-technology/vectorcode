//! RRF fusion and HybridSearcher — combines dense + sparse results.
//!
//! Provides `rrf_fuse` pure function for Reciprocal Rank Fusion and
//! `HybridSearcher` which runs dense and sparse in parallel via `tokio::join!`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

use crate::reranker::{RerankDocument, Reranker};
use crate::types::SearchResult;

use super::{SearchMode, SearchOptions, SearchStrategy};

/// Reciprocal Rank Fusion — combines multiple ranked result lists.
///
/// K parameter controls the weight of high-ranked items (default 60).
/// Deduplicates by composite key (file_path, start_line, end_line).
/// Ranks are 1-indexed (first result = rank 1).
///
/// For each result set, for each result at 1-indexed position `rank`:
///   score += 1.0 / (k as f64 + rank as f64)
///
/// Results are sorted by descending RRF score and truncated to `limit`.
pub fn rrf_fuse(result_sets: &[Vec<SearchResult>], k: u32, limit: usize) -> Vec<SearchResult> {
    // Accumulate RRF scores per composite key
    let mut scores: HashMap<(String, u32, u32), f64> = HashMap::new();
    // Keep the first occurrence of each result (for metadata)
    let mut result_map: HashMap<(String, u32, u32), SearchResult> = HashMap::new();

    for results in result_sets {
        for (idx, result) in results.iter().enumerate() {
            let rank = (idx + 1) as f64; // 1-indexed
            let key = (result.file_path.clone(), result.start_line, result.end_line);

            *scores.entry(key.clone()).or_insert(0.0) += 1.0 / (k as f64 + rank);

            // Keep the first occurrence (highest-ranked from first list)
            result_map.entry(key).or_insert_with(|| result.clone());
        }
    }

    // Build fused results with RRF scores
    let mut fused: Vec<SearchResult> = scores
        .into_iter()
        .map(|(key, rrf_score)| {
            let mut result = result_map.remove(&key).unwrap();
            result.score = rrf_score as f32;
            result
        })
        .collect();

    // Sort by descending RRF score
    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Truncate to limit
    fused.truncate(limit);
    fused
}

/// Hybrid search engine combining dense + sparse via RRF fusion.
///
/// Runs both search strategies in parallel using `tokio::join!`.
/// Graceful degradation: if one searcher fails, returns results from the other.
/// If both fail, returns the first error.
///
/// When constructed with `with_reranker()`, the top-K fused candidates are
/// re-scored by a cross-encoder reranker. On timeout or error, the reranker
/// is skipped and RRF-ordered results are returned (graceful degradation).
pub struct HybridSearcher {
    dense: Arc<dyn SearchStrategy>,
    sparse: Arc<dyn SearchStrategy>,
    rrf_k: u32,
    reranker: Option<Arc<dyn Reranker>>,
    reranker_top_k: usize,
    reranker_timeout: Duration,
    mode: SearchMode,
}

impl HybridSearcher {
    /// Create a new HybridSearcher with dense and sparse strategies (no reranker).
    ///
    /// This is the backward-compatible constructor — `mode()` returns `SearchMode::Hybrid`.
    pub fn new(
        dense: Arc<dyn SearchStrategy>,
        sparse: Arc<dyn SearchStrategy>,
        rrf_k: u32,
    ) -> Self {
        Self {
            dense,
            sparse,
            rrf_k,
            reranker: None,
            reranker_top_k: 0,
            reranker_timeout: Duration::from_secs(5),
            mode: SearchMode::Hybrid,
        }
    }

    /// Create a HybridSearcher with an optional reranker for cross-encoder re-scoring.
    ///
    /// When `reranker` is `Some`, the `search()` method will re-score the top
    /// `reranker_top_k` fused candidates using the reranker within `reranker_timeout`.
    /// `mode()` returns `SearchMode::HybridRerank`.
    pub fn with_reranker(
        dense: Arc<dyn SearchStrategy>,
        sparse: Arc<dyn SearchStrategy>,
        rrf_k: u32,
        reranker: Option<Arc<dyn Reranker>>,
        reranker_top_k: usize,
        reranker_timeout: Duration,
    ) -> Self {
        Self {
            dense,
            sparse,
            rrf_k,
            reranker,
            reranker_top_k,
            reranker_timeout,
            mode: SearchMode::HybridRerank,
        }
    }
}

#[async_trait]
impl SearchStrategy for HybridSearcher {
    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<SearchResult>> {
        let limit = options.limit;
        let rrf_k = self.rrf_k;

        // Run dense and sparse in parallel
        let (dense_result, sparse_result) = tokio::join!(
            self.dense.search(query, options.clone()),
            self.sparse.search(query, options)
        );

        let fused = match (dense_result, sparse_result) {
            (Ok(dense_results), Ok(sparse_results)) => {
                // Both succeeded — fuse via RRF
                rrf_fuse(&[dense_results, sparse_results], rrf_k, limit)
            }
            (Ok(dense_results), Err(_)) => {
                // Sparse failed — return dense results (graceful degradation)
                dense_results.into_iter().take(limit).collect()
            }
            (Err(_), Ok(sparse_results)) => {
                // Dense failed — return sparse results (graceful degradation)
                sparse_results.into_iter().take(limit).collect()
            }
            (Err(e), Err(_)) => {
                // Both failed — return first error
                return Err(e);
            }
        };

        // If reranker is available and we have candidates, re-score top-K
        if let Some(reranker) = &self.reranker {
            if !fused.is_empty() && self.reranker_top_k > 0 {
                return Ok(self.apply_reranking(query, fused, reranker).await);
            }
        }

        Ok(fused)
    }

    fn mode(&self) -> SearchMode {
        self.mode
    }
}

impl HybridSearcher {
    /// Re-score the top-K fused results using the reranker.
    ///
    /// On timeout or error, logs a warning and returns the original RRF-ordered
    /// results (graceful degradation — never propagates reranker errors).
    async fn apply_reranking(
        &self,
        query: &str,
        fused: Vec<SearchResult>,
        reranker: &Arc<dyn Reranker>,
    ) -> Vec<SearchResult> {
        let top_k = self.reranker_top_k.min(fused.len());
        let (top_candidates, tail) = {
            let mut fused = fused;
            let tail = fused.split_off(top_k);
            (fused, tail)
        };

        // Build RerankDocuments from top candidates
        let docs: Vec<RerankDocument> = top_candidates
            .iter()
            .enumerate()
            .map(|(i, r)| RerankDocument {
                content: r.content.clone(),
                index: i,
            })
            .collect();

        // Call reranker with timeout
        let rerank_result =
            tokio::time::timeout(self.reranker_timeout, reranker.rerank(query, &docs)).await;

        match rerank_result {
            Ok(Ok(scores)) => {
                // scores: Vec<(original_index, relevance_score)> sorted by score DESC
                // Rebuild top_candidates ordered by reranker scores
                let mut reranked: Vec<SearchResult> = scores
                    .iter()
                    .filter_map(|(orig_idx, score)| {
                        top_candidates.get(*orig_idx).map(|r| SearchResult {
                            repo_name: None,
                            score: *score,
                            ..r.clone()
                        })
                    })
                    .collect();

                // Append the tail (unchanged)
                reranked.extend(tail);
                reranked
            }
            Ok(Err(e)) => {
                warn!("Reranker returned error, falling back to RRF order: {e}");
                // Reconstruct original order: top_candidates + tail
                let mut result = top_candidates;
                result.extend(tail);
                result
            }
            Err(_elapsed) => {
                warn!(
                    "Reranker timed out after {}ms, falling back to RRF order",
                    self.reranker_timeout.as_millis()
                );
                let mut result = top_candidates;
                result.extend(tail);
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::searcher::SearchMode;

    fn make_result(file_path: &str, start_line: u32, end_line: u32, score: f32) -> SearchResult {
        SearchResult {
            repo_name: None,
            file_path: file_path.to_string(),
            start_line,
            end_line,
            symbol: None,
            kind: "function_declaration".to_string(),
            content: format!("content of {file_path}:{start_line}-{end_line}"),
            parent_context: None,
            language: "typescript".to_string(),
            score,
        }
    }

    // ─── rrf_fuse tests ────────────────────────────────────────────────

    #[test]
    fn rrf_fuse_empty_inputs_returns_empty() {
        let result = rrf_fuse(&[], 60, 10);
        assert!(result.is_empty());
    }

    #[test]
    fn rrf_fuse_all_empty_lists_returns_empty() {
        let result = rrf_fuse(&[vec![], vec![]], 60, 10);
        assert!(result.is_empty());
    }

    #[test]
    fn rrf_fuse_single_list_passthrough() {
        let list = vec![
            make_result("a.ts", 1, 10, 0.9),
            make_result("b.ts", 1, 10, 0.8),
            make_result("c.ts", 1, 10, 0.7),
        ];

        let result = rrf_fuse(&[list], 60, 10);

        assert_eq!(result.len(), 3);
        // Single list: rank 1 gets highest RRF score
        assert_eq!(result[0].file_path, "a.ts");
        assert_eq!(result[1].file_path, "b.ts");
        assert_eq!(result[2].file_path, "c.ts");
        // Scores should be 1/(60+1), 1/(60+2), 1/(60+3)
        assert!((result[0].score - 1.0 / 61.0).abs() < 1e-6);
        assert!((result[1].score - 1.0 / 62.0).abs() < 1e-6);
        assert!((result[2].score - 1.0 / 63.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_fuse_dedup_by_composite_key() {
        // Same chunk appears in both lists (same file_path, start_line, end_line)
        let list_a = vec![make_result("a.ts", 1, 10, 0.9)];
        let list_b = vec![make_result("a.ts", 1, 10, 0.8)];

        let result = rrf_fuse(&[list_a, list_b], 60, 10);

        // Should dedup to a single result
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].file_path, "a.ts");
        // Score should be sum: 1/(60+1) + 1/(60+1) = 2/61
        assert!((result[0].score - 2.0 / 61.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_fuse_overlapping_results_rank_higher() {
        // Chunk "shared.ts" appears in both lists → higher RRF score
        let list_a = vec![
            make_result("shared.ts", 1, 10, 0.9), // rank 1 in A
            make_result("only_a.ts", 1, 10, 0.8), // rank 2 in A
        ];
        let list_b = vec![
            make_result("shared.ts", 1, 10, 0.7), // rank 1 in B
            make_result("only_b.ts", 1, 10, 0.6), // rank 2 in B
        ];

        let result = rrf_fuse(&[list_a, list_b], 60, 10);

        assert_eq!(result.len(), 3);
        // shared.ts has score 1/61 + 1/61 = 2/61 ≈ 0.0328
        // only_a.ts has score 1/62 ≈ 0.0161
        // only_b.ts has score 1/62 ≈ 0.0161
        assert_eq!(
            result[0].file_path, "shared.ts",
            "Shared chunk should rank first"
        );
        assert!(
            result[0].score > result[1].score,
            "Shared should have higher score"
        );
    }

    #[test]
    fn rrf_fuse_respects_limit() {
        let list = vec![
            make_result("a.ts", 1, 10, 0.9),
            make_result("b.ts", 1, 10, 0.8),
            make_result("c.ts", 1, 10, 0.7),
            make_result("d.ts", 1, 10, 0.6),
            make_result("e.ts", 1, 10, 0.5),
        ];

        let result = rrf_fuse(&[list], 60, 3);

        assert_eq!(result.len(), 3, "Should truncate to limit");
    }

    #[test]
    fn rrf_fuse_different_composite_keys_not_deduped() {
        // Same file_path but different line ranges → different chunks
        let list = vec![
            make_result("a.ts", 1, 10, 0.9),
            make_result("a.ts", 11, 20, 0.8),
        ];

        let result = rrf_fuse(&[list], 60, 10);

        assert_eq!(
            result.len(),
            2,
            "Different line ranges should not be deduped"
        );
    }

    #[test]
    fn rrf_fuse_k_parameter_affects_scores() {
        let list = vec![make_result("a.ts", 1, 10, 0.9)];

        let result_k60 = rrf_fuse(std::slice::from_ref(&list), 60, 10);
        let result_k10 = rrf_fuse(std::slice::from_ref(&list), 10, 10);

        // Lower k → higher score for same rank
        assert!(result_k10[0].score > result_k60[0].score);
        assert!((result_k60[0].score - 1.0 / 61.0).abs() < 1e-6);
        assert!((result_k10[0].score - 1.0 / 11.0).abs() < 1e-6);
    }
    // ─── HybridSearcher tests ──────────────────────────────────────────

    /// A mock SearchStrategy that always fails — for testing graceful degradation.
    struct FailingSearcher;

    #[async_trait]
    impl SearchStrategy for FailingSearcher {
        async fn search(&self, _query: &str, _options: SearchOptions) -> Result<Vec<SearchResult>> {
            Err(anyhow::anyhow!("mock search failure"))
        }

        fn mode(&self) -> SearchMode {
            SearchMode::Dense // doesn't matter for tests
        }
    }

    /// A mock SearchStrategy that returns predefined results.
    struct MockSearcher {
        results: Vec<SearchResult>,
        mode: SearchMode,
    }

    #[async_trait]
    impl SearchStrategy for MockSearcher {
        async fn search(&self, _query: &str, _options: SearchOptions) -> Result<Vec<SearchResult>> {
            Ok(self.results.clone())
        }

        fn mode(&self) -> SearchMode {
            self.mode
        }
    }

    #[tokio::test]
    async fn hybrid_searcher_mode_returns_hybrid() {
        let dense = Arc::new(MockSearcher {
            results: vec![],
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(MockSearcher {
            results: vec![],
            mode: SearchMode::Sparse,
        });
        let hybrid = HybridSearcher::new(dense, sparse, 60);
        assert_eq!(hybrid.mode(), SearchMode::Hybrid);
    }

    #[tokio::test]
    async fn hybrid_searcher_returns_fused_results() {
        let dense_results = vec![
            make_result("shared.ts", 1, 10, 0.9),
            make_result("only_dense.ts", 1, 10, 0.7),
        ];
        let sparse_results = vec![
            make_result("shared.ts", 1, 10, 0.8),
            make_result("only_sparse.ts", 1, 10, 0.6),
        ];

        let dense = Arc::new(MockSearcher {
            results: dense_results,
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(MockSearcher {
            results: sparse_results,
            mode: SearchMode::Sparse,
        });

        let hybrid = HybridSearcher::new(dense, sparse, 60);
        let options = SearchOptions::default();
        let results = hybrid.search("test query", options).await.unwrap();

        // Should have 3 unique results (shared.ts deduped)
        assert_eq!(results.len(), 3);
        // shared.ts should rank first (appears in both lists)
        assert_eq!(results[0].file_path, "shared.ts");
    }

    #[tokio::test]
    async fn hybrid_searcher_falls_back_to_dense_on_sparse_failure() {
        let dense_results = vec![make_result("a.ts", 1, 10, 0.9)];

        let dense = Arc::new(MockSearcher {
            results: dense_results,
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(FailingSearcher);

        let hybrid = HybridSearcher::new(dense, sparse, 60);
        let options = SearchOptions::default();
        let results = hybrid.search("test query", options).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "a.ts");
    }

    #[tokio::test]
    async fn hybrid_searcher_falls_back_to_sparse_on_dense_failure() {
        let sparse_results = vec![make_result("b.ts", 1, 10, 0.8)];

        let dense = Arc::new(FailingSearcher);
        let sparse = Arc::new(MockSearcher {
            results: sparse_results,
            mode: SearchMode::Sparse,
        });

        let hybrid = HybridSearcher::new(dense, sparse, 60);
        let options = SearchOptions::default();
        let results = hybrid.search("test query", options).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "b.ts");
    }

    #[tokio::test]
    async fn hybrid_searcher_returns_error_when_both_fail() {
        let dense = Arc::new(FailingSearcher);
        let sparse = Arc::new(FailingSearcher);

        let hybrid = HybridSearcher::new(dense, sparse, 60);
        let options = SearchOptions::default();
        let result = hybrid.search("test query", options).await;

        assert!(result.is_err(), "Should return error when both fail");
    }

    #[tokio::test]
    async fn hybrid_searcher_respects_limit() {
        let dense_results = vec![
            make_result("a.ts", 1, 10, 0.9),
            make_result("b.ts", 1, 10, 0.8),
            make_result("c.ts", 1, 10, 0.7),
        ];
        let sparse_results = vec![
            make_result("d.ts", 1, 10, 0.6),
            make_result("e.ts", 1, 10, 0.5),
        ];

        let dense = Arc::new(MockSearcher {
            results: dense_results,
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(MockSearcher {
            results: sparse_results,
            mode: SearchMode::Sparse,
        });

        let hybrid = HybridSearcher::new(dense, sparse, 60);
        let options = SearchOptions {
            limit: 2,
            ..Default::default()
        };
        let results = hybrid.search("test query", options).await.unwrap();

        assert_eq!(results.len(), 2, "Should respect limit");
    }

    #[tokio::test]
    async fn hybrid_searcher_both_empty_returns_empty() {
        let dense = Arc::new(MockSearcher {
            results: vec![],
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(MockSearcher {
            results: vec![],
            mode: SearchMode::Sparse,
        });

        let hybrid = HybridSearcher::new(dense, sparse, 60);
        let options = SearchOptions::default();
        let results = hybrid.search("test query", options).await.unwrap();

        assert!(results.is_empty(), "Both empty should return empty");
    }

    #[tokio::test]
    async fn hybrid_searcher_implements_search_strategy() {
        let dense = Arc::new(MockSearcher {
            results: vec![],
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(MockSearcher {
            results: vec![],
            mode: SearchMode::Sparse,
        });
        let hybrid = HybridSearcher::new(dense, sparse, 60);

        // Verify we can use it through the trait
        let strategy: &dyn SearchStrategy = &hybrid;
        assert_eq!(strategy.mode(), SearchMode::Hybrid);
    }

    // ─── MockReranker ──────────────────────────────────────────────────

    use crate::error::VectorCodeError;

    struct MockReranker {
        scores: Vec<(usize, f32)>,
        should_fail: bool,
        delay: Option<Duration>,
    }

    impl MockReranker {
        fn new(scores: Vec<(usize, f32)>) -> Self {
            Self {
                scores,
                should_fail: false,
                delay: None,
            }
        }

        fn failing() -> Self {
            Self {
                scores: vec![],
                should_fail: true,
                delay: None,
            }
        }

        fn slow(delay: Duration) -> Self {
            Self {
                scores: vec![],
                should_fail: false,
                delay: Some(delay),
            }
        }
    }

    #[async_trait]
    impl Reranker for MockReranker {
        async fn rerank(
            &self,
            _query: &str,
            _docs: &[RerankDocument],
        ) -> crate::reranker::Result<Vec<(usize, f32)>> {
            if self.should_fail {
                return Err(VectorCodeError::RerankerError {
                    message: "mock failure".into(),
                });
            }
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            Ok(self.scores.clone())
        }

        fn model_name(&self) -> &str {
            "mock-reranker"
        }

        fn provider_name(&self) -> &str {
            "mock"
        }
    }

    fn make_reranker_searcher(
        dense_results: Vec<SearchResult>,
        sparse_results: Vec<SearchResult>,
        reranker: MockReranker,
        top_k: usize,
        timeout: Duration,
    ) -> HybridSearcher {
        let dense = Arc::new(MockSearcher {
            results: dense_results,
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(MockSearcher {
            results: sparse_results,
            mode: SearchMode::Sparse,
        });
        HybridSearcher::with_reranker(dense, sparse, 60, Some(Arc::new(reranker)), top_k, timeout)
    }

    // ─── HybridSearcher + Reranker tests ────────────────────────────────

    #[tokio::test]
    async fn hybrid_searcher_with_reranker_mode_returns_hybrid_rerank() {
        let dense = Arc::new(MockSearcher {
            results: vec![],
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(MockSearcher {
            results: vec![],
            mode: SearchMode::Sparse,
        });
        let searcher = HybridSearcher::with_reranker(
            dense,
            sparse,
            60,
            Some(Arc::new(MockReranker::new(vec![]))),
            20,
            Duration::from_secs(5),
        );
        assert_eq!(searcher.mode(), SearchMode::HybridRerank);
    }

    #[tokio::test]
    async fn hybrid_searcher_with_reranker_success_reorders_top_k() {
        // 3 candidates from dense, 0 from sparse (simple case)
        let dense_results = vec![
            make_result("a.ts", 1, 10, 0.9),
            make_result("b.ts", 1, 10, 0.8),
            make_result("c.ts", 1, 10, 0.7),
        ];

        // Reranker reverses the order: c > b > a
        let reranker = MockReranker::new(vec![(2, 0.95), (1, 0.85), (0, 0.75)]);
        let searcher =
            make_reranker_searcher(dense_results, vec![], reranker, 3, Duration::from_secs(5));

        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();

        // After reranking: c.ts first (score 0.95), b.ts second (0.85), a.ts third (0.75)
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].file_path, "c.ts");
        assert!((results[0].score - 0.95).abs() < 1e-6);
        assert_eq!(results[1].file_path, "b.ts");
        assert!((results[1].score - 0.85).abs() < 1e-6);
        assert_eq!(results[2].file_path, "a.ts");
        assert!((results[2].score - 0.75).abs() < 1e-6);
    }

    #[tokio::test]
    async fn hybrid_searcher_with_reranker_preserves_metadata() {
        let dense_results = vec![make_result("x.ts", 42, 50, 0.9)];

        // Reranker returns same score but different value
        let reranker = MockReranker::new(vec![(0, 0.99)]);
        let searcher =
            make_reranker_searcher(dense_results, vec![], reranker, 1, Duration::from_secs(5));

        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();

        assert_eq!(results.len(), 1);
        // Metadata preserved from original SearchResult
        assert_eq!(results[0].file_path, "x.ts");
        assert_eq!(results[0].start_line, 42);
        assert_eq!(results[0].end_line, 50);
        assert_eq!(results[0].kind, "function_declaration");
        assert_eq!(results[0].language, "typescript");
        // Score updated by reranker
        assert!((results[0].score - 0.99).abs() < 1e-6);
    }

    #[tokio::test]
    async fn hybrid_searcher_with_reranker_timeout_fallback() {
        let dense_results = vec![
            make_result("a.ts", 1, 10, 0.9),
            make_result("b.ts", 1, 10, 0.8),
        ];

        // Reranker that takes 2 seconds — timeout is 50ms
        let reranker = MockReranker::slow(Duration::from_secs(2));
        let searcher = make_reranker_searcher(
            dense_results,
            vec![],
            reranker,
            2,
            Duration::from_millis(50),
        );

        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();

        // Should fall back to RRF order (not crash)
        assert_eq!(results.len(), 2);
        // RRF scores are deterministic — just verify we got results back
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn hybrid_searcher_with_reranker_error_fallback() {
        let dense_results = vec![
            make_result("a.ts", 1, 10, 0.9),
            make_result("b.ts", 1, 10, 0.8),
        ];

        let reranker = MockReranker::failing();
        let searcher =
            make_reranker_searcher(dense_results, vec![], reranker, 2, Duration::from_secs(5));

        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();

        // Should fall back to RRF order (not error)
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn hybrid_searcher_no_reranker_behaves_like_before() {
        let dense_results = vec![
            make_result("shared.ts", 1, 10, 0.9),
            make_result("only_dense.ts", 1, 10, 0.7),
        ];
        let sparse_results = vec![
            make_result("shared.ts", 1, 10, 0.8),
            make_result("only_sparse.ts", 1, 10, 0.6),
        ];

        // with_reranker but reranker is None — should behave like plain hybrid
        let dense = Arc::new(MockSearcher {
            results: dense_results,
            mode: SearchMode::Dense,
        });
        let sparse = Arc::new(MockSearcher {
            results: sparse_results,
            mode: SearchMode::Sparse,
        });
        let searcher = HybridSearcher::with_reranker(
            dense,
            sparse,
            60,
            None, // no reranker
            20,
            Duration::from_secs(5),
        );

        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();

        // Same behavior as HybridSearcher::new — 3 unique fused results
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].file_path, "shared.ts");
    }

    #[tokio::test]
    async fn hybrid_searcher_with_reranker_empty_fused_is_noop() {
        let reranker = MockReranker::new(vec![]);
        let searcher = make_reranker_searcher(vec![], vec![], reranker, 20, Duration::from_secs(5));

        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn hybrid_searcher_with_reranker_top_k_smaller_than_fused() {
        // 5 fused results, but reranker only processes top 2
        let dense_results = vec![
            make_result("a.ts", 1, 10, 0.9),
            make_result("b.ts", 1, 10, 0.8),
            make_result("c.ts", 1, 10, 0.7),
        ];

        // Reranker reverses top 2: b > a
        let reranker = MockReranker::new(vec![(1, 0.99), (0, 0.88)]);
        let searcher =
            make_reranker_searcher(dense_results, vec![], reranker, 2, Duration::from_secs(5));

        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();

        // Top 2 reranked + 1 tail (c.ts)
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].file_path, "b.ts");
        assert!((results[0].score - 0.99).abs() < 1e-6);
        assert_eq!(results[1].file_path, "a.ts");
        assert!((results[1].score - 0.88).abs() < 1e-6);
        // Tail preserved
        assert_eq!(results[2].file_path, "c.ts");
    }
}
