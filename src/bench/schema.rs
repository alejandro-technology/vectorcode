//! Schema types for benchmark configuration and results (serde-compatible).
//!
//! - `Query`: a single benchmark query with relevance judgments
//! - `RelevanceJudgment`: file path + grade (0-3)
//! - `QuerySet`: collection of queries for a corpus
//! - `CorpusConfig`: corpus definition (local path or git URL + filters)
//! - `BenchmarkResult`: aggregate metrics from a benchmark run

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A single benchmark query with hand-labeled relevance judgments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    /// The query text (natural language or keyword-style).
    pub text: String,

    /// Relevance judgments: file path → grade (0-3).
    /// 0 = irrelevant, 1 = marginally relevant, 2 = relevant, 3 = highly relevant.
    pub judgments: Vec<RelevanceJudgment>,

    /// Query kind: Semantic (default) or Structural.
    #[serde(default)]
    pub kind: QueryKind,

    /// Expected symbols for structural queries (required when kind=Structural).
    #[serde(default)]
    pub expected_symbols: Vec<SymbolJudgment>,

    /// Target symbol for structural queries (e.g., "search" for "who calls search").
    #[serde(default)]
    pub target_symbol: Option<String>,

    /// Target tool for structural queries: "callers", "dependents", or "imports".
    #[serde(default)]
    pub target_tool: Option<String>,
}

/// Kind of benchmark query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum QueryKind {
    /// Semantic query (file-level relevance judgments).
    #[default]
    Semantic,
    /// Structural query (symbol-level expectations).
    Structural,
}

/// Symbol-level judgment for structural queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolJudgment {
    /// Symbol name (e.g., "main", "search").
    pub symbol: String,

    /// Optional file path for disambiguation.
    #[serde(default)]
    pub file: Option<String>,

    /// Relevance grade: 0 (irrelevant) to 3 (highly relevant).
    pub grade: u8,
}

/// File-level relevance judgment for a query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevanceJudgment {
    /// Relative file path within the corpus (e.g., "src/lib.rs").
    pub file: String,

    /// Relevance grade: 0 (irrelevant) to 3 (highly relevant).
    pub grade: u8,
}

/// Collection of queries for a benchmark corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuerySet {
    /// Human-readable name for this query set (e.g., "mini", "vscode").
    pub name: String,

    /// The queries in this set.
    pub queries: Vec<Query>,
}

/// Corpus configuration — defines where files come from and which to include.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusConfig {
    /// Corpus name (e.g., "mini", "vscode").
    pub name: String,

    /// Source: local path or git URL.
    pub source: CorpusSource,

    /// File extensions to include (e.g., [".rs", ".ts", ".py"]).
    pub file_extensions: Vec<String>,

    /// Optional sparse checkout paths (for git corpora).
    #[serde(default)]
    pub sparse_paths: Vec<String>,
}

/// Source of corpus files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CorpusSource {
    /// Local directory (for test fixtures).
    Local { path: PathBuf },

    /// Git repository (cloned at runtime).
    Git { url: String },
}

/// Aggregate metrics from a single benchmark run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    /// Corpus name.
    pub corpus: String,

    /// Search mode used for this run ("dense", "sparse", "hybrid", "hybrid-rerank").
    pub search_mode: String,

    /// Number of files indexed.
    pub files_indexed: usize,

    /// Number of chunks created.
    pub chunks_created: usize,

    /// Number of queries executed.
    pub queries_executed: usize,

    /// Per-query results.
    pub query_results: Vec<QueryResult>,

    /// Aggregate metrics (averaged across queries).
    pub aggregate: AggregateMetrics,

    /// Wall-clock duration in seconds.
    pub duration_secs: f64,
}

/// Results for a single query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// The query text.
    pub query: String,

    /// Ranked list of file paths (highest score first).
    pub predicted: Vec<String>,

    /// Metrics for this query.
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub ndcg_at_10: f64,
    pub mrr: f64,

    /// Symbol-level metrics for structural queries (0.0 for semantic).
    #[serde(default)]
    pub symbol_recall_at_5: f64,
    #[serde(default)]
    pub symbol_recall_at_10: f64,
    #[serde(default)]
    pub symbol_precision_at_5: f64,
}

/// Aggregate metrics across all queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateMetrics {
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub ndcg_at_10: f64,
    pub mrr: f64,

    /// Symbol-level aggregates for structural queries (0.0 for semantic-only).
    #[serde(default)]
    pub symbol_recall_at_5: f64,
    #[serde(default)]
    pub symbol_recall_at_10: f64,
    #[serde(default)]
    pub symbol_precision_at_5: f64,
}

/// Top-level corpus configuration file (benchmarks/corpus.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusFile {
    #[serde(flatten)]
    pub corpora: HashMap<String, CorpusEntry>,
}

/// Entry in corpus.toml — either a single repo or a list of repos.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CorpusEntry {
    /// Single repository.
    Single(CorpusRepo),

    /// Multiple repositories (for mini-corpus).
    Multi { repos: Vec<CorpusRepo> },
}

/// Repository definition in corpus.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusRepo {
    /// Git URL or local path.
    pub url: String,

    /// Sparse checkout paths (for large repos).
    #[serde(default)]
    pub sparse_paths: Vec<String>,

    /// File extensions to include.
    #[serde(default)]
    pub file_extensions: Vec<String>,
}

/// Validate a structural query has required fields.
///
/// `expected_symbols` may be empty when the query legitimately expects no results
/// (e.g., "who calls sign" when no internal callers exist in the corpus).
/// In that case, symbol metrics will correctly compute as 0.0.
pub fn validate_structural(query: &Query) -> Result<(), String> {
    if query.kind != QueryKind::Structural {
        return Ok(());
    }
    if query.target_symbol.is_none() {
        return Err("Structural query must have target_symbol".to_string());
    }
    if query.target_tool.is_none() {
        return Err("Structural query must have target_tool".to_string());
    }
    Ok(())
}

// ─── Phase 3: Store evaluation schema ──────────────────────────────────

/// Metrics report for a single backend in the store evaluation harness.
///
/// Captures the 4 axes per the spec (R2): indexing wall-clock, peak RSS,
/// on-disk size, query latency p50/p95.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreMetricsReport {
    /// Backend name (e.g., "sqlite-vec", "lancedb").
    pub backend: String,

    /// Corpus name (e.g., "vscode", "mini").
    pub corpus: String,

    /// Indexing wall-clock duration in seconds.
    pub indexing_secs: f64,

    /// Number of files indexed.
    pub files_indexed: usize,

    /// Number of chunks created during indexing.
    pub chunks_created: usize,

    /// Peak resident set size during indexing, in bytes (normalized).
    pub peak_rss_bytes: u64,

    /// On-disk size of the index in bytes.
    pub disk_size_bytes: u64,

    /// Query latency p50 in milliseconds.
    pub query_p50_ms: f64,

    /// Query latency p95 in milliseconds.
    pub query_p95_ms: f64,

    /// Number of queries executed for the latency sample.
    pub query_sample_size: usize,

    /// Whether the SLO (indexing wall-clock ≤ 360s on vscode) was met.
    pub slo_passed: bool,

    /// The SLO limit applied (seconds). 360 by default for the vscode corpus.
    pub slo_limit_secs: u32,
}

/// Verdict comparing two backend reports against the spec's thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// All 4 axes pass: migrate to the candidate backend.
    Migrate,

    /// At least one axis fails: stay with the incumbent. Reasons is non-empty
    /// and lists the failing axes with measured values.
    Stay { reasons: Vec<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── StoreMetricsReport serialization ──────────────────────────────

    #[test]
    fn store_metrics_report_serializes_all_axes() {
        let report = StoreMetricsReport {
            backend: "sqlite-vec".to_string(),
            corpus: "vscode".to_string(),
            indexing_secs: 240.5,
            files_indexed: 15000,
            chunks_created: 200000,
            peak_rss_bytes: 512_000_000,
            disk_size_bytes: 1_000_000_000,
            query_p50_ms: 25.0,
            query_p95_ms: 80.0,
            query_sample_size: 100,
            slo_passed: true,
            slo_limit_secs: 360,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"backend\":\"sqlite-vec\""));
        assert!(json.contains("\"indexing_secs\":240.5"));
        assert!(json.contains("\"peak_rss_bytes\":512000000"));
        assert!(json.contains("\"query_p50_ms\":25.0"));
        assert!(json.contains("\"query_p95_ms\":80.0"));
        assert!(json.contains("\"slo_passed\":true"));
    }

    #[test]
    fn verdict_migrate_serializes_as_string() {
        let v = Verdict::Migrate;
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"Migrate\"");
    }

    #[test]
    fn verdict_stay_carries_reasons() {
        let v = Verdict::Stay {
            reasons: vec![
                "indexing too slow".to_string(),
                "disk too large".to_string(),
            ],
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains("Stay"));
        assert!(json.contains("indexing too slow"));
        assert!(json.contains("disk too large"));
    }

    // ─── Original schema tests (kept verbatim) ─────────────────────────

    #[test]
    fn test_query_serialization() {
        let query = Query {
            text: "error handling".to_string(),
            judgments: vec![
                RelevanceJudgment {
                    file: "src/error.rs".to_string(),
                    grade: 3,
                },
                RelevanceJudgment {
                    file: "src/lib.rs".to_string(),
                    grade: 1,
                },
            ],
            kind: QueryKind::Semantic,
            expected_symbols: vec![],
            target_symbol: None,
            target_tool: None,
        };

        let toml_str = toml::to_string(&query).unwrap();
        assert!(toml_str.contains("error handling"));
        assert!(toml_str.contains("src/error.rs"));
    }

    #[test]
    fn test_corpus_config_local() {
        let config = CorpusConfig {
            name: "test".to_string(),
            source: CorpusSource::Local {
                path: PathBuf::from("tests/fixtures/mini"),
            },
            file_extensions: vec![".rs".to_string(), ".ts".to_string()],
            sparse_paths: vec![],
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("local"));
        assert!(toml_str.contains("tests/fixtures/mini"));
    }

    #[test]
    fn test_corpus_config_git() {
        let config = CorpusConfig {
            name: "vscode".to_string(),
            source: CorpusSource::Git {
                url: "https://github.com/microsoft/vscode.git".to_string(),
            },
            file_extensions: vec![".ts".to_string()],
            sparse_paths: vec!["src/vs/editor".to_string()],
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("git"));
        assert!(toml_str.contains("vscode.git"));
    }

    #[test]
    fn test_benchmark_result_serialization() {
        let result = BenchmarkResult {
            corpus: "mini".to_string(),
            search_mode: "dense".to_string(),
            files_indexed: 25,
            chunks_created: 150,
            queries_executed: 10,
            query_results: vec![],
            aggregate: AggregateMetrics {
                recall_at_5: 0.65,
                recall_at_10: 0.80,
                ndcg_at_10: 0.72,
                mrr: 0.55,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            duration_secs: 12.5,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("mini"));
        assert!(json.contains("0.65"));
        assert!(json.contains("dense"));
    }

    #[test]
    fn kind_defaults_semantic() {
        let toml_str = r#"
            text = "test query"
            judgments = []
        "#;
        let query: Query = toml::from_str(toml_str).unwrap();
        assert_eq!(query.kind, QueryKind::Semantic);
    }

    #[test]
    fn structural_parses_expected_symbols() {
        let toml_str = r#"
            text = "who calls search"
            kind = "structural"
            target_symbol = "search"
            target_tool = "callers"
            judgments = []
            expected_symbols = [{ symbol = "main", file = "main.rs", grade = 3 }]
        "#;
        let query: Query = toml::from_str(toml_str).unwrap();
        assert_eq!(query.kind, QueryKind::Structural);
        assert_eq!(query.target_symbol.as_deref(), Some("search"));
        assert_eq!(query.target_tool.as_deref(), Some("callers"));
        assert_eq!(query.expected_symbols.len(), 1);
        assert_eq!(query.expected_symbols[0].symbol, "main");
    }

    #[test]
    fn structural_without_expected_symbols_errors() {
        let toml_str = r#"
            text = "who calls search"
            kind = "structural"
            judgments = []
        "#;
        let query: Query = toml::from_str(toml_str).unwrap();
        assert_eq!(query.kind, QueryKind::Structural);
        assert!(query.expected_symbols.is_empty());
        // Validation should catch this
        assert!(validate_structural(&query).is_err());
    }
}
