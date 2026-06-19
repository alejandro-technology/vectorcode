//! Benchmark runner — orchestrates corpus → index → search → metrics.
//!
//! `run_benchmark` is the main entry point. It:
//! 1. Prepares the corpus (copy or clone)
//! 2. Indexes files with the provided embedder
//! 3. Runs each query through the Searcher
//! 4. Deduplicates chunk results to file-level ranking
//! 5. Computes metrics (recall@k, nDCG@k, MRR)
//! 6. Returns aggregate BenchmarkResult

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tempfile::TempDir;

use crate::bench::corpus::Corpus;
use crate::bench::metrics;
use crate::bench::schema::{AggregateMetrics, BenchmarkResult, Query, QueryResult, QuerySet};
use crate::config::schema::{IndexingConfig, SearchConfig};
use crate::embedder::Embedder;
use crate::engine::{Indexer, Searcher};
use crate::store::db::Database;
use crate::types::SearchResult;

/// Run a full benchmark against a corpus.
///
/// This function:
/// - Creates a temporary directory for the corpus
/// - Indexes all corpus files with the provided embedder
/// - Executes each query through the Searcher
/// - Computes per-query and aggregate metrics
/// - Cleans up the temp directory on drop
pub async fn run_benchmark(
    corpus: &dyn Corpus,
    queries: &QuerySet,
    embedder: Arc<dyn Embedder>,
) -> Result<BenchmarkResult> {
    let start = Instant::now();

    // Step 1: Prepare corpus in a temporary directory
    let temp_dir = TempDir::new()?;
    let corpus_path = temp_dir.path();

    let relative_files = corpus.prepare(corpus_path).await?;
    if relative_files.is_empty() {
        anyhow::bail!(
            "Corpus '{}' produced no files. Check file_extensions filter.",
            corpus.name()
        );
    }

    // Convert relative paths to absolute for indexing
    let absolute_files: Vec<std::path::PathBuf> = relative_files
        .iter()
        .map(|rel| corpus_path.join(rel))
        .collect();

    // Step 2: Index corpus files
    let db = Database::open_in_memory()?;
    db.init_schema(embedder.dimensions())?;

    let db = Arc::new(tokio::sync::Mutex::new(db));
    let indexing_config = IndexingConfig::default();
    let indexer = Indexer::new(db.clone(), embedder.clone(), indexing_config);

    let index_report = indexer.index_files(&absolute_files, corpus_path).await?;

    // Step 3: Create searcher
    let search_config = SearchConfig::default();
    let searcher = Searcher::new(db.clone(), embedder.clone(), search_config);

    // Step 4: Execute queries and compute metrics
    let mut query_results = Vec::new();

    for query in &queries.queries {
        let result = execute_query(query, &searcher, corpus_path).await?;
        query_results.push(result);
    }

    // Step 5: Compute aggregate metrics
    let aggregate = compute_aggregate_metrics(&query_results);

    let duration = start.elapsed();

    Ok(BenchmarkResult {
        corpus: corpus.name().to_string(),
        files_indexed: index_report.files_indexed,
        chunks_created: index_report.chunks_new,
        queries_executed: queries.queries.len(),
        query_results,
        aggregate,
        duration_secs: duration.as_secs_f64(),
    })
}

/// Execute a single query and compute its metrics.
async fn execute_query(
    query: &Query,
    searcher: &Searcher,
    corpus_path: &Path,
) -> Result<QueryResult> {
    // Run search with limit=10 (for recall@10 and nDCG@10)
    let search_options = crate::engine::SearchOptions {
        limit: 10,
        threshold: 0.0, // No threshold for benchmarking
        language: None,
        path: None,
    };

    let results = searcher.search(&query.text, search_options).await?;

    // Deduplicate chunk results to file-level ranking
    let predicted = dedupe_to_file_rank(&results, corpus_path);

    // Build relevance sets from judgments
    let relevant: HashSet<String> = query
        .judgments
        .iter()
        .filter(|j| j.grade >= 1)
        .map(|j| j.file.clone())
        .collect();

    let grades: HashMap<String, f64> = query
        .judgments
        .iter()
        .map(|j| (j.file.clone(), j.grade as f64))
        .collect();

    // Compute metrics
    let recall_at_5 = metrics::recall_at_k(&predicted, &relevant, 5);
    let recall_at_10 = metrics::recall_at_k(&predicted, &relevant, 10);
    let ndcg_at_10 = metrics::ndcg_at_k(&predicted, &grades, 10);
    let mrr = metrics::mrr(&predicted, &relevant);

    Ok(QueryResult {
        query: query.text.clone(),
        predicted,
        recall_at_5,
        recall_at_10,
        ndcg_at_10,
        mrr,
    })
}

/// Deduplicate chunk-level search results to file-level ranking.
///
/// For each file, keep the highest-scoring chunk. Return files in order
/// of their best chunk's score (descending).
fn dedupe_to_file_rank(results: &[SearchResult], corpus_path: &Path) -> Vec<String> {
    let mut seen: HashMap<String, f32> = HashMap::new();

    for result in results {
        // Convert absolute path to relative (corpus-relative)
        let rel_path = Path::new(&result.file_path)
            .strip_prefix(corpus_path)
            .unwrap_or(Path::new(&result.file_path))
            .to_string_lossy()
            .to_string();

        // Keep the highest score for each file
        let entry = seen.entry(rel_path).or_insert(0.0);
        if result.score > *entry {
            *entry = result.score;
        }
    }

    // Sort by score descending
    let mut files: Vec<(String, f32)> = seen.into_iter().collect();
    files.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    files.into_iter().map(|(path, _)| path).collect()
}

/// Compute aggregate metrics (mean across all queries).
fn compute_aggregate_metrics(query_results: &[QueryResult]) -> AggregateMetrics {
    if query_results.is_empty() {
        return AggregateMetrics {
            recall_at_5: 0.0,
            recall_at_10: 0.0,
            ndcg_at_10: 0.0,
            mrr: 0.0,
        };
    }

    let n = query_results.len() as f64;
    let recall_at_5 = query_results.iter().map(|r| r.recall_at_5).sum::<f64>() / n;
    let recall_at_10 = query_results.iter().map(|r| r.recall_at_10).sum::<f64>() / n;
    let ndcg_at_10 = query_results.iter().map(|r| r.ndcg_at_10).sum::<f64>() / n;
    let mrr = query_results.iter().map(|r| r.mrr).sum::<f64>() / n;

    AggregateMetrics {
        recall_at_5,
        recall_at_10,
        ndcg_at_10,
        mrr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::corpus::LocalCorpus;
    use crate::bench::schema::{Query, QuerySet, RelevanceJudgment};
    use crate::embedder::mock::MockEmbedder;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_run_benchmark_with_local_corpus() {
        // Create a temporary corpus with test files
        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();

        // Create test files with distinct content
        tokio::fs::write(
            src_path.join("error.rs"),
            r#"
use std::fmt;

#[derive(Debug)]
pub struct Error {
    message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}
"#,
        )
        .await
        .unwrap();

        tokio::fs::write(
            src_path.join("search.rs"),
            r#"
pub fn search(query: &str) -> Vec<String> {
    // Search implementation
    vec![query.to_string()]
}
"#,
        )
        .await
        .unwrap();

        let corpus = LocalCorpus::new(
            "test".to_string(),
            src_path.to_path_buf(),
            vec![".rs".to_string()],
        );

        let queries = QuerySet {
            name: "test".to_string(),
            queries: vec![Query {
                text: "error handling".to_string(),
                judgments: vec![RelevanceJudgment {
                    file: "error.rs".to_string(),
                    grade: 3,
                }],
            }],
        };

        let embedder = Arc::new(MockEmbedder::new(384));
        let result = run_benchmark(&corpus, &queries, embedder).await;

        assert!(
            result.is_ok(),
            "Benchmark should succeed: {:?}",
            result.err()
        );
        let result = result.unwrap();
        assert_eq!(result.corpus, "test");
        assert_eq!(result.files_indexed, 2);
        assert_eq!(result.queries_executed, 1);
        assert!(!result.query_results.is_empty());
    }

    #[test]
    fn test_dedupe_to_file_rank() {
        let corpus_path = Path::new("/tmp/corpus");
        let results = vec![
            SearchResult {
                file_path: "/tmp/corpus/a.rs".to_string(),
                start_line: 1,
                end_line: 10,
                symbol: None,
                kind: "function".to_string(),
                language: "rust".to_string(),
                parent_context: None,
                content: "fn foo() {}".to_string(),
                score: 0.9,
            },
            SearchResult {
                file_path: "/tmp/corpus/a.rs".to_string(),
                start_line: 20,
                end_line: 30,
                symbol: None,
                kind: "function".to_string(),
                language: "rust".to_string(),
                parent_context: None,
                content: "fn bar() {}".to_string(),
                score: 0.7,
            },
            SearchResult {
                file_path: "/tmp/corpus/b.rs".to_string(),
                start_line: 1,
                end_line: 10,
                symbol: None,
                kind: "function".to_string(),
                language: "rust".to_string(),
                parent_context: None,
                content: "fn baz() {}".to_string(),
                score: 0.8,
            },
        ];

        let ranked = dedupe_to_file_rank(&results, corpus_path);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0], "a.rs"); // Highest score 0.9
        assert_eq!(ranked[1], "b.rs"); // Score 0.8
    }

    #[test]
    fn test_compute_aggregate_metrics() {
        let results = vec![
            QueryResult {
                query: "q1".to_string(),
                predicted: vec![],
                recall_at_5: 0.5,
                recall_at_10: 0.8,
                ndcg_at_10: 0.6,
                mrr: 0.5,
            },
            QueryResult {
                query: "q2".to_string(),
                predicted: vec![],
                recall_at_5: 0.7,
                recall_at_10: 0.9,
                ndcg_at_10: 0.8,
                mrr: 1.0,
            },
        ];

        let agg = compute_aggregate_metrics(&results);
        assert!((agg.recall_at_5 - 0.6).abs() < 1e-9);
        assert!((agg.recall_at_10 - 0.85).abs() < 1e-9);
        assert!((agg.ndcg_at_10 - 0.7).abs() < 1e-9);
        assert!((agg.mrr - 0.75).abs() < 1e-9);
    }

    #[test]
    fn test_compute_aggregate_metrics_empty() {
        let agg = compute_aggregate_metrics(&[]);
        assert_eq!(agg.recall_at_5, 0.0);
        assert_eq!(agg.recall_at_10, 0.0);
        assert_eq!(agg.ndcg_at_10, 0.0);
        assert_eq!(agg.mrr, 0.0);
    }
}
