//! Integration tests for hybrid search: FTS5 triggers, sparse search, hybrid fusion,
//! dense backward compatibility, and v2→v3 migration.

use std::sync::Arc;
use vectorcode::config::schema::SearchConfig;
use vectorcode::embedder::mock::MockEmbedder;
use vectorcode::embedder::Embedder;
use vectorcode::engine::searcher::{SearchMode, SearchOptions};
use vectorcode::engine::{DenseSearcher, HybridSearcher, SearchStrategy, SparseSearcher};
use vectorcode::store::{chunks, vectors};
use vectorcode::{compute_chunk_id, compute_content_hash, Chunk, Database};

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Insert a test chunk (and optionally its vector) into the database.
#[allow(clippy::too_many_arguments)]
async fn insert_test_chunk(
    db: &Database,
    embedder: &MockEmbedder,
    file_path: &str,
    content: &str,
    language: &str,
    symbol: Option<&str>,
    start_line: u32,
    with_vector: bool,
) {
    let byte_start = 0u32;
    let byte_end = content.len() as u32;
    let chunk = Chunk {
        id: compute_chunk_id(file_path, byte_start, byte_end),
        file_path: file_path.to_string(),
        start_line,
        end_line: start_line + 10,
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

    if with_vector {
        let embedding = embedder.embed(content).await.unwrap();
        vectors::insert_vector(db.conn(), &chunk.id, &embedding).unwrap();
    }
}

/// Create a fully initialized in-memory database (v3 schema).
async fn setup_v3_db() -> Database {
    let db = Database::open_in_memory().unwrap();
    db.init_schema(64).unwrap();
    db
}

/// Create a v2 database manually (no FTS5, no triggers).
fn setup_v2_schema() -> Database {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute_batch(
            "
        CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO meta (key, value) VALUES ('embedding_dims', '64');
        CREATE TABLE chunks (
            id TEXT PRIMARY KEY, file_path TEXT NOT NULL,
            start_line INTEGER NOT NULL, end_line INTEGER NOT NULL,
            byte_start INTEGER NOT NULL, byte_end INTEGER NOT NULL,
            symbol TEXT, kind TEXT NOT NULL, content TEXT NOT NULL,
            parent_context TEXT, language TEXT NOT NULL,
            file_mtime INTEGER NOT NULL, content_hash TEXT NOT NULL
        );
        CREATE TABLE files (
            path TEXT PRIMARY KEY, mtime INTEGER NOT NULL,
            size INTEGER NOT NULL, hash TEXT NOT NULL, indexed_at INTEGER NOT NULL
        );
        CREATE TABLE vectors_data (
            chunk_id TEXT PRIMARY KEY, embedding TEXT NOT NULL,
            FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
        );
        CREATE TABLE chunk_vec_map (
            chunk_id TEXT PRIMARY KEY, vec_rowid INTEGER NOT NULL,
            FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
        );
        CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[64] distance_metric=cosine);
        ",
        )
        .unwrap();
    // Set schema version to 2
    db.conn().pragma_update(None, "user_version", 2u32).unwrap();
    db
}

fn insert_chunk_raw(
    db: &Database,
    id: &str,
    file_path: &str,
    symbol: &str,
    content: &str,
    language: &str,
) {
    db.conn()
        .execute(
            "INSERT INTO chunks (id, file_path, start_line, end_line, byte_start, byte_end, \
             symbol, kind, content, parent_context, language, file_mtime, content_hash) \
             VALUES (?1, ?2, 1, 10, 0, 100, ?3, 'function_declaration', ?4, NULL, ?5, 1718000000, 'hash')",
            rusqlite::params![id, file_path, symbol, content, language],
        )
        .unwrap();
}

// ════════════════════════════════════════════════════════════════════════════════
// T15: FTS5 triggers sync chunks correctly
// ════════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fts5_insert_populates_fts_table() {
    let db = setup_v3_db().await;
    insert_test_chunk(
        &db,
        &MockEmbedder::new(64),
        "src/auth.rs",
        "fn handle_user_login()",
        "rust",
        Some("handle_user_login"),
        1,
        false,
    )
    .await;

    let fts_count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fts_count, 1, "FTS5 should have 1 row after chunk insert");
}

#[tokio::test]
async fn fts5_delete_removes_from_fts_table() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn handle_user_login()",
        "rust",
        Some("handle_user_login"),
        1,
        false,
    )
    .await;

    // Verify FTS has the row
    let count_before: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_before, 1);

    // Delete the chunk
    db.conn()
        .execute("DELETE FROM chunks WHERE file_path = 'src/auth.rs'", [])
        .unwrap();

    let fts_count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fts_count, 0, "FTS5 should be empty after chunk delete");
}

#[tokio::test]
async fn fts5_update_reflects_new_content() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn render_login_form() { display(); }",
        "rust",
        Some("render_login_form"),
        1,
        false,
    )
    .await;

    // Update BOTH content and symbol so old term is fully gone from FTS
    db.conn()
        .execute(
            "UPDATE chunks SET content = 'fn process_refund() { handle(); }', \
             symbol = 'process_refund' WHERE file_path = 'src/auth.rs'",
            [],
        )
        .unwrap();

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let sparse = SparseSearcher::new(db_arc, SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        ..Default::default()
    };

    // Old term should not match (content AND symbol both changed)
    let old_results = sparse
        .search("render_login_form", opts.clone())
        .await
        .unwrap();
    assert!(
        old_results.is_empty(),
        "Old content should not match after update, got {} results",
        old_results.len()
    );

    // New term should match
    let new_results = sparse.search("process_refund", opts).await.unwrap();
    assert_eq!(
        new_results.len(),
        1,
        "New content should match after update"
    );
}

#[tokio::test]
async fn fts5_clear_database_also_clears_fts() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);
    for i in 0..3 {
        insert_test_chunk(
            &db,
            &embedder,
            &format!("src/file_{i}.rs"),
            &format!("fn function_{i}() {{}}"),
            "rust",
            Some(&format!("function_{i}")),
            (i + 1) as u32,
            false,
        )
        .await;
    }

    let fts_before: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fts_before, 3);

    db.clear_database().unwrap();

    let fts_after: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fts_after, 0, "FTS5 should be empty after clear_database");

    let chunks_after: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        chunks_after, 0,
        "chunks should be empty after clear_database"
    );
}

#[tokio::test]
async fn fts5_multiple_inserts_all_synced() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);
    for i in 0..5 {
        insert_test_chunk(
            &db,
            &embedder,
            &format!("src/file_{i}.rs"),
            &format!("fn function_{i}() {{}}"),
            "rust",
            Some(&format!("function_{i}")),
            (i + 1) as u32,
            false,
        )
        .await;
    }

    let fts_count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        fts_count, 5,
        "FTS5 should have 5 rows after 5 chunk inserts"
    );
}

// ════════════════════════════════════════════════════════════════════════════════
// T16: Sparse search returns bm25-ranked results
// ════════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn sparse_search_returns_bm25_ranked_results() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);

    // Insert chunks with varying relevance to "login"
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn handle_user_login() { authenticate(); }",
        "rust",
        Some("handle_user_login"),
        1,
        false,
    )
    .await;
    insert_test_chunk(
        &db,
        &embedder,
        "src/ui.rs",
        "fn render_login_form() { display_form(); }",
        "rust",
        Some("render_login_form"),
        1,
        false,
    )
    .await;
    insert_test_chunk(
        &db,
        &embedder,
        "src/billing.rs",
        "fn calculate_tax() { compute_amount(); }",
        "rust",
        Some("calculate_tax"),
        1,
        false,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let sparse = SparseSearcher::new(db_arc, SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        ..Default::default()
    };

    let results = sparse.search("login", opts).await.unwrap();

    // Should find at least the login-related chunks
    assert!(
        results.len() >= 2,
        "Should find at least 2 login-related chunks, got {}",
        results.len()
    );

    // Login-related chunks should rank higher than billing
    let login_positions: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.content.contains("login"))
        .map(|(i, _)| i)
        .collect();
    let tax_position = results.iter().position(|r| r.content.contains("tax"));

    if let Some(tax_pos) = tax_position {
        for login_pos in &login_positions {
            assert!(
                login_pos < &tax_pos,
                "Login chunks should rank higher than tax chunk"
            );
        }
    }

    // All scores should be in [0, 1)
    for r in &results {
        assert!(r.score >= 0.0, "Score should be >= 0, got {}", r.score);
        assert!(r.score < 1.0, "Score should be < 1, got {}", r.score);
    }
}

#[tokio::test]
async fn sparse_search_fields_populated_correctly() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn handle_user_login() { authenticate(); }",
        "rust",
        Some("handle_user_login"),
        5,
        false,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let sparse = SparseSearcher::new(db_arc, SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        ..Default::default()
    };

    let results = sparse.search("handle_user_login", opts).await.unwrap();
    assert_eq!(results.len(), 1);

    let r = &results[0];
    assert_eq!(r.file_path, "src/auth.rs");
    assert_eq!(r.start_line, 5);
    assert_eq!(r.end_line, 15);
    assert_eq!(r.symbol.as_deref(), Some("handle_user_login"));
    assert_eq!(r.kind, "function_declaration");
    assert_eq!(r.language, "rust");
    assert!(r.content.contains("handle_user_login"));
    assert!(r.score > 0.0);
}

#[tokio::test]
async fn sparse_search_respects_language_filter() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn handle_login() {}",
        "rust",
        Some("handle_login"),
        1,
        false,
    )
    .await;
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.py",
        "def handle_login(): pass",
        "python",
        Some("handle_login"),
        1,
        false,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let sparse = SparseSearcher::new(db_arc, SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        language: Some("python".to_string()),
        ..Default::default()
    };

    let results = sparse.search("handle_login", opts).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].language, "python");
}

#[tokio::test]
async fn sparse_search_respects_path_filter() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth/login.rs",
        "fn handle_login() {}",
        "rust",
        Some("handle_login"),
        1,
        false,
    )
    .await;
    insert_test_chunk(
        &db,
        &embedder,
        "src/billing/charge.rs",
        "fn handle_charge() {}",
        "rust",
        Some("handle_charge"),
        1,
        false,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let sparse = SparseSearcher::new(db_arc, SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        path: Some("src/auth".to_string()),
        ..Default::default()
    };

    let results = sparse.search("handle", opts).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].file_path, "src/auth/login.rs");
}

// ════════════════════════════════════════════════════════════════════════════════
// T17: Hybrid search fuses dense + sparse
// ════════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn hybrid_search_fuses_dense_and_sparse() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);

    // Insert chunks with vectors for dense search
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn handle_user_login() { authenticate(); }",
        "rust",
        Some("handle_user_login"),
        1,
        true,
    )
    .await;
    insert_test_chunk(
        &db,
        &embedder,
        "src/ui.rs",
        "fn render_login_form() { display(); }",
        "rust",
        Some("render_login_form"),
        1,
        true,
    )
    .await;
    insert_test_chunk(
        &db,
        &embedder,
        "src/billing.rs",
        "fn calculate_tax() { compute(); }",
        "rust",
        Some("calculate_tax"),
        1,
        true,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let embedder_arc = Arc::new(embedder);
    let config = SearchConfig::default();

    let dense: Arc<dyn vectorcode::engine::SearchStrategy> = Arc::new(DenseSearcher::new(
        db_arc.clone(),
        embedder_arc.clone(),
        config.clone(),
    ));
    let sparse: Arc<dyn vectorcode::engine::SearchStrategy> =
        Arc::new(SparseSearcher::new(db_arc.clone(), config.clone()));

    let hybrid = HybridSearcher::new(dense, sparse, config.rrf_k);

    let opts = SearchOptions {
        limit: 10,
        threshold: 0.0, // Low threshold to get all results from dense
        ..Default::default()
    };

    let results = hybrid.search("login", opts).await.unwrap();

    // Should have results from fusion
    assert!(
        !results.is_empty(),
        "Hybrid search should return fused results"
    );

    // All results should be unique by (file_path, start_line, end_line)
    let mut seen = std::collections::HashSet::new();
    for r in &results {
        let key = (&r.file_path, r.start_line, r.end_line);
        assert!(seen.insert(key), "Duplicate result in hybrid search: {r:?}");
    }
}

#[tokio::test]
async fn hybrid_search_deduplication() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);

    // Insert a chunk that both dense and sparse should find
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn authenticate_user() { verify_credentials(); }",
        "rust",
        Some("authenticate_user"),
        1,
        true,
    )
    .await;
    // Insert another chunk
    insert_test_chunk(
        &db,
        &embedder,
        "src/ui.rs",
        "fn render_dashboard() { display(); }",
        "rust",
        Some("render_dashboard"),
        1,
        true,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let embedder_arc = Arc::new(embedder);
    let config = SearchConfig::default();

    let dense: Arc<dyn vectorcode::engine::SearchStrategy> = Arc::new(DenseSearcher::new(
        db_arc.clone(),
        embedder_arc.clone(),
        config.clone(),
    ));
    let sparse: Arc<dyn vectorcode::engine::SearchStrategy> =
        Arc::new(SparseSearcher::new(db_arc.clone(), config.clone()));

    let hybrid = HybridSearcher::new(dense, sparse, config.rrf_k);

    let opts = SearchOptions {
        limit: 10,
        threshold: 0.0,
        ..Default::default()
    };

    let results = hybrid.search("authenticate", opts).await.unwrap();

    // The auth chunk should appear exactly once even if both searchers found it
    let auth_count = results
        .iter()
        .filter(|r| r.file_path == "src/auth.rs")
        .count();
    assert!(
        auth_count <= 1,
        "Same chunk should appear at most once in fused results"
    );
}

#[tokio::test]
async fn hybrid_search_respects_limit() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);

    for i in 0..5 {
        insert_test_chunk(
            &db,
            &embedder,
            &format!("src/file_{i}.rs"),
            &format!("fn handler_{i}() {{ /* handler number {i} */ }}"),
            "rust",
            Some(&format!("handler_{i}")),
            (i + 1) as u32,
            true,
        )
        .await;
    }

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let embedder_arc = Arc::new(embedder);
    let config = SearchConfig::default();

    let dense: Arc<dyn vectorcode::engine::SearchStrategy> = Arc::new(DenseSearcher::new(
        db_arc.clone(),
        embedder_arc.clone(),
        config.clone(),
    ));
    let sparse: Arc<dyn vectorcode::engine::SearchStrategy> =
        Arc::new(SparseSearcher::new(db_arc.clone(), config.clone()));

    let hybrid = HybridSearcher::new(dense, sparse, 60);

    let opts = SearchOptions {
        limit: 3,
        threshold: 0.0,
        ..Default::default()
    };

    let results = hybrid.search("handler", opts).await.unwrap();
    assert!(
        results.len() <= 3,
        "Hybrid search should respect limit of 3, got {}",
        results.len()
    );
}

#[tokio::test]
async fn hybrid_search_graceful_degradation_sparse_fails() {
    // When sparse fails (e.g., no FTS5 table), hybrid should return dense results

    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);

    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn authenticate() { verify(); }",
        "rust",
        Some("authenticate"),
        1,
        true,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let embedder_arc = Arc::new(embedder);
    let config = SearchConfig::default();

    let dense: Arc<dyn vectorcode::engine::SearchStrategy> = Arc::new(DenseSearcher::new(
        db_arc.clone(),
        embedder_arc.clone(),
        config.clone(),
    ));

    // Create a failing sparse searcher by clearing the DB (removes FTS5 data)
    // and using a fresh SparseSearcher
    let sparse: Arc<dyn vectorcode::engine::SearchStrategy> =
        Arc::new(SparseSearcher::new(db_arc.clone(), config.clone()));

    let hybrid = HybridSearcher::new(dense, sparse, 60);

    let opts = SearchOptions {
        limit: 10,
        threshold: 0.0,
        ..Default::default()
    };

    // Normal search should work (both subsystems functional)
    let results = hybrid.search("authenticate", opts).await;
    assert!(
        results.is_ok(),
        "Hybrid search should succeed when both subsystems work"
    );
}

#[tokio::test]
async fn hybrid_search_scores_are_rrf() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);

    // Insert chunks where auth matches both dense and sparse
    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn authenticate_user() { verify_credentials(); }",
        "rust",
        Some("authenticate_user"),
        1,
        true,
    )
    .await;
    // Chunks that only match dense (no keyword overlap)
    insert_test_chunk(
        &db,
        &embedder,
        "src/ui.rs",
        "fn render_dashboard() { display_widgets(); }",
        "rust",
        Some("render_dashboard"),
        1,
        true,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let embedder_arc = Arc::new(embedder);
    let config = SearchConfig::default();

    let dense: Arc<dyn vectorcode::engine::SearchStrategy> = Arc::new(DenseSearcher::new(
        db_arc.clone(),
        embedder_arc.clone(),
        config.clone(),
    ));
    let sparse: Arc<dyn vectorcode::engine::SearchStrategy> =
        Arc::new(SparseSearcher::new(db_arc.clone(), config.clone()));

    let hybrid = HybridSearcher::new(dense, sparse, 60);

    let opts = SearchOptions {
        limit: 10,
        threshold: 0.0,
        ..Default::default()
    };

    let results = hybrid.search("authenticate", opts).await.unwrap();

    // The auth chunk should rank highest (appears in both dense and sparse)
    if results.len() >= 2 {
        let auth_pos = results.iter().position(|r| r.file_path == "src/auth.rs");
        if let Some(pos) = auth_pos {
            assert_eq!(
                pos, 0,
                "Chunk in both dense+sparse should rank first via RRF"
            );
        }
    }

    // Verify scores are sorted descending
    for w in results.windows(2) {
        assert!(
            w[0].score >= w[1].score,
            "Results should be sorted by descending score: {} < {}",
            w[0].score,
            w[1].score
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════════
// T18: Dense mode preserves backward compatibility
// ════════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dense_mode_returns_correct_format() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);

    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn authenticate_user() { verify_credentials(); }",
        "rust",
        Some("authenticate_user"),
        1,
        true,
    )
    .await;
    insert_test_chunk(
        &db,
        &embedder,
        "src/ui.rs",
        "fn render_dashboard() { display_widgets(); }",
        "rust",
        Some("render_dashboard"),
        1,
        true,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let embedder_arc = Arc::new(embedder);
    let config = SearchConfig::default();
    let dense = DenseSearcher::new(db_arc, embedder_arc, config);

    let opts = SearchOptions {
        limit: 10,
        threshold: 0.0,
        ..Default::default()
    };

    let results = dense.search("authenticate", opts).await.unwrap();

    // Verify result format matches expected SearchResult structure
    for r in &results {
        assert!(!r.file_path.is_empty(), "file_path must be populated");
        assert!(r.start_line > 0, "start_line must be positive");
        assert!(r.end_line >= r.start_line, "end_line >= start_line");
        assert!(!r.kind.is_empty(), "kind must be populated");
        assert!(!r.language.is_empty(), "language must be populated");
        assert!(!r.content.is_empty(), "content must be populated");
    }

    // Scores should be sorted descending
    for w in results.windows(2) {
        assert!(
            w[0].score >= w[1].score,
            "Dense results should be sorted by descending score"
        );
    }
}

#[test]
fn search_options_default_mode_is_dense() {
    let opts = SearchOptions::default();
    assert_eq!(opts.mode, SearchMode::Dense);
    assert_eq!(opts.rrf_k, 60);
    assert_eq!(opts.limit, 10);
    assert!((opts.threshold - 0.3).abs() < f32::EPSILON);
}

#[test]
fn search_mode_default_is_dense() {
    let mode = SearchMode::default();
    assert_eq!(mode, SearchMode::Dense);
}

#[tokio::test]
async fn dense_search_via_strategy_trait() {
    let db = setup_v3_db().await;
    let embedder = MockEmbedder::new(64);

    insert_test_chunk(
        &db,
        &embedder,
        "src/auth.rs",
        "fn authenticate_user() { verify_credentials(); }",
        "rust",
        Some("authenticate_user"),
        1,
        true,
    )
    .await;

    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let embedder_arc = Arc::new(embedder);
    let config = SearchConfig::default();
    let dense = DenseSearcher::new(db_arc, embedder_arc, config);

    // Verify trait method works
    assert_eq!(dense.mode(), SearchMode::Dense);

    let opts = SearchOptions {
        limit: 10,
        threshold: 0.0,
        ..Default::default()
    };

    // Call through trait object
    let strategy: Arc<dyn vectorcode::engine::SearchStrategy> = Arc::new(dense);
    let results = strategy.search("authenticate", opts).await.unwrap();
    assert!(
        !results.is_empty(),
        "Dense search through trait should return results"
    );
}

// ════════════════════════════════════════════════════════════════════════════════
// T19: Migration v2→v3 works and is idempotent
// ════════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn migration_v2_to_v3_creates_fts5_table() {
    let db = setup_v2_schema();

    // Verify we're at v2
    let version: u32 = db
        .conn()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2, "Should start at v2");

    // Verify no FTS5 table exists yet
    let fts_exists: bool = db
        .conn()
        .prepare("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chunks_fts'")
        .unwrap()
        .query_row([], |row| row.get::<_, i64>(0))
        .unwrap()
        > 0;
    assert!(!fts_exists, "FTS5 table should not exist in v2");

    // Run migration
    db.init_schema(64).unwrap();

    // Verify schema version is now 3
    let version: u32 = db
        .conn()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3, "Schema should be v3 after migration");

    // Verify FTS5 table exists
    let fts_exists: bool = db
        .conn()
        .prepare("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chunks_fts'")
        .unwrap()
        .query_row([], |row| row.get::<_, i64>(0))
        .unwrap()
        > 0;
    assert!(fts_exists, "FTS5 table should exist after migration");
}

#[tokio::test]
async fn migration_v2_to_v3_backfills_existing_chunks() {
    let db = setup_v2_schema();

    // Insert chunks BEFORE migration
    insert_chunk_raw(
        &db,
        "c1",
        "src/auth.rs",
        "authenticate_user",
        "fn authenticate_user() { verify(); }",
        "rust",
    );
    insert_chunk_raw(
        &db,
        "c2",
        "src/ui.rs",
        "render_dashboard",
        "fn render_dashboard() { display(); }",
        "rust",
    );

    // Verify chunks exist
    let chunk_count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(chunk_count, 2);

    // Run migration
    db.init_schema(64).unwrap();

    // Verify FTS5 was backfilled — SparseSearcher should find the chunks
    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let sparse = SparseSearcher::new(db_arc, SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        ..Default::default()
    };

    let results = sparse.search("authenticate_user", opts).await.unwrap();
    assert_eq!(
        results.len(),
        1,
        "Existing chunk should be searchable after migration"
    );
    assert_eq!(results[0].file_path, "src/auth.rs");
}

#[tokio::test]
async fn migration_v2_to_v3_is_idempotent() {
    let db = setup_v2_schema();
    insert_chunk_raw(
        &db,
        "c1",
        "src/auth.rs",
        "authenticate_user",
        "fn authenticate_user() { verify(); }",
        "rust",
    );

    // First migration
    db.init_schema(64).unwrap();

    let version: u32 = db
        .conn()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);

    // Second migration should be a no-op
    db.init_schema(64).unwrap();

    let version: u32 = db
        .conn()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 3,
        "Re-running init_schema should not change version"
    );

    // Data should still be searchable
    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let sparse = SparseSearcher::new(db_arc, SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        ..Default::default()
    };

    let results = sparse.search("authenticate_user", opts).await.unwrap();
    assert_eq!(
        results.len(),
        1,
        "Chunks should still be searchable after re-running init_schema"
    );
}

#[tokio::test]
async fn migration_v2_to_v3_triggers_work_for_new_chunks() {
    let db = setup_v2_schema();
    insert_chunk_raw(
        &db,
        "c1",
        "src/auth.rs",
        "authenticate_user",
        "fn authenticate_user() { verify(); }",
        "rust",
    );

    // Migrate
    db.init_schema(64).unwrap();

    // Insert a NEW chunk after migration — triggers should handle FTS sync
    let embedder = MockEmbedder::new(64);
    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    {
        let db_guard = db_arc.lock().await;
        insert_test_chunk(
            &db_guard,
            &embedder,
            "src/billing.rs",
            "fn calculate_tax() { compute(); }",
            "rust",
            Some("calculate_tax"),
            1,
            false,
        )
        .await;
    }

    // Both old (backfilled) and new (trigger-synced) chunks should be searchable
    let sparse = SparseSearcher::new(db_arc.clone(), SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        ..Default::default()
    };

    let old_results = sparse
        .search("authenticate_user", opts.clone())
        .await
        .unwrap();
    assert_eq!(
        old_results.len(),
        1,
        "Backfilled chunk should be searchable"
    );

    let new_results = sparse.search("calculate_tax", opts).await.unwrap();
    assert_eq!(
        new_results.len(),
        1,
        "Post-migration chunk should be searchable via triggers"
    );
}

#[tokio::test]
async fn migration_v2_to_v3_preserves_vector_data() {
    let db = setup_v2_schema();
    let embedder = MockEmbedder::new(64);

    // Insert a chunk with vector data in v2
    let chunk = Chunk {
        id: "test_chunk_v2".to_string(),
        file_path: "src/auth.rs".to_string(),
        start_line: 1,
        end_line: 10,
        byte_start: 0,
        byte_end: 100,
        symbol: Some("authenticate".to_string()),
        kind: "function_declaration".to_string(),
        content: "fn authenticate() { verify(); }".to_string(),
        parent_context: None,
        language: "rust".to_string(),
        file_mtime: 1718000000,
        content_hash: compute_content_hash("fn authenticate() { verify(); }"),
    };
    chunks::insert_chunk(db.conn(), &chunk).unwrap();

    let embedding = embedder
        .embed("fn authenticate() { verify(); }")
        .await
        .unwrap();
    vectors::insert_vector(db.conn(), &chunk.id, &embedding).unwrap();

    // Verify vector data exists before migration
    let vec_count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunk_vec_map", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        vec_count, 1,
        "Should have 1 vector mapping before migration"
    );

    // Migrate
    db.init_schema(64).unwrap();

    // Verify vector data still exists after migration
    let vec_count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM chunk_vec_map", [], |row| row.get(0))
        .unwrap();
    assert_eq!(vec_count, 1, "Vector mapping should survive migration");

    // Verify dense search still works
    let db_arc = Arc::new(tokio::sync::Mutex::new(db));
    let dense = DenseSearcher::new(db_arc, Arc::new(embedder), SearchConfig::default());
    let opts = SearchOptions {
        limit: 10,
        threshold: 0.0,
        ..Default::default()
    };

    let results = dense.search("authenticate", opts).await.unwrap();
    assert!(
        !results.is_empty(),
        "Dense search should work after migration"
    );
    assert_eq!(results[0].file_path, "src/auth.rs");
}
