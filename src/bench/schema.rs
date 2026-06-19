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
}

/// Aggregate metrics across all queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateMetrics {
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub ndcg_at_10: f64,
    pub mrr: f64,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            duration_secs: 12.5,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("mini"));
        assert!(json.contains("0.65"));
        assert!(json.contains("dense"));
    }
}
