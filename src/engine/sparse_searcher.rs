//! Sparse lexical search engine using FTS5 bm25 ranking.
//!
//! Delegates to `store::fts::search_sparse` for FTS5 MATCH queries.
//! Implements `SearchStrategy` with `mode() -> SearchMode::Sparse`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::schema::SearchConfig;
use crate::store::db::Database;
use crate::store::fts;
use crate::types::SearchResult;

use super::{SearchMode, SearchOptions, SearchStrategy};

/// Sparse lexical search engine using FTS5 bm25 ranking.
///
/// Uses full-text search (FTS5) for keyword-based search over indexed chunks.
/// No embedder needed — operates directly on tokenized text.
pub struct SparseSearcher {
    db: Arc<tokio::sync::Mutex<Database>>,
    #[allow(dead_code)] // Stored for future config-driven defaults
    config: SearchConfig,
}

impl SparseSearcher {
    /// Create a new SparseSearcher with the given database and config.
    pub fn new(db: Arc<tokio::sync::Mutex<Database>>, config: SearchConfig) -> Self {
        Self { db, config }
    }
}

#[async_trait]
impl SearchStrategy for SparseSearcher {
    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<SearchResult>> {
        let db = self.db.lock().await;
        let results = fts::search_sparse(
            db.conn(),
            query,
            options.limit,
            options.language.as_deref(),
            options.path.as_deref(),
        )?;
        Ok(results)
    }

    fn mode(&self) -> SearchMode {
        SearchMode::Sparse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::SearchConfig;
    use crate::store::chunks;
    use crate::store::db::Database;
    use crate::types::{compute_chunk_id, compute_content_hash, Chunk};

    fn setup_test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(64).unwrap();
        db
    }

    fn setup_sparse_searcher() -> SparseSearcher {
        let db = setup_test_db();
        let config = SearchConfig::default();
        SparseSearcher::new(Arc::new(tokio::sync::Mutex::new(db)), config)
    }

    fn insert_test_chunk(
        db: &Database,
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
    }

    // ─── SparseSearcher mode tests ─────────────────────────────────────

    #[test]
    fn sparse_searcher_mode_returns_sparse() {
        let searcher = setup_sparse_searcher();
        assert_eq!(searcher.mode(), SearchMode::Sparse);
    }

    #[test]
    fn sparse_searcher_implements_search_strategy() {
        let searcher = setup_sparse_searcher();
        let strategy: &dyn SearchStrategy = &searcher;
        assert_eq!(strategy.mode(), SearchMode::Sparse);
    }

    // ─── SparseSearcher search tests ───────────────────────────────────

    #[tokio::test]
    async fn sparse_searcher_empty_db_returns_empty() {
        let searcher = setup_sparse_searcher();
        let options = SearchOptions::default();
        let results = searcher.search("test query", options).await.unwrap();
        assert!(results.is_empty(), "Empty DB should return no results");
    }

    #[tokio::test]
    async fn sparse_searcher_returns_results_for_known_symbol() {
        let db = setup_test_db();
        insert_test_chunk(
            &db,
            "src/auth.ts",
            "function authenticateUser(username: string): boolean { return true; }",
            "typescript",
            Some("authenticateUser"),
        );

        let searcher = SparseSearcher::new(
            Arc::new(tokio::sync::Mutex::new(db)),
            SearchConfig::default(),
        );

        let options = SearchOptions::default();
        let results = searcher.search("authenticateUser", options).await.unwrap();

        assert_eq!(results.len(), 1, "Should find the chunk by symbol");
        assert_eq!(results[0].file_path, "src/auth.ts");
        assert_eq!(results[0].symbol.as_deref(), Some("authenticateUser"));
        assert!(
            results[0].score >= 0.0 && results[0].score < 1.0,
            "Score should be normalized to [0,1), got {}",
            results[0].score
        );
    }

    #[tokio::test]
    async fn sparse_searcher_returns_results_for_content_match() {
        let db = setup_test_db();
        insert_test_chunk(
            &db,
            "src/pay.ts",
            "function handlePayment() { processCharge(); }",
            "typescript",
            Some("handlePayment"),
        );

        let searcher = SparseSearcher::new(
            Arc::new(tokio::sync::Mutex::new(db)),
            SearchConfig::default(),
        );

        let options = SearchOptions::default();
        let results = searcher.search("processCharge", options).await.unwrap();

        assert_eq!(results.len(), 1, "Should find chunk by content keyword");
        assert_eq!(
            results[0].content,
            "function handlePayment() { processCharge(); }"
        );
    }

    #[tokio::test]
    async fn sparse_searcher_filters_by_language() {
        let db = setup_test_db();
        insert_test_chunk(
            &db,
            "src/a.ts",
            "function handler() {}",
            "typescript",
            Some("handler"),
        );
        insert_test_chunk(
            &db,
            "src/b.py",
            "def handler(): pass",
            "python",
            Some("handler"),
        );

        let searcher = SparseSearcher::new(
            Arc::new(tokio::sync::Mutex::new(db)),
            SearchConfig::default(),
        );

        let options = SearchOptions {
            language: Some("python".to_string()),
            ..Default::default()
        };
        let results = searcher.search("handler", options).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].language, "python");
    }

    #[tokio::test]
    async fn sparse_searcher_filters_by_path() {
        let db = setup_test_db();
        insert_test_chunk(
            &db,
            "src/auth/login.ts",
            "function login() {}",
            "typescript",
            Some("login"),
        );
        insert_test_chunk(
            &db,
            "src/pay/charge.ts",
            "function charge() {}",
            "typescript",
            Some("charge"),
        );

        let searcher = SparseSearcher::new(
            Arc::new(tokio::sync::Mutex::new(db)),
            SearchConfig::default(),
        );

        let options = SearchOptions {
            path: Some("src/auth".to_string()),
            ..Default::default()
        };
        let results = searcher.search("function", options).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "src/auth/login.ts");
    }

    #[tokio::test]
    async fn sparse_searcher_respects_limit() {
        let db = setup_test_db();
        for i in 0..5 {
            insert_test_chunk(
                &db,
                &format!("src/file_{i}.ts"),
                &format!("function handler_{i}() {{ /* handler number {i} */ }}"),
                "typescript",
                Some("handler"),
            );
        }

        let searcher = SparseSearcher::new(
            Arc::new(tokio::sync::Mutex::new(db)),
            SearchConfig::default(),
        );

        let options = SearchOptions {
            limit: 2,
            ..Default::default()
        };
        let results = searcher.search("handler", options).await.unwrap();

        assert_eq!(results.len(), 2, "Should respect limit of 2");
    }

    #[tokio::test]
    async fn sparse_searcher_sanitizes_special_chars() {
        let db = setup_test_db();
        insert_test_chunk(
            &db,
            "src/auth.ts",
            "function authenticateUser() {}",
            "typescript",
            Some("authenticateUser"),
        );

        let searcher = SparseSearcher::new(
            Arc::new(tokio::sync::Mutex::new(db)),
            SearchConfig::default(),
        );

        // This would crash FTS5 without sanitization
        let options = SearchOptions::default();
        let results = searcher.search("authenticateUser*", options).await.unwrap();

        assert_eq!(results.len(), 1, "Should sanitize * and still find results");
    }

    #[tokio::test]
    async fn sparse_searcher_empty_query_returns_empty() {
        let db = setup_test_db();
        insert_test_chunk(
            &db,
            "src/auth.ts",
            "function authenticateUser() {}",
            "typescript",
            Some("authenticateUser"),
        );

        let searcher = SparseSearcher::new(
            Arc::new(tokio::sync::Mutex::new(db)),
            SearchConfig::default(),
        );

        let options = SearchOptions::default();
        let results = searcher.search("", options).await.unwrap();
        assert!(results.is_empty(), "Empty query should return no results");
    }

    #[tokio::test]
    async fn sparse_searcher_ranked_by_bm25() {
        let db = setup_test_db();
        // Insert two chunks — one with symbol match (higher bm25 weight),
        // one with only content match
        insert_test_chunk(
            &db,
            "src/a.ts",
            "function authenticateUser() { /* symbol match */ }",
            "typescript",
            Some("authenticateUser"),
        );
        insert_test_chunk(
            &db,
            "src/b.ts",
            "function helper() { authenticateUser(); /* content only */ }",
            "typescript",
            Some("helper"),
        );

        let searcher = SparseSearcher::new(
            Arc::new(tokio::sync::Mutex::new(db)),
            SearchConfig::default(),
        );

        let options = SearchOptions {
            threshold: 0.0,
            ..Default::default()
        };
        let results = searcher.search("authenticateUser", options).await.unwrap();

        assert_eq!(results.len(), 2, "Should find both chunks");
        // Symbol match should rank higher (bm25 weight 10 vs 5 for content)
        assert_eq!(
            results[0].symbol.as_deref(),
            Some("authenticateUser"),
            "Symbol match should rank first"
        );
    }
}
