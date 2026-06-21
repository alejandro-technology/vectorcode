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
use crate::engine::{build_strategy, Indexer, SearchMode, SearchStrategy};
use crate::store::db::Database;
use crate::types::SearchResult;

/// Run a full benchmark against a corpus using the specified search mode.
///
/// This function:
/// - Creates a temporary directory for the corpus
/// - Indexes all corpus files with the provided embedder
/// - Builds the appropriate search strategy for the given mode
/// - Executes each query through the strategy
/// - Computes per-query and aggregate metrics
/// - Cleans up the temp directory on drop
pub async fn run_benchmark(
    corpus: &dyn Corpus,
    queries: &QuerySet,
    embedder: Arc<dyn Embedder>,
    mode: SearchMode,
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

    // Step 3: Build search strategy for the requested mode
    let mut search_config = SearchConfig::default();
    if mode == SearchMode::HybridRerank {
        search_config.rerank.enabled = true;
    }
    let strategy = build_strategy(mode, db.clone(), embedder.clone(), search_config).await;

    // Step 4: Execute queries and compute metrics
    let mut query_results = Vec::new();

    for query in &queries.queries {
        let result = match query.kind {
            crate::bench::schema::QueryKind::Semantic => {
                execute_query(query, strategy.as_ref(), corpus_path).await?
            }
            crate::bench::schema::QueryKind::Structural => {
                execute_structural_query(query, &db, corpus_path).await?
            }
        };
        query_results.push(result);
    }

    // Step 5: Compute aggregate metrics
    let aggregate = compute_aggregate_metrics(&query_results);

    let duration = start.elapsed();

    Ok(BenchmarkResult {
        corpus: corpus.name().to_string(),
        search_mode: mode.to_string(),
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
    strategy: &dyn SearchStrategy,
    corpus_path: &Path,
) -> Result<QueryResult> {
    // Run search with limit=10 (for recall@10 and nDCG@10)
    let search_options = crate::engine::SearchOptions {
        limit: 10,
        threshold: 0.0, // No threshold for benchmarking
        language: None,
        path: None,
        ..Default::default()
    };

    let start_time = Instant::now();
    let results = strategy.search(&query.text, search_options).await?;
    let latency_ms = start_time.elapsed().as_secs_f64() * 1000.0;

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
        latency_ms,
        symbol_recall_at_5: 0.0,
        symbol_recall_at_10: 0.0,
        symbol_precision_at_5: 0.0,
    })
}

/// Execute a structural query and compute symbol-level metrics.
async fn execute_structural_query(
    query: &Query,
    db: &Arc<tokio::sync::Mutex<Database>>,
    corpus_path: &Path,
) -> Result<QueryResult> {
    use crate::bench::schema::validate_structural;
    use crate::store::graph::GraphStore;

    // Validate structural query
    validate_structural(query).map_err(|e| anyhow::anyhow!(e))?;

    // After validate_structural, target_symbol and target_tool are guaranteed
    // to be set. If they are not, return an error rather than panicking.
    let target_symbol = query.target_symbol.as_ref().ok_or_else(|| {
        anyhow::anyhow!("structural query missing target_symbol after validation")
    })?;
    let target_tool = query
        .target_tool
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("structural query missing target_tool after validation"))?;

    let start_time = Instant::now();

    // Call the appropriate GraphStore method
    let db_guard = db.lock().await;
    let nodes = match target_tool.as_str() {
        "callers" => db_guard.get_callers(target_symbol)?,
        "dependents" => db_guard.get_dependents(target_symbol, None)?,
        "imports" => db_guard.get_imports(target_symbol, None)?,
        other => anyhow::bail!("Unknown target_tool: {other}"),
    };

    let latency_ms = start_time.elapsed().as_secs_f64() * 1000.0;

    // Convert GraphNode[] to predicted symbol keys, stripping corpus prefix
    let predicted: Vec<String> = nodes
        .iter()
        .map(|n| {
            if n.file_path.is_empty() {
                format!("::{}", n.symbol)
            } else {
                // Strip corpus temp-dir prefix to get corpus-relative path
                let rel_path = Path::new(&n.file_path)
                    .strip_prefix(corpus_path)
                    .unwrap_or(Path::new(&n.file_path))
                    .to_string_lossy()
                    .to_string();
                format!("{rel_path}::{}", n.symbol)
            }
        })
        .collect();

    // Build expected set from expected_symbols (grade >= 1)
    let expected: HashSet<String> = query
        .expected_symbols
        .iter()
        .filter(|s| s.grade >= 1)
        .map(|s| {
            if let Some(ref file) = s.file {
                format!("{file}::{}", s.symbol)
            } else {
                format!("::{}", s.symbol)
            }
        })
        .collect();

    // Compute symbol metrics
    let symbol_recall_at_5 = metrics::symbol_recall_at_k(&predicted, &expected, 5);
    let symbol_recall_at_10 = metrics::symbol_recall_at_k(&predicted, &expected, 10);
    let symbol_precision_at_5 = metrics::symbol_precision_at_k(&predicted, &expected, 5);

    Ok(QueryResult {
        query: query.text.clone(),
        predicted,
        recall_at_5: 0.0,
        recall_at_10: 0.0,
        ndcg_at_10: 0.0,
        mrr: 0.0,
        latency_ms,
        symbol_recall_at_5,
        symbol_recall_at_10,
        symbol_precision_at_5,
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
            latency_p50_ms: 0.0,
            latency_p95_ms: 0.0,
            latency_avg_ms: 0.0,
            symbol_recall_at_5: 0.0,
            symbol_recall_at_10: 0.0,
            symbol_precision_at_5: 0.0,
        };
    }

    let n = query_results.len() as f64;
    let recall_at_5 = query_results.iter().map(|r| r.recall_at_5).sum::<f64>() / n;
    let recall_at_10 = query_results.iter().map(|r| r.recall_at_10).sum::<f64>() / n;
    let ndcg_at_10 = query_results.iter().map(|r| r.ndcg_at_10).sum::<f64>() / n;
    let mrr = query_results.iter().map(|r| r.mrr).sum::<f64>() / n;
    let symbol_recall_at_5 = query_results
        .iter()
        .map(|r| r.symbol_recall_at_5)
        .sum::<f64>()
        / n;
    let symbol_recall_at_10 = query_results
        .iter()
        .map(|r| r.symbol_recall_at_10)
        .sum::<f64>()
        / n;
    let symbol_precision_at_5 = query_results
        .iter()
        .map(|r| r.symbol_precision_at_5)
        .sum::<f64>()
        / n;

    let latency_avg_ms = query_results.iter().map(|r| r.latency_ms).sum::<f64>() / n;

    let mut latencies: Vec<f64> = query_results.iter().map(|r| r.latency_ms).collect();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let p50_idx = ((n - 1.0) * 0.5).round() as usize;
    let p95_idx = ((n - 1.0) * 0.95).round() as usize;
    let latency_p50_ms = latencies.get(p50_idx).copied().unwrap_or(0.0);
    let latency_p95_ms = latencies.get(p95_idx).copied().unwrap_or(0.0);

    AggregateMetrics {
        recall_at_5,
        recall_at_10,
        ndcg_at_10,
        mrr,
        latency_p50_ms,
        latency_p95_ms,
        latency_avg_ms,
        symbol_recall_at_5,
        symbol_recall_at_10,
        symbol_precision_at_5,
    }
}

/// Run benchmarks across multiple search modes and collect all results.
///
/// Calls `run_benchmark` for each mode in sequence. Useful for comparing
/// dense vs hybrid vs hybrid-rerank performance in a single CLI invocation.
pub async fn run_multi_mode_benchmark(
    corpus: &dyn Corpus,
    queries: &QuerySet,
    embedder: Arc<dyn Embedder>,
    modes: &[SearchMode],
) -> Result<Vec<BenchmarkResult>> {
    let mut results = Vec::with_capacity(modes.len());
    for &mode in modes {
        let result = run_benchmark(corpus, queries, embedder.clone(), mode).await?;
        results.push(result);
    }
    Ok(results)
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
                kind: crate::bench::schema::QueryKind::Semantic,
                expected_symbols: vec![],
                target_symbol: None,
                target_tool: None,
            }],
        };

        let embedder = Arc::new(MockEmbedder::new(384));
        let result = run_benchmark(&corpus, &queries, embedder, SearchMode::Dense).await;

        assert!(
            result.is_ok(),
            "Benchmark should succeed: {:?}",
            result.err()
        );
        let result = result.unwrap();
        assert_eq!(result.corpus, "test");
        assert_eq!(result.search_mode, "dense");
        assert_eq!(result.files_indexed, 2);
        assert_eq!(result.queries_executed, 1);
        assert!(!result.query_results.is_empty());
    }

    #[test]
    fn test_dedupe_to_file_rank() {
        let corpus_path = Path::new("/tmp/corpus");
        let results = vec![
            SearchResult {
                repo_name: None,
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
                repo_name: None,
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
                repo_name: None,
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
                latency_ms: 20.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            QueryResult {
                query: "q2".to_string(),
                predicted: vec![],
                recall_at_5: 0.7,
                recall_at_10: 0.9,
                ndcg_at_10: 0.8,
                mrr: 1.0,
                latency_ms: 100.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
        ];

        let agg = compute_aggregate_metrics(&results);
        assert!((agg.recall_at_5 - 0.6).abs() < 1e-9);
        assert!((agg.recall_at_10 - 0.85).abs() < 1e-9);
        assert!((agg.ndcg_at_10 - 0.7).abs() < 1e-9);
        assert!((agg.mrr - 0.75).abs() < 1e-9);
    }

    #[test]
    fn test_compute_aggregate_metrics_latency() {
        let results = vec![
            QueryResult {
                query: "q1".to_string(),
                predicted: vec![],
                recall_at_5: 0.0,
                recall_at_10: 0.0,
                ndcg_at_10: 0.0,
                mrr: 0.0,
                latency_ms: 10.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            QueryResult {
                query: "q2".to_string(),
                predicted: vec![],
                recall_at_5: 0.0,
                recall_at_10: 0.0,
                ndcg_at_10: 0.0,
                mrr: 0.0,
                latency_ms: 30.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            QueryResult {
                query: "q3".to_string(),
                predicted: vec![],
                recall_at_5: 0.0,
                recall_at_10: 0.0,
                ndcg_at_10: 0.0,
                mrr: 0.0,
                latency_ms: 20.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            QueryResult {
                query: "q4".to_string(),
                predicted: vec![],
                recall_at_5: 0.0,
                recall_at_10: 0.0,
                ndcg_at_10: 0.0,
                mrr: 0.0,
                latency_ms: 40.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            QueryResult {
                query: "q5".to_string(),
                predicted: vec![],
                recall_at_5: 0.0,
                recall_at_10: 0.0,
                ndcg_at_10: 0.0,
                mrr: 0.0,
                latency_ms: 50.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
        ];
        let agg = compute_aggregate_metrics(&results);
        assert_eq!(agg.latency_p50_ms, 30.0);
        assert_eq!(agg.latency_p95_ms, 50.0);
        assert_eq!(agg.latency_avg_ms, 30.0);
    }

    #[test]
    fn test_compute_aggregate_metrics_empty() {
        let agg = compute_aggregate_metrics(&[]);
        assert_eq!(agg.recall_at_5, 0.0);
        assert_eq!(agg.recall_at_10, 0.0);
        assert_eq!(agg.ndcg_at_10, 0.0);
        assert_eq!(agg.mrr, 0.0);
    }

    /// Helper: create a temporary corpus with two test files.
    async fn setup_test_corpus() -> (TempDir, LocalCorpus) {
        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();

        tokio::fs::write(
            src_path.join("error.rs"),
            "pub struct Error { message: String }\nimpl std::fmt::Display for Error {}\n",
        )
        .await
        .unwrap();

        tokio::fs::write(
            src_path.join("search.rs"),
            "pub fn search(query: &str) -> Vec<String> { vec![query.to_string()] }\n",
        )
        .await
        .unwrap();

        let corpus = LocalCorpus::new(
            "test".to_string(),
            src_path.to_path_buf(),
            vec![".rs".to_string()],
        );
        (src_dir, corpus)
    }

    fn test_queries() -> QuerySet {
        QuerySet {
            name: "test".to_string(),
            queries: vec![Query {
                text: "error handling".to_string(),
                judgments: vec![RelevanceJudgment {
                    file: "error.rs".to_string(),
                    grade: 3,
                }],
                kind: crate::bench::schema::QueryKind::Semantic,
                expected_symbols: vec![],
                target_symbol: None,
                target_tool: None,
            }],
        }
    }

    #[tokio::test]
    async fn test_benchmark_result_includes_mode() {
        let (_dir, corpus) = setup_test_corpus().await;
        let queries = test_queries();
        let embedder = Arc::new(MockEmbedder::new(384));

        let result = run_benchmark(&corpus, &queries, embedder, SearchMode::Dense)
            .await
            .unwrap();

        assert_eq!(result.search_mode, "dense");
    }

    #[tokio::test]
    async fn test_run_multi_mode_benchmark() {
        let (_dir, corpus) = setup_test_corpus().await;
        let queries = test_queries();
        let embedder = Arc::new(MockEmbedder::new(384));

        let modes = vec![SearchMode::Dense, SearchMode::Hybrid];
        let results = run_multi_mode_benchmark(&corpus, &queries, embedder, &modes)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].search_mode, "dense");
        assert_eq!(results[1].search_mode, "hybrid");

        // Both should have indexed the same number of files
        assert_eq!(results[0].files_indexed, 2);
        assert_eq!(results[1].files_indexed, 2);
    }

    #[test]
    fn mini_structural_toml_loads_10_plus() {
        let toml_str = std::fs::read_to_string("benchmarks/queries/mini_structural.toml")
            .expect("mini_structural.toml should exist");
        let query_set: QuerySet = toml::from_str(&toml_str).expect("should parse as QuerySet");

        let structural_count = query_set
            .queries
            .iter()
            .filter(|q| q.kind == crate::bench::schema::QueryKind::Structural)
            .count();

        assert!(
            structural_count >= 10,
            "mini_structural.toml should have at least 10 structural queries, got {structural_count}"
        );

        // Verify all structural queries have expected_symbols
        for query in &query_set.queries {
            if query.kind == crate::bench::schema::QueryKind::Structural {
                assert!(
                    !query.expected_symbols.is_empty() || query.target_symbol.is_some(),
                    "Structural query '{}' should have expected_symbols or target_symbol",
                    query.text
                );
            }
        }
    }
}
