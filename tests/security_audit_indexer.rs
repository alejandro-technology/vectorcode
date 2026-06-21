//! Security audit — indexer tests (phase-4.2).
//!
//! Enforces REQ-SEC-01: the indexer must skip files whose canonical path
//! falls outside the project root (closes the symlink escape vector at
//! `indexer.rs:435`).
//!
//! **Strict TDD — RED at C1**: the current indexer follows symlinks to
//! files outside the workspace, so the "leaked" file ends up in the chunk
//! table. C2 adds the canonicalize + `starts_with` check that turns these
//! tests green.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::Arc;

use vectorcode::config::schema::IndexingConfig;
use vectorcode::embedder::mock::MockEmbedder;
use vectorcode::engine::indexer::Indexer;
use vectorcode::store::db::Database;

struct IndexerHarness {
    indexer: Indexer,
    db_path: PathBuf,
}

impl IndexerHarness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        // Keep the dir alive by leaking it into a static — tests run sequentially.
        std::mem::forget(dir);
        let db = Database::open(&db_path).unwrap();
        db.init_schema(64).unwrap();
        let embedder = Arc::new(MockEmbedder::new(64));
        let config = IndexingConfig::default();
        let indexer = Indexer::new(Arc::new(tokio::sync::Mutex::new(db)), embedder, config);
        Self { indexer, db_path }
    }

    fn chunks_for(&self, rel_path: &str) -> Vec<vectorcode::Chunk> {
        let db = Database::open(&self.db_path).unwrap();
        vectorcode::store::chunks::list_chunks_by_file(db.conn(), rel_path).unwrap_or_default()
    }
}

fn sample_rs_content() -> &'static str {
    r#"
pub fn calculate_total(items: Vec<u32>, tax_rate: f64) -> f64 {
    let subtotal: f64 = items.iter().map(|&x| x as f64).sum();
    let tax_amount = subtotal * tax_rate;
    let total = subtotal + tax_amount;
    println!("Subtotal: {}, Tax: {}, Total: {}", subtotal, tax_amount, total);
    total
}

pub fn filter_active_users(users: Vec<u32>) -> Vec<u32> {
    let active: Vec<u32> = users.into_iter().filter(|&u| u > 0).collect();
    println!("Found {} active users", active.len());
    active
}
"#
}

/// Test 12 (per proposal): symlink to a file outside the workspace must be
/// skipped — the outside file's content must NOT appear in the index.
#[tokio::test]
async fn indexer_skips_symlink_pointing_outside_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();

    // File containing "SECRET" content, outside the workspace.
    let outside_file = outside.path().join("secret.rs");
    fs::write(&outside_file, "SECRET_DATA_THAT_MUST_NOT_LEAK").unwrap();

    // Symlink inside the workspace that points at the outside file.
    let link = workspace.path().join("leaked.rs");
    symlink(&outside_file, &link).unwrap();

    let harness = IndexerHarness::new();
    let report = harness
        .indexer
        .index_project(workspace.path())
        .await
        .unwrap();

    // No file should be indexed (the only file is the symlink, which must be skipped).
    assert_eq!(
        report.files_indexed, 0,
        "Symlink to outside file must be skipped, but {} files were indexed",
        report.files_indexed
    );
    assert_eq!(
        report.chunks_new, 0,
        "No chunks should be created from a symlink escape, got {}",
        report.chunks_new
    );

    // And the chunk table must be empty for the leaked path.
    let chunks = harness.chunks_for("leaked.rs");
    assert!(
        chunks.is_empty(),
        "Found {} chunks for leaked.rs — symlink escape was NOT blocked",
        chunks.len()
    );
}

/// Internal symlink (resolves inside the workspace) must still be indexed
/// normally — we only want to block ESCAPE, not break legitimate monorepo
/// symlinks.
#[tokio::test]
async fn indexer_indexes_internal_symlink() {
    let workspace = tempfile::tempdir().unwrap();
    let src_dir = workspace.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let real = src_dir.join("real.rs");
    fs::write(&real, sample_rs_content()).unwrap();

    let link = src_dir.join("alias.rs");
    symlink(&real, &link).unwrap();

    let harness = IndexerHarness::new();
    let report = harness
        .indexer
        .index_project(workspace.path())
        .await
        .unwrap();

    // At least one file should be indexed (the real file).
    assert!(
        report.files_indexed >= 1,
        "Real file should be indexed, report: {report:?}"
    );
    assert!(
        report.chunks_new >= 1,
        "Chunks should be created for the real file, report: {report:?}"
    );

    // The real file should have chunks in the database.
    let real_chunks = harness.chunks_for("src/real.rs");
    assert!(
        !real_chunks.is_empty(),
        "Real file should have chunks, got {}",
        real_chunks.len()
    );
}

/// Chained symlink: a → b → /etc/hostname (outside the workspace).
/// The final canonical target must be detected as outside.
#[tokio::test]
async fn indexer_skips_chained_symlink_escape() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();

    let outside_file = outside.path().join("chain_target.txt");
    fs::write(&outside_file, "OUTSIDE_CONTENT").unwrap();

    // Create an intermediate symlink inside the workspace, pointing to the
    // outside file.
    let intermediate = workspace.path().join("b");
    symlink(&outside_file, &intermediate).unwrap();

    // Create a top-level symlink inside the workspace pointing to the
    // intermediate.
    let top = workspace.path().join("a");
    symlink(&intermediate, &top).unwrap();

    let harness = IndexerHarness::new();
    let report = harness
        .indexer
        .index_project(workspace.path())
        .await
        .unwrap();

    // The chained escape must be detected and skipped.
    assert_eq!(
        report.files_indexed, 0,
        "Chained symlink escape must be skipped, but {} files were indexed",
        report.files_indexed
    );

    // No chunks for the top-level symlink.
    let a_chunks = harness.chunks_for("a");
    assert!(
        a_chunks.is_empty(),
        "Found {} chunks for 'a' — chained symlink escape was NOT blocked",
        a_chunks.len()
    );
}

/// Sanity check: a real file inside the workspace (no symlinks) must still
/// be indexed. This guards against an over-eager C2 fix that skips
/// everything.
#[tokio::test]
async fn indexer_indexes_real_file_inside_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let src = workspace.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("main.rs"), sample_rs_content()).unwrap();

    let harness = IndexerHarness::new();
    let report = harness
        .indexer
        .index_project(workspace.path())
        .await
        .unwrap();

    assert!(
        report.files_indexed >= 1,
        "Real file should be indexed, report: {report:?}"
    );
    assert!(
        report.chunks_new >= 1,
        "Chunks should be created, report: {report:?}"
    );

    let chunks = harness.chunks_for("src/main.rs");
    assert!(
        !chunks.is_empty(),
        "Real file should have chunks, got {}",
        chunks.len()
    );
}
