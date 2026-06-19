//! RRF fusion and HybridSearcher — combines dense + sparse results.
//!
//! Provides `rrf_fuse` pure function for Reciprocal Rank Fusion and
//! `HybridSearcher` which runs dense and sparse in parallel via `tokio::join!`.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

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
pub struct HybridSearcher {
    dense: Arc<dyn SearchStrategy>,
    sparse: Arc<dyn SearchStrategy>,
    rrf_k: u32,
}

impl HybridSearcher {
    /// Create a new HybridSearcher with dense and sparse strategies.
    pub fn new(
        dense: Arc<dyn SearchStrategy>,
        sparse: Arc<dyn SearchStrategy>,
        rrf_k: u32,
    ) -> Self {
        Self {
            dense,
            sparse,
            rrf_k,
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

        match (dense_result, sparse_result) {
            (Ok(dense_results), Ok(sparse_results)) => {
                // Both succeeded — fuse via RRF
                let fused = rrf_fuse(&[dense_results, sparse_results], rrf_k, limit);
                Ok(fused)
            }
            (Ok(dense_results), Err(_)) => {
                // Sparse failed — return dense results (graceful degradation)
                Ok(dense_results.into_iter().take(limit).collect())
            }
            (Err(_), Ok(sparse_results)) => {
                // Dense failed — return sparse results (graceful degradation)
                Ok(sparse_results.into_iter().take(limit).collect())
            }
            (Err(e), Err(_)) => {
                // Both failed — return first error
                Err(e)
            }
        }
    }

    fn mode(&self) -> SearchMode {
        SearchMode::Hybrid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::searcher::SearchMode;

    fn make_result(file_path: &str, start_line: u32, end_line: u32, score: f32) -> SearchResult {
        SearchResult {
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
}
