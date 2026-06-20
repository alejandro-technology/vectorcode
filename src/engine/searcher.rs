//! Search pipeline — semantic search over indexed code chunks (spec §10).
//!
//! Embeds natural language queries and performs cosine similarity search
//! over stored chunk vectors, with optional language and path filtering.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::schema::SearchConfig;
use crate::embedder::Embedder;
use crate::store::db::Database;
use crate::store::vectors;
use crate::types::SearchResult;

use super::fusion::HybridSearcher;
use super::sparse_searcher::SparseSearcher;
use crate::reranker::Reranker;

/// Search mode — determines which search strategy to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum SearchMode {
    /// Dense semantic search using vector embeddings (default).
    #[default]
    Dense,
    /// Sparse lexical search using FTS5 bm25 ranking.
    Sparse,
    /// Hybrid search combining dense + sparse via RRF fusion.
    Hybrid,
    /// Hybrid search with cross-encoder reranking of top candidates.
    HybridRerank,
    /// Graph-based structural search using the knowledge graph.
    Graph,
}

impl std::fmt::Display for SearchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dense => write!(f, "dense"),
            Self::Sparse => write!(f, "sparse"),
            Self::Hybrid => write!(f, "hybrid"),
            Self::HybridRerank => write!(f, "hybrid-rerank"),
            Self::Graph => write!(f, "graph"),
        }
    }
}

impl std::str::FromStr for SearchMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "dense" => Ok(Self::Dense),
            "sparse" => Ok(Self::Sparse),
            "hybrid" => Ok(Self::Hybrid),
            "hybrid-rerank" => Ok(Self::HybridRerank),
            "graph" => Ok(Self::Graph),
            other => Err(format!(
                "unknown search mode '{other}', expected: dense, sparse, hybrid, hybrid-rerank, graph"
            )),
        }
    }
}

/// Strategy trait for search implementations (spec §10).
///
/// Abstracts over Dense, Sparse, and Hybrid search modes.
/// Each implementor provides its own `search` logic and reports its mode.
#[async_trait]
pub trait SearchStrategy: Send + Sync {
    /// Execute a search query with the given options.
    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<SearchResult>>;

    /// Return the search mode this strategy implements.
    fn mode(&self) -> SearchMode;
}

/// Options for a semantic search query (spec §10.2).
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Maximum number of results to return.
    pub limit: usize,
    /// Minimum similarity score (0.0–1.0). Results below this are filtered.
    pub threshold: f32,
    /// Filter by programming language (e.g., "typescript").
    pub language: Option<String>,
    /// Filter by file path prefix (e.g., "src/auth/").
    pub path: Option<String>,
    /// Search mode: Dense, Sparse, or Hybrid.
    pub mode: SearchMode,
    /// RRF fusion constant (default: 60).
    pub rrf_k: u32,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 10,
            threshold: 0.3,
            language: None,
            path: None,
            mode: SearchMode::Dense,
            rrf_k: 60,
        }
    }
}

/// Dense semantic search engine over indexed code chunks (spec §10).
///
/// Uses vector embeddings and cosine similarity for semantic search.
/// Implements `SearchStrategy` with `mode() -> SearchMode::Dense`.
pub struct DenseSearcher {
    db: Arc<tokio::sync::Mutex<Database>>,
    embedder: Arc<dyn Embedder>,
    config: SearchConfig,
}

/// Backward-compatible type alias — all existing callers use `Searcher`.
pub type Searcher = DenseSearcher;

impl DenseSearcher {
    /// Create a new DenseSearcher with the given database, embedder, and config.
    pub fn new(
        db: Arc<tokio::sync::Mutex<Database>>,
        embedder: Arc<dyn Embedder>,
        config: SearchConfig,
    ) -> Self {
        Self {
            db,
            embedder,
            config,
        }
    }

    /// Create default search options from the searcher's config.
    pub fn default_search_options(&self) -> SearchOptions {
        SearchOptions {
            limit: self.config.default_limit,
            threshold: self.config.default_threshold,
            language: None,
            path: None,
            mode: self.config.search_mode(),
            rrf_k: self.config.rrf_k,
        }
    }

    /// Execute a semantic search query (spec §10.1).
    ///
    /// 1. Enriches short queries (< 3 words) with "code that" prefix
    /// 2. Embeds the query using the configured embedder
    /// 3. Performs vector similarity search
    /// 4. Applies language and path filters
    /// 5. Returns ranked results above threshold
    pub async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<SearchResult>> {
        // Step 1: Enrich query for better embedding
        let enriched = enrich_query(query);

        // Step 2: Embed query using same provider/model as index
        let query_vec = self.embedder.embed(&enriched).await?;

        // Step 3: Vector similarity search
        // Request extra results when post-filtering is needed
        let fetch_limit = if options.language.is_some() || options.path.is_some() {
            options.limit.saturating_mul(5)
        } else {
            options.limit
        };
        let fetch_limit = fetch_limit.max(50);
        let threshold = options.threshold;

        // Build LIKE pattern for SQL-level path filtering
        let path_like_pattern = options
            .path
            .as_deref()
            .map(|p| format!("{}%", vectors::escape_like_pattern(p)));

        let mut results = {
            let db = self.db.lock().await;
            vectors::search_similar(
                db.conn(),
                &query_vec,
                fetch_limit,
                threshold,
                path_like_pattern.as_deref(),
            )?
        };

        // Step 4: Filter by language
        if let Some(lang) = &options.language {
            results.retain(|r| r.language == *lang);
        }

        // Step 5: Apply final limit
        results.truncate(options.limit);

        Ok(results)
    }
}

#[async_trait]
impl SearchStrategy for DenseSearcher {
    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<SearchResult>> {
        // Delegate to the inherent method (same logic, zero behavior change)
        self.search(query, options).await
    }

    fn mode(&self) -> SearchMode {
        SearchMode::Dense
    }
}

/// Build the appropriate search strategy based on the given mode.
///
/// Factory function that creates the right `SearchStrategy` implementation
/// for the requested mode. Used by CLI/MCP to dispatch search requests.
///
/// For `HybridRerank`, the reranker is loaded asynchronously (lazy, with timeout).
/// If the reranker fails to load, falls back to plain hybrid search.
pub async fn build_strategy(
    mode: SearchMode,
    db: Arc<tokio::sync::Mutex<Database>>,
    embedder: Arc<dyn Embedder>,
    config: SearchConfig,
) -> Arc<dyn SearchStrategy> {
    match mode {
        SearchMode::Dense => Arc::new(DenseSearcher::new(db, embedder, config)),
        SearchMode::Sparse => Arc::new(SparseSearcher::new(db, config)),
        SearchMode::Hybrid => {
            let dense: Arc<dyn SearchStrategy> =
                Arc::new(DenseSearcher::new(db.clone(), embedder, config.clone()));
            let sparse: Arc<dyn SearchStrategy> = Arc::new(SparseSearcher::new(db, config.clone()));
            Arc::new(HybridSearcher::new(dense, sparse, config.rrf_k))
        }
        SearchMode::HybridRerank => {
            let reranker: Option<Arc<dyn Reranker>> = if config.rerank.enabled {
                match crate::reranker::onnx::OnnxReranker::from_cache_with_timeout().await {
                    Ok(r) => Some(Arc::new(r)),
                    Err(e) => {
                        tracing::warn!(
                            "Reranker failed to load: {e}. Falling back to hybrid search."
                        );
                        None
                    }
                }
            } else {
                None
            };

            let dense: Arc<dyn SearchStrategy> =
                Arc::new(DenseSearcher::new(db.clone(), embedder, config.clone()));
            let sparse: Arc<dyn SearchStrategy> = Arc::new(SparseSearcher::new(db, config.clone()));
            let timeout = std::time::Duration::from_millis(config.rerank.timeout_ms);
            Arc::new(HybridSearcher::with_reranker(
                dense,
                sparse,
                config.rrf_k,
                reranker,
                config.rerank.top_k,
                timeout,
            ))
        }
        SearchMode::Graph => Arc::new(super::graph_retriever::GraphRetriever::new(db)),
    }
}

/// Enrich a short query for better embedding (spec §10.1 step 2).
///
/// If the query has fewer than 3 words, prepend "code that" to provide
/// context that this is a code search query. This helps embedding models
/// produce vectors more aligned with code semantics.
fn enrich_query(query: &str) -> String {
    let word_count = query.split_whitespace().count();
    if word_count < 3 {
        format!("code that {query}")
    } else {
        query.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::SearchConfig;
    use crate::embedder::mock::MockEmbedder;
    use crate::store::db::Database;
    use crate::store::{chunks, vectors};
    use crate::types::{compute_chunk_id, compute_content_hash, Chunk};

    fn setup_test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(64).unwrap();
        db
    }

    fn setup_searcher() -> Searcher {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));
        let config = SearchConfig::default();
        Searcher::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            embedder,
            config,
        )
    }

    /// Insert a chunk and its vector into the database.
    async fn insert_test_chunk(
        db: &Database,
        embedder: &MockEmbedder,
        file_path: &str,
        content: &str,
        language: &str,
        symbol: Option<&str>,
    ) {
        let byte_start = 0u32;
        let byte_end = content.len() as u32;
        let chunk = Chunk {
            id: compute_chunk_id(file_path, byte_start, byte_end),
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: 10,
            byte_start,
            byte_end,
            symbol: symbol.map(|s| s.to_string()),
            kind: "function_declaration".to_string(),
            content: content.to_string(),
            parent_context: None,
            language: language.to_string(),
            file_mtime: 1718000000,
            content_hash: compute_content_hash(content),
        };
        chunks::insert_chunk(db.conn(), &chunk).unwrap();

        let embedding = embedder.embed(content).await.unwrap();
        vectors::insert_vector(db.conn(), &chunk.id, &embedding).unwrap();
    }

    // ─── enrich_query tests ────────────────────────────────────────────

    #[test]
    fn enrich_query_single_word_prepends_prefix() {
        let enriched = enrich_query("authentication");
        assert_eq!(enriched, "code that authentication");
    }

    #[test]
    fn enrich_query_two_words_prepends_prefix() {
        let enriched = enrich_query("payment retry");
        assert_eq!(enriched, "code that payment retry");
    }

    #[test]
    fn enrich_query_three_words_no_prefix() {
        let enriched = enrich_query("payment retry logic");
        assert_eq!(enriched, "payment retry logic");
    }

    #[test]
    fn enrich_query_many_words_no_prefix() {
        let enriched = enrich_query("how does the payment retry logic work");
        assert_eq!(enriched, "how does the payment retry logic work");
    }

    #[test]
    fn enrich_query_empty_string_prepends_prefix() {
        let enriched = enrich_query("");
        assert_eq!(enriched, "code that ");
    }

    // ─── SearchMode tests ──────────────────────────────────────────────

    #[test]
    fn search_mode_default_is_dense() {
        let mode = SearchMode::default();
        assert_eq!(mode, SearchMode::Dense);
    }

    #[test]
    fn search_mode_from_str_dense() {
        assert_eq!("dense".parse::<SearchMode>().unwrap(), SearchMode::Dense);
    }

    #[test]
    fn search_mode_from_str_sparse() {
        assert_eq!("sparse".parse::<SearchMode>().unwrap(), SearchMode::Sparse);
    }

    #[test]
    fn search_mode_from_str_hybrid() {
        assert_eq!("hybrid".parse::<SearchMode>().unwrap(), SearchMode::Hybrid);
    }

    #[test]
    fn search_mode_from_str_hybrid_rerank() {
        assert_eq!(
            "hybrid-rerank".parse::<SearchMode>().unwrap(),
            SearchMode::HybridRerank
        );
    }

    #[test]
    fn search_mode_from_str_graph() {
        assert_eq!("graph".parse::<SearchMode>().unwrap(), SearchMode::Graph);
    }

    #[test]
    fn search_mode_from_str_invalid_returns_error() {
        assert!("invalid".parse::<SearchMode>().is_err());
    }

    #[test]
    fn search_mode_from_str_case_sensitive() {
        assert!("Dense".parse::<SearchMode>().is_err());
        assert!("HYBRID".parse::<SearchMode>().is_err());
        assert!("Hybrid-Rerank".parse::<SearchMode>().is_err());
    }

    // ─── SearchStrategy trait tests ────────────────────────────────────

    #[test]
    fn dense_searcher_mode_returns_dense() {
        let searcher = setup_searcher();
        assert_eq!(searcher.mode(), SearchMode::Dense);
    }

    #[tokio::test]
    async fn dense_searcher_implements_search_strategy() {
        let searcher = setup_searcher();
        // Verify we can call search through the trait
        let strategy: &dyn SearchStrategy = &searcher;
        let results = strategy
            .search("test query", SearchOptions::default())
            .await
            .unwrap();
        assert!(results.is_empty(), "Empty DB should return no results");
    }

    // ─── build_strategy tests ──────────────────────────────────────────

    #[tokio::test]
    async fn build_strategy_dense_returns_dense_searcher() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));
        let config = SearchConfig::default();
        let strategy = build_strategy(
            SearchMode::Dense,
            Arc::new(tokio::sync::Mutex::new(db)),
            embedder,
            config,
        )
        .await;
        assert_eq!(strategy.mode(), SearchMode::Dense);
    }

    #[tokio::test]
    async fn build_strategy_sparse_returns_sparse_searcher() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));
        let config = SearchConfig::default();
        let strategy = build_strategy(
            SearchMode::Sparse,
            Arc::new(tokio::sync::Mutex::new(db)),
            embedder,
            config,
        )
        .await;
        assert_eq!(strategy.mode(), SearchMode::Sparse);
    }

    #[tokio::test]
    async fn build_strategy_hybrid_returns_hybrid_searcher() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));
        let config = SearchConfig::default();
        let strategy = build_strategy(
            SearchMode::Hybrid,
            Arc::new(tokio::sync::Mutex::new(db)),
            embedder,
            config,
        )
        .await;
        assert_eq!(strategy.mode(), SearchMode::Hybrid);
    }

    #[tokio::test]
    async fn build_strategy_hybrid_rerank_returns_hybrid_searcher() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));
        // rerank.enabled = false → no reranker loaded, but mode is still HybridRerank
        let config = SearchConfig::default();
        let strategy = build_strategy(
            SearchMode::HybridRerank,
            Arc::new(tokio::sync::Mutex::new(db)),
            embedder,
            config,
        )
        .await;
        assert_eq!(strategy.mode(), SearchMode::HybridRerank);
    }

    #[tokio::test]
    async fn build_strategy_graph_returns_graph_retriever() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));
        let config = SearchConfig::default();
        let strategy = build_strategy(
            SearchMode::Graph,
            Arc::new(tokio::sync::Mutex::new(db)),
            embedder,
            config,
        )
        .await;
        assert_eq!(strategy.mode(), SearchMode::Graph);
    }

    // ─── SearchOptions tests ───────────────────────────────────────────

    #[test]
    fn default_search_options_has_expected_values() {
        let opts = SearchOptions::default();
        assert_eq!(opts.limit, 10);
        assert!((opts.threshold - 0.3).abs() < f32::EPSILON);
        assert!(opts.language.is_none());
        assert!(opts.path.is_none());
        assert_eq!(opts.mode, SearchMode::Dense);
        assert_eq!(opts.rrf_k, 60);
    }

    #[test]
    fn searcher_default_search_options_uses_config() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));
        let config = SearchConfig {
            default_limit: 25,
            default_threshold: 0.5,
            default_mode: "sparse".to_string(),
            rrf_k: 100,
            ..Default::default()
        };
        let searcher = Searcher::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            embedder,
            config,
        );
        let opts = searcher.default_search_options();
        assert_eq!(opts.limit, 25);
        assert!((opts.threshold - 0.5).abs() < f32::EPSILON);
        assert_eq!(opts.mode, SearchMode::Sparse);
        assert_eq!(opts.rrf_k, 100);
    }

    // ─── Searcher integration tests ────────────────────────────────────

    #[tokio::test]
    async fn search_returns_results_for_matching_query() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));

        // Insert test chunks with known content
        insert_test_chunk(
            &db,
            &embedder,
            "src/auth.ts",
            "function authenticateUser(username: string, password: string): boolean { /* auth logic */ }",
            "typescript",
            Some("authenticateUser"),
        )
        .await;

        let config = SearchConfig::default();
        let searcher = Searcher::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            embedder.clone(),
            config,
        );

        // Search with the exact same content — should find it (self-similarity = 1.0)
        let options = SearchOptions {
            threshold: 0.0, // Accept all results
            ..Default::default()
        };
        let results = searcher
            .search(
                "function authenticateUser(username: string, password: string): boolean { /* auth logic */ }",
                options,
            )
            .await
            .unwrap();

        assert!(
            !results.is_empty(),
            "Should find at least one result for exact content match"
        );
        assert_eq!(results[0].file_path, "src/auth.ts");
        assert!(
            results[0].score > 0.9,
            "Self-similarity should be ~1.0, got {}",
            results[0].score
        );
    }

    #[tokio::test]
    async fn search_empty_db_returns_empty() {
        let searcher = setup_searcher();
        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();
        assert!(results.is_empty(), "Empty DB should return no results");
    }

    #[tokio::test]
    async fn search_filters_by_language() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));

        insert_test_chunk(
            &db,
            &embedder,
            "src/app.ts",
            "function typescriptFunction(): void { console.log('ts'); }",
            "typescript",
            Some("typescriptFunction"),
        )
        .await;
        insert_test_chunk(
            &db,
            &embedder,
            "src/main.py",
            "def python_function(): print('py')",
            "python",
            Some("python_function"),
        )
        .await;

        let config = SearchConfig::default();
        let searcher = Searcher::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            embedder.clone(),
            config,
        );

        // Search with language filter for TypeScript only
        let options = SearchOptions {
            language: Some("typescript".to_string()),
            threshold: 0.0,
            ..Default::default()
        };

        // Search with the TS content to ensure we get results
        let results = searcher
            .search(
                "function typescriptFunction(): void { console.log('ts'); }",
                options,
            )
            .await
            .unwrap();

        for result in &results {
            assert_eq!(
                result.language, "typescript",
                "All results should be TypeScript, got: {}",
                result.language
            );
        }
    }

    #[tokio::test]
    async fn search_filters_by_path_prefix() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));

        insert_test_chunk(
            &db,
            &embedder,
            "src/auth/login.ts",
            "function handleLogin(credentials: LoginCredentials): Promise<Session> { /* login */ }",
            "typescript",
            Some("handleLogin"),
        )
        .await;
        insert_test_chunk(
            &db,
            &embedder,
            "src/payment/charge.ts",
            "function processCharge(amount: number): Promise<Receipt> { /* charge */ }",
            "typescript",
            Some("processCharge"),
        )
        .await;

        let config = SearchConfig::default();
        let searcher = Searcher::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            embedder.clone(),
            config,
        );

        let options = SearchOptions {
            path: Some("src/auth/".to_string()),
            threshold: 0.0,
            ..Default::default()
        };

        let results = searcher
            .search(
                "function handleLogin(credentials: LoginCredentials): Promise<Session> { /* login */ }",
                options,
            )
            .await
            .unwrap();

        for result in &results {
            assert!(
                result.file_path.starts_with("src/auth/"),
                "All results should be under src/auth/, got: {}",
                result.file_path
            );
        }
    }

    #[tokio::test]
    async fn search_respects_limit() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));

        // Insert multiple chunks
        for i in 0..5 {
            let content = format!(
                "function handler_{}(request: Request): Response {{ /* handler number {} with padding */ }}",
                i, i
            );
            insert_test_chunk(
                &db,
                &embedder,
                &format!("src/handler_{}.ts", i),
                &content,
                "typescript",
                Some(&format!("handler_{}", i)),
            )
            .await;
        }

        let config = SearchConfig::default();
        let searcher = Searcher::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            embedder.clone(),
            config,
        );

        let options = SearchOptions {
            limit: 2,
            threshold: 0.0,
            ..Default::default()
        };

        let results = searcher
            .search(
                "function handler_0(request: Request): Response { /* handler number 0 with padding */ }",
                options,
            )
            .await
            .unwrap();

        assert!(
            results.len() <= 2,
            "Should return at most 2 results, got {}",
            results.len()
        );
    }

    #[tokio::test]
    async fn search_threshold_filters_low_scores() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));

        insert_test_chunk(
            &db,
            &embedder,
            "src/app.ts",
            "function specificFunction(): void { console.log('specific'); }",
            "typescript",
            Some("specificFunction"),
        )
        .await;

        let config = SearchConfig::default();
        let searcher = Searcher::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            embedder.clone(),
            config,
        );

        // Use a very high threshold that should filter everything
        let options = SearchOptions {
            threshold: 0.999,
            ..Default::default()
        };

        // Search with a completely different query
        let results = searcher
            .search(
                "something completely different and unrelated to the code",
                options,
            )
            .await
            .unwrap();

        // With threshold 0.999, only near-identical vectors should pass
        // The different query should produce a different vector
        // (unless the mock hash happens to be similar, which is unlikely)
        // We mainly verify the threshold filtering doesn't crash
        assert!(
            results.is_empty() || results[0].score >= 0.999,
            "All results should have score >= 0.999"
        );
    }

    #[tokio::test]
    async fn search_results_are_sorted_by_score_descending() {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));

        // Insert several chunks
        for i in 0..3 {
            let content = format!(
                "export function method_{}(x: number): number {{ return x * {}; /* padding text to make it longer */ }}",
                i, i + 1
            );
            insert_test_chunk(
                &db,
                &embedder,
                &format!("src/math_{}.ts", i),
                &content,
                "typescript",
                Some(&format!("method_{}", i)),
            )
            .await;
        }

        let config = SearchConfig::default();
        let searcher = Searcher::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            embedder.clone(),
            config,
        );

        let options = SearchOptions {
            threshold: 0.0,
            ..Default::default()
        };

        let results = searcher
            .search(
                "export function method_0(x: number): number { return x * 1; /* padding text to make it longer */ }",
                options,
            )
            .await
            .unwrap();

        // Verify results are sorted by score descending
        for i in 1..results.len() {
            assert!(
                results[i - 1].score >= results[i].score,
                "Results should be sorted by score descending: {} < {} at index {}",
                results[i - 1].score,
                results[i].score,
                i
            );
        }
    }
}
