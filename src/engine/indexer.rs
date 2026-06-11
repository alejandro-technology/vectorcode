//! Indexing pipeline — orchestrates file discovery, chunking, embedding, and storage.
//!
//! Implements spec §9: full project indexing and incremental sync.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::info;

use crate::config::schema::IndexingConfig;
use crate::embedder::Embedder;
use crate::engine::chunker::chunk_file;
use crate::engine::languages::SupportedLanguage;
use crate::store::db::Database;
use crate::store::{chunks, files, vectors};
use crate::types::{compute_content_hash, Chunk};

/// Statistics returned after an indexing operation.
#[derive(Debug, Clone)]
pub struct IndexReport {
    /// Total files found during discovery.
    pub files_scanned: usize,
    /// Files that had new or changed chunks.
    pub files_indexed: usize,
    /// Total chunks in the database after indexing.
    pub chunks_total: usize,
    /// New chunks embedded and stored in this run.
    pub chunks_new: usize,
    /// Chunks skipped because they were unchanged.
    pub chunks_skipped: usize,
    /// Wall-clock duration of the entire indexing operation.
    pub duration: Duration,
}

/// Orchestrates the full and incremental indexing pipeline (spec §9).
pub struct Indexer {
    db: Database,
    embedder: Arc<dyn Embedder>,
    config: IndexingConfig,
}

impl Indexer {
    /// Create a new Indexer with the given database, embedder, and config.
    pub fn new(db: Database, embedder: Arc<dyn Embedder>, config: IndexingConfig) -> Self {
        Self {
            db,
            embedder,
            config,
        }
    }

    /// Index an entire project directory (spec §9.1).
    ///
    /// Discovers files, chunks them, embeds new/changed chunks,
    /// stores results, and cleans stale data.
    pub async fn index_project(&self, project_path: &Path) -> Result<IndexReport> {
        let start = Instant::now();

        // Step 1: Discover files
        info!("[1/3] Discovering files...");
        let file_paths = discover_files(project_path, &self.config);
        let files_scanned = file_paths.len();
        info!("[1/3] Found {} files", files_scanned);

        // Build set of valid relative paths (for stale chunk cleanup)
        let valid_paths: HashSet<String> = file_paths
            .iter()
            .filter_map(|p| p.strip_prefix(project_path).ok())
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        // Step 2: Process files — chunk and detect changes
        let (new_chunks, files_indexed, chunks_skipped) =
            self.process_file_entries(&file_paths, project_path)?;

        let chunks_new = new_chunks.len();
        info!(
            "[2/3] Chunking... {} new, {} skipped",
            chunks_new, chunks_skipped
        );

        // Step 3: Embed and store
        info!("[3/3] Embedding {} chunks...", chunks_new);
        if !new_chunks.is_empty() {
            let texts: Vec<String> = new_chunks.iter().map(enrich_chunk_content).collect();
            let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let embeddings = self.embedder.embed_batch(&text_refs).await?;

            for (chunk, embedding) in new_chunks.iter().zip(embeddings.iter()) {
                chunks::insert_chunk(self.db.conn(), chunk)?;
                vectors::insert_vector(self.db.conn(), &chunk.id, embedding)?;
            }
        }

        // Clean stale chunks (files that no longer exist on disk)
        let _stale = chunks::delete_stale_chunks(self.db.conn(), &valid_paths)?;

        // Count total chunks in DB
        let chunks_total: i64 =
            self.db
                .conn()
                .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;

        let duration = start.elapsed();
        info!(
            "Indexed {} files, {} chunks in {:.1}s",
            files_scanned,
            chunks_total,
            duration.as_secs_f64()
        );

        Ok(IndexReport {
            files_scanned,
            files_indexed,
            chunks_total: chunks_total as usize,
            chunks_new,
            chunks_skipped,
            duration,
        })
    }

    /// Index specific files — incremental sync (spec §9.2).
    ///
    /// Same as `index_project` but only processes the given files.
    /// Does NOT clean stale chunks (since we're processing a subset).
    pub async fn index_files(
        &self,
        file_paths: &[PathBuf],
        project_path: &Path,
    ) -> Result<IndexReport> {
        let start = Instant::now();
        let files_scanned = file_paths.len();

        let (new_chunks, files_indexed, chunks_skipped) =
            self.process_file_entries(file_paths, project_path)?;

        let chunks_new = new_chunks.len();

        if !new_chunks.is_empty() {
            let texts: Vec<String> = new_chunks.iter().map(enrich_chunk_content).collect();
            let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let embeddings = self.embedder.embed_batch(&text_refs).await?;

            for (chunk, embedding) in new_chunks.iter().zip(embeddings.iter()) {
                chunks::insert_chunk(self.db.conn(), chunk)?;
                vectors::insert_vector(self.db.conn(), &chunk.id, embedding)?;
            }
        }

        // NOTE: No stale chunk cleanup for incremental sync

        let chunks_total: i64 =
            self.db
                .conn()
                .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;

        let duration = start.elapsed();

        Ok(IndexReport {
            files_scanned,
            files_indexed,
            chunks_total: chunks_total as usize,
            chunks_new,
            chunks_skipped,
            duration,
        })
    }

    /// Process a list of files: read, chunk, detect changes, collect new chunks.
    ///
    /// Returns (new_chunks, files_indexed, chunks_skipped).
    fn process_file_entries(
        &self,
        file_paths: &[PathBuf],
        project_path: &Path,
    ) -> Result<(Vec<Chunk>, usize, usize)> {
        let mut new_chunks: Vec<Chunk> = Vec::new();
        let mut files_indexed = 0;
        let mut chunks_skipped = 0;

        for file_path in file_paths {
            let relative_path = file_path
                .strip_prefix(project_path)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();

            // Get file metadata
            let metadata = match std::fs::metadata(file_path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Skip files > max_file_size
            if metadata.len() > self.config.max_file_size {
                continue;
            }

            let mtime = metadata
                .modified()
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                })
                .unwrap_or(0);
            let size = metadata.len() as i64;

            // Read file content (skip binary/unreadable files)
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let content_hash = compute_content_hash(&content);

            // Get existing chunks for this file
            let existing_chunks = chunks::list_chunks_by_file(self.db.conn(), &relative_path)?;

            // Check if file is unchanged (mtime + size + hash all match)
            if let Some(file_record) = files::get_file(self.db.conn(), &relative_path)? {
                if file_record.mtime == mtime
                    && file_record.size == size
                    && file_record.hash == content_hash
                {
                    // File unchanged — count existing chunks as skipped
                    chunks_skipped += existing_chunks.len();
                    continue;
                }
            }

            // File is new or changed — parse and chunk
            let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let language = SupportedLanguage::from_extension(ext);
            let file_chunks = chunk_file(&content, &relative_path, language);

            // Collect new chunk IDs to detect removed chunks
            let new_chunk_ids: HashSet<String> = file_chunks.iter().map(|c| c.id.clone()).collect();

            // Delete old chunks that are no longer present in the file
            for old_chunk in &existing_chunks {
                if !new_chunk_ids.contains(&old_chunk.id) {
                    chunks::delete_chunk(self.db.conn(), &old_chunk.id)?;
                }
            }

            // Filter out chunks that already exist with the same content hash
            let mut file_new_chunks = Vec::new();
            for mut chunk in file_chunks {
                if chunks::chunk_exists_with_hash(self.db.conn(), &chunk.id, &chunk.content_hash)? {
                    chunks_skipped += 1;
                    continue;
                }
                chunk.file_mtime = mtime;
                file_new_chunks.push(chunk);
            }

            if !file_new_chunks.is_empty() {
                files_indexed += 1;
                new_chunks.extend(file_new_chunks);
            }

            // Update file record
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            files::upsert_file(
                self.db.conn(),
                &relative_path,
                mtime,
                size,
                &content_hash,
                now,
            )?;
        }

        Ok((new_chunks, files_indexed, chunks_skipped))
    }
}

/// Discover source files in a project directory (spec §9.1 step 3).
///
/// Uses the `ignore` crate to respect .gitignore and applies configured
/// exclusions for directories, extensions, and file size.
pub fn discover_files(project_path: &Path, config: &IndexingConfig) -> Vec<PathBuf> {
    let exclude_dirs = config.exclude_dirs.clone();

    let mut builder = ignore::WalkBuilder::new(project_path);
    builder.hidden(false);
    builder.filter_entry(move |entry| {
        // Skip excluded directories
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            let name = entry.file_name().to_str().unwrap_or("");
            if exclude_dirs.iter().any(|d| d == name) {
                return false;
            }
        }
        true
    });

    let mut files = Vec::new();
    for entry in builder.build().flatten() {
        let path = entry.path().to_path_buf();
        if !path.is_file() {
            continue;
        }

        // Check file size
        if let Ok(metadata) = path.metadata() {
            if metadata.len() > config.max_file_size {
                continue;
            }
        }

        // Check excluded extensions
        let path_str = path.to_str().unwrap_or("");
        if config
            .exclude_extensions
            .iter()
            .any(|ex| path_str.ends_with(ex))
        {
            continue;
        }

        files.push(path);
    }

    files
}

/// Enrich chunk content with metadata for better embedding (spec §8.2).
///
/// Format: `"{language} | {file_path} | {parent_context} | {symbol}\n{content}"`
///
/// This is sent to the embedder but NOT stored in the database.
fn enrich_chunk_content(chunk: &Chunk) -> String {
    let context = chunk.parent_context.as_deref().unwrap_or("");
    let symbol = chunk.symbol.as_deref().unwrap_or("");
    format!(
        "{} | {} | {} | {}\n{}",
        chunk.language, chunk.file_path, context, symbol, chunk.content
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::IndexingConfig;
    use crate::embedder::mock::MockEmbedder;
    use crate::store::db::Database;

    fn setup_test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(64).unwrap();
        db
    }

    fn setup_indexer() -> Indexer {
        let db = setup_test_db();
        let embedder = Arc::new(MockEmbedder::new(64));
        let config = IndexingConfig::default();
        Indexer::new(db, embedder, config)
    }

    /// Create a minimal TypeScript file that produces at least one chunk (>100 bytes).
    fn create_ts_file(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn sample_ts_content() -> String {
        r#"export function calculateSum(a: number, b: number): number {
    const result = a + b;
    console.log("Result:", result);
    console.log("This function performs addition of two numbers");
    console.log("It logs the result to the console for debugging");
    return result;
}

export function subtractValues(a: number, b: number): number {
    const difference = a - b;
    console.log("Difference:", difference);
    console.log("This function performs subtraction of two numbers");
    console.log("It logs the difference to the console for debugging");
    return difference;
}
"#
        .to_string()
    }

    fn sample_py_content() -> String {
        r#"
def calculate_total(items: list, tax_rate: float) -> float:
    """Calculate the total price of items with tax."""
    subtotal = sum(item.price for item in items)
    tax_amount = subtotal * tax_rate
    total = subtotal + tax_amount
    print(f"Subtotal: {subtotal}, Tax: {tax_amount}, Total: {total}")
    return total


def filter_active_users(users: list) -> list:
    """Filter a list of users to only active ones."""
    active = [u for u in users if u.is_active]
    print(f"Found {len(active)} active users out of {len(users)}")
    return active
"#
        .to_string()
    }

    // ─── discover_files tests ──────────────────────────────────────────

    #[test]
    fn discover_files_finds_source_files() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "app.ts", &sample_ts_content());
        std::fs::write(dir.path().join("main.py"), sample_py_content()).unwrap();

        let config = IndexingConfig::default();
        let files = discover_files(dir.path(), &config);

        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(
            names.contains(&"app.ts".to_string()),
            "Should find .ts file, got: {names:?}"
        );
        assert!(
            names.contains(&"main.py".to_string()),
            "Should find .py file, got: {names:?}"
        );
    }

    #[test]
    fn discover_files_excludes_configured_dirs() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "app.ts", &sample_ts_content());

        // Create an excluded directory with a file inside
        let node_modules = dir.path().join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();
        create_ts_file(&node_modules, "lib.ts", &sample_ts_content());

        let config = IndexingConfig::default();
        let files = discover_files(dir.path(), &config);

        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"app.ts".to_string()), "Should find app.ts");
        assert!(
            !names.contains(&"lib.ts".to_string()),
            "Should NOT find lib.ts inside node_modules, got: {names:?}"
        );
    }

    #[test]
    fn discover_files_skips_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "small.ts", &sample_ts_content());

        // Create a file that exceeds max_file_size
        let mut config = IndexingConfig::default();
        config.max_file_size = 50; // Very small limit

        let files = discover_files(dir.path(), &config);
        assert!(
            files.is_empty(),
            "Should skip files exceeding max_file_size, found: {:?}",
            files
        );
    }

    #[test]
    fn discover_files_skips_excluded_extensions() {
        let dir = tempfile::tempdir().unwrap();
        // Create a .min.js file (in excluded extensions)
        std::fs::write(dir.path().join("bundle.min.js"), "var x=1;").unwrap();
        create_ts_file(dir.path(), "app.ts", &sample_ts_content());

        let config = IndexingConfig::default();
        let files = discover_files(dir.path(), &config);

        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(
            !names.contains(&"bundle.min.js".to_string()),
            "Should skip .min.js files, got: {names:?}"
        );
        assert!(
            names.contains(&"app.ts".to_string()),
            "Should include .ts files"
        );
    }

    #[test]
    fn discover_files_empty_dir_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = IndexingConfig::default();
        let files = discover_files(dir.path(), &config);
        assert!(files.is_empty(), "Empty dir should return no files");
    }

    // ─── enrich_chunk_content tests ────────────────────────────────────

    #[test]
    fn enrich_chunk_content_includes_all_fields() {
        let chunk = Chunk {
            id: "test_id".to_string(),
            file_path: "src/auth.ts".to_string(),
            start_line: 1,
            end_line: 10,
            byte_start: 0,
            byte_end: 200,
            symbol: Some("authenticate".to_string()),
            kind: "function_declaration".to_string(),
            content: "function authenticate() { ... }".to_string(),
            parent_context: Some("class AuthService".to_string()),
            language: "typescript".to_string(),
            file_mtime: 0,
            content_hash: "hash".to_string(),
        };

        let enriched = enrich_chunk_content(&chunk);
        assert!(enriched.contains("typescript"), "Should include language");
        assert!(enriched.contains("src/auth.ts"), "Should include file_path");
        assert!(
            enriched.contains("class AuthService"),
            "Should include parent_context"
        );
        assert!(enriched.contains("authenticate"), "Should include symbol");
        assert!(
            enriched.contains("function authenticate()"),
            "Should include content"
        );
    }

    #[test]
    fn enrich_chunk_content_handles_none_fields() {
        let chunk = Chunk {
            id: "test_id".to_string(),
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 5,
            byte_start: 0,
            byte_end: 100,
            symbol: None,
            kind: "function_item".to_string(),
            content: "fn test() {}".to_string(),
            parent_context: None,
            language: "rust".to_string(),
            file_mtime: 0,
            content_hash: "hash".to_string(),
        };

        let enriched = enrich_chunk_content(&chunk);
        assert!(enriched.contains("rust"));
        assert!(enriched.contains("fn test() {}"));
        // Should not panic with None fields
        assert!(enriched.contains(" | test.rs |  | \n"));
    }

    // ─── Indexer integration tests ─────────────────────────────────────

    #[tokio::test]
    async fn index_project_indexes_files_and_returns_report() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "calculator.ts", &sample_ts_content());

        let indexer = setup_indexer();
        let report = indexer.index_project(dir.path()).await.unwrap();

        assert!(
            report.files_scanned >= 1,
            "Should scan at least 1 file, got {}",
            report.files_scanned
        );
        assert!(
            report.files_indexed >= 1,
            "Should index at least 1 file, got {}",
            report.files_indexed
        );
        assert!(
            report.chunks_new >= 1,
            "Should produce at least 1 new chunk, got {}",
            report.chunks_new
        );
        assert!(
            report.chunks_total >= 1,
            "Total chunks should be >= 1, got {}",
            report.chunks_total
        );
        assert!(report.duration.as_nanos() > 0, "Duration should be > 0");
    }

    #[tokio::test]
    async fn index_project_stores_chunks_in_database() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "app.ts", &sample_ts_content());

        let indexer = setup_indexer();
        let report = indexer.index_project(dir.path()).await.unwrap();

        assert!(report.chunks_new >= 1);
        assert_eq!(
            report.chunks_total, report.chunks_new,
            "Total should equal new on first index"
        );

        // Verify chunks are in the DB
        let all_chunks = chunks::list_chunks_by_file(indexer.db.conn(), "app.ts").unwrap();
        assert!(
            !all_chunks.is_empty(),
            "Chunks should be stored in DB for app.ts"
        );
    }

    #[tokio::test]
    async fn index_project_skips_unchanged_on_reindex() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "app.ts", &sample_ts_content());

        let indexer = setup_indexer();

        // First index
        let report1 = indexer.index_project(dir.path()).await.unwrap();
        assert!(
            report1.chunks_new >= 1,
            "First run should produce new chunks"
        );

        // Second index (files unchanged)
        let report2 = indexer.index_project(dir.path()).await.unwrap();
        assert_eq!(
            report2.chunks_new, 0,
            "Second run should produce 0 new chunks, got {}",
            report2.chunks_new
        );
        assert!(
            report2.chunks_skipped >= 1,
            "Second run should skip chunks, got {}",
            report2.chunks_skipped
        );
        assert_eq!(
            report2.chunks_total, report1.chunks_total,
            "Total chunks should remain the same"
        );
    }

    #[tokio::test]
    async fn index_project_cleans_stale_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("temp.ts");
        create_ts_file(&dir.path(), "temp.ts", &sample_ts_content());

        let indexer = setup_indexer();

        // First index — creates chunks
        let report1 = indexer.index_project(dir.path()).await.unwrap();
        assert!(report1.chunks_new >= 1);

        // Delete the file
        std::fs::remove_file(&file_path).unwrap();

        // Second index — should clean stale chunks
        let report2 = indexer.index_project(dir.path()).await.unwrap();
        assert_eq!(
            report2.chunks_total, 0,
            "Total chunks should be 0 after file deletion, got {}",
            report2.chunks_total
        );
    }

    #[tokio::test]
    async fn index_files_indexes_specific_files() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "a.ts", &sample_ts_content());
        create_ts_file(dir.path(), "b.ts", &sample_ts_content());

        let indexer = setup_indexer();

        // Index only a.ts
        let file_paths = vec![dir.path().join("a.ts")];
        let report = indexer.index_files(&file_paths, dir.path()).await.unwrap();

        assert_eq!(report.files_scanned, 1, "Should scan exactly 1 file");
        assert!(report.chunks_new >= 1, "Should produce new chunks");

        // Verify only a.ts has chunks
        let a_chunks = chunks::list_chunks_by_file(indexer.db.conn(), "a.ts").unwrap();
        let b_chunks = chunks::list_chunks_by_file(indexer.db.conn(), "b.ts").unwrap();
        assert!(!a_chunks.is_empty(), "a.ts should have chunks");
        assert!(b_chunks.is_empty(), "b.ts should NOT have chunks");
    }

    #[tokio::test]
    async fn index_project_multiple_languages() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "app.ts", &sample_ts_content());
        std::fs::write(dir.path().join("main.py"), sample_py_content()).unwrap();

        let indexer = setup_indexer();
        let report = indexer.index_project(dir.path()).await.unwrap();

        assert!(report.files_scanned >= 2, "Should scan at least 2 files");
        assert!(
            report.chunks_new >= 2,
            "Should produce chunks for both languages, got {}",
            report.chunks_new
        );

        // Verify both languages are stored
        let ts_chunks = chunks::list_chunks_by_file(indexer.db.conn(), "app.ts").unwrap();
        let py_chunks = chunks::list_chunks_by_file(indexer.db.conn(), "main.py").unwrap();
        assert!(!ts_chunks.is_empty(), "Should have TypeScript chunks");
        assert!(!py_chunks.is_empty(), "Should have Python chunks");
    }

    #[tokio::test]
    async fn index_project_empty_dir_returns_zero_stats() {
        let dir = tempfile::tempdir().unwrap();

        let indexer = setup_indexer();
        let report = indexer.index_project(dir.path()).await.unwrap();

        assert_eq!(report.files_scanned, 0);
        assert_eq!(report.files_indexed, 0);
        assert_eq!(report.chunks_new, 0);
        assert_eq!(report.chunks_total, 0);
    }

    #[tokio::test]
    async fn index_project_vectors_are_stored() {
        let dir = tempfile::tempdir().unwrap();
        create_ts_file(dir.path(), "app.ts", &sample_ts_content());

        let indexer = setup_indexer();
        indexer.index_project(dir.path()).await.unwrap();

        // Verify vectors are stored by searching
        let ts_chunks = chunks::list_chunks_by_file(indexer.db.conn(), "app.ts").unwrap();
        assert!(!ts_chunks.is_empty());

        // Each chunk should have a corresponding vector
        for chunk in &ts_chunks {
            let results =
                vectors::search_similar(indexer.db.conn(), &[0.0; 64], 100, -1.0).unwrap();
            let found = results.iter().any(|r| r.file_path == chunk.file_path);
            assert!(found, "Chunk {} should have a stored vector", chunk.id);
        }
    }
}
