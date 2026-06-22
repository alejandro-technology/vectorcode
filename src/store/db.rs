use anyhow::Result;
use rusqlite::Connection;
use std::sync::OnceLock;

use crate::VectorCodeError;

/// Current schema version — bump when migrating.
const SCHEMA_VERSION: u32 = 4;

/// Normalize a vector to the target dimension by padding with zeros or truncating.
///
/// Used during v1→v2 migration when stored embeddings may not match the
/// configured dimensions.
fn normalize_dimensions(vec: &[f32], target_dims: usize) -> Vec<f32> {
    if vec.len() == target_dims {
        return vec.to_vec();
    }
    let mut result = vec![0.0f32; target_dims];
    let copy_len = vec.len().min(target_dims);
    result[..copy_len].copy_from_slice(&vec[..copy_len]);
    result
}

/// Register the sqlite-vec extension exactly once per process.
///
/// Uses `OnceLock` to ensure `sqlite3_auto_extension` is called before any
/// connection is opened, and only once. After this, every new SQLite connection
/// will automatically have sqlite-vec functions (vec_version, vec0, etc.).
fn register_sqlite_vec() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        // sqlite3_vec_init is an opaque C entry point; transmute to the
        // sqlite3_auto_extension callback signature expected by SQLite.
        type AutoExtFn = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        unsafe {
            let init_fn: AutoExtFn = std::mem::transmute::<unsafe extern "C" fn(), AutoExtFn>(
                sqlite_vec::sqlite3_vec_init,
            );
            rusqlite::ffi::sqlite3_auto_extension(Some(init_fn));
        }
    });
}

/// SQLite database wrapper with WAL mode and schema management.
///
/// Spec §6: single file at `.vectorcode/index.db`, WAL mode (ST-1, ST-6).
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) a database at the given path with WAL mode.
    pub fn open(path: &std::path::Path) -> Result<Self, VectorCodeError> {
        register_sqlite_vec();
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, VectorCodeError> {
        register_sqlite_vec();
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self { conn })
    }

    /// Initialize the full schema per spec §6.
    ///
    /// Creates `meta`, `chunks`, `files`, `vectors_data`, and `chunk_vec_map` tables.
    /// The `vec_chunks` virtual table (sqlite-vec) is created when the extension is
    /// available; if not, we fall back to `vectors_data` (ST-5 fallback).
    ///
    /// Handles v1→v2 migration: migrates `vectors_data` JSON embeddings into
    /// `vec_chunks` binary format when upgrading from schema version 1.
    ///
    /// Uses `user_version` PRAGMA for migration tracking.
    pub fn init_schema(&self, dims: u32) -> Result<(), VectorCodeError> {
        let current_version: u32 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;

        if current_version >= SCHEMA_VERSION {
            return Ok(());
        }

        self.conn.execute_batch(
            "
            -- Index metadata (singleton row pattern: key-value)
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Chunk metadata per spec §6
            CREATE TABLE IF NOT EXISTS chunks (
                id             TEXT PRIMARY KEY,
                file_path      TEXT NOT NULL,
                start_line     INTEGER NOT NULL,
                end_line       INTEGER NOT NULL,
                byte_start     INTEGER NOT NULL,
                byte_end       INTEGER NOT NULL,
                symbol         TEXT,
                kind           TEXT NOT NULL,
                content        TEXT NOT NULL,
                parent_context TEXT,
                language       TEXT NOT NULL,
                file_mtime     INTEGER NOT NULL,
                content_hash   TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_chunks_file_path ON chunks(file_path);
            CREATE INDEX IF NOT EXISTS idx_chunks_symbol ON chunks(symbol) WHERE symbol IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_chunks_language ON chunks(language);
            CREATE INDEX IF NOT EXISTS idx_chunks_content_hash ON chunks(content_hash);

            -- File tracking for incremental sync per spec §6
            CREATE TABLE IF NOT EXISTS files (
                path       TEXT PRIMARY KEY,
                mtime      INTEGER NOT NULL,
                size       INTEGER NOT NULL,
                hash       TEXT NOT NULL,
                indexed_at INTEGER NOT NULL
            );

            -- Vector fallback storage (used when sqlite-vec extension is unavailable).
            -- Stores embedding as JSON array of floats.
            CREATE TABLE IF NOT EXISTS vectors_data (
                chunk_id  TEXT PRIMARY KEY,
                embedding TEXT NOT NULL,
                FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
            );

            -- Mapping table: chunk_id → vec_chunks rowid.
            -- Used when sqlite-vec extension is available to link text chunk IDs
            -- to the implicit integer rowids of the vec0 virtual table.
            CREATE TABLE IF NOT EXISTS chunk_vec_map (
                chunk_id  TEXT PRIMARY KEY,
                vec_rowid INTEGER NOT NULL,
                FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
            );

            -- FTS5 virtual table for sparse (lexical) search.
            -- External content mode: backed by the chunks table.
            -- Column order matters for bm25 weights: symbol(10), content(5), file_path(2), language(1).
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                symbol,
                content,
                file_path,
                language,
                content='chunks',
                content_rowid='rowid',
                tokenize='unicode61 remove_diacritics 1'
            );

            -- FTS5 sync triggers: keep chunks_fts in sync with chunks automatically.
            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, symbol, content, file_path, language)
                VALUES (new.rowid, COALESCE(new.symbol, ''), new.content, new.file_path, new.language);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, symbol, content, file_path, language)
                VALUES ('delete', old.rowid, COALESCE(old.symbol, ''), old.content, old.file_path, old.language);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, symbol, content, file_path, language)
                VALUES ('delete', old.rowid, COALESCE(old.symbol, ''), old.content, old.file_path, old.language);
                INSERT INTO chunks_fts(rowid, symbol, content, file_path, language)
                VALUES (new.rowid, COALESCE(new.symbol, ''), new.content, new.file_path, new.language);
            END;

            -- Phase 2 Knowledge Graph tables
            CREATE TABLE IF NOT EXISTS graph_nodes (
                id         TEXT PRIMARY KEY,
                symbol     TEXT NOT NULL,
                kind       TEXT NOT NULL,
                file_path  TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_graph_nodes_symbol ON graph_nodes(symbol);

            CREATE TABLE IF NOT EXISTS graph_edges (
                source_id     TEXT NOT NULL,
                target_symbol TEXT NOT NULL,
                edge_type     TEXT NOT NULL,
                FOREIGN KEY (source_id) REFERENCES graph_nodes(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_graph_edges_source_id ON graph_edges(source_id);
            CREATE INDEX IF NOT EXISTS idx_graph_edges_target_symbol ON graph_edges(target_symbol);
            ",
        )?;

        // Attempt to create the sqlite-vec virtual table with cosine distance.
        // This will fail gracefully if the extension is not loaded.
        let vec_sql = format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(\
                embedding float[{dims}] distance_metric=cosine\
            )"
        );
        let _vec_result = self.conn.execute_batch(&vec_sql);
        // We intentionally ignore errors here — the fallback table handles it.

        // Store the embedding dimensions in meta table for later retrieval
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('embedding_dims', ?1)",
            [dims.to_string().as_str()],
        )?;

        // v1 → v2 migration: migrate vectors_data JSON embeddings to vec_chunks binary.
        if current_version == 1 && self.has_vec_extension() {
            self.migrate_v1_to_v2(dims)?;
        }

        // v2 → v3 migration: backfill existing chunks into FTS5 index.
        // Uses the canonical FTS5 'rebuild' command which repopulates from the
        // external content table (chunks). Wrapped in transaction for atomicity.
        if current_version == 1 || current_version == 2 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')")?;
            tx.commit()?;
        }

        // Set schema version
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;

        Ok(())
    }

    /// Recreate the vector index tables to support a new embedding dimension.
    /// This will drop `vec_chunks` and `chunk_vec_map`, forcing the indexer
    /// to re-embed all existing chunks.
    pub fn recreate_vector_index(&self, new_dims: u32) -> Result<(), VectorCodeError> {
        let tx = self.conn.unchecked_transaction()?;
        
        // Drop existing vector mapping and sqlite-vec table
        tx.execute_batch(
            "DROP TABLE IF EXISTS chunk_vec_map;
             DROP TABLE IF EXISTS vec_chunks;"
        )?;
        
        // Recreate the chunk mapping table
        tx.execute_batch(
            "CREATE TABLE chunk_vec_map (
                chunk_id TEXT PRIMARY KEY,
                vec_rowid INTEGER NOT NULL UNIQUE,
                FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
            );"
        )?;
        
        // Recreate the sqlite-vec table with the new dimensions
        let vec_sql = format!(
            "CREATE VIRTUAL TABLE vec_chunks USING vec0(\
                embedding float[{new_dims}] distance_metric=cosine\
            )"
        );
        let _ = tx.execute_batch(&vec_sql);
        
        // Update the metadata
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('embedding_dims', ?1)",
            [new_dims.to_string().as_str()],
        )?;
        
        tx.commit()?;
        Ok(())
    }

    /// Migrate vector data from v1 (JSON in vectors_data) to v2 (binary in vec_chunks).
    ///
    /// Reads each row from `vectors_data`, deserializes the JSON embedding,
    /// converts to a binary blob, and inserts into `vec_chunks` + `chunk_vec_map`.
    fn migrate_v1_to_v2(&self, dims: u32) -> Result<(), VectorCodeError> {
        // Check if there's anything to migrate
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vectors_data", [], |row| row.get(0))?;
        if count == 0 {
            return Ok(());
        }

        let mut select_stmt = self
            .conn
            .prepare("SELECT chunk_id, embedding FROM vectors_data")?;
        let rows_iter = select_stmt.query_map([], |row| {
            let chunk_id: String = row.get(0)?;
            let embedding_json: String = row.get(1)?;
            Ok((chunk_id, embedding_json))
        })?;

        let mut rows = Vec::new();
        for r in rows_iter {
            rows.push(r.map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Migration read error: {e}"),
            })?);
        }

        // Drop the statement to release lock/borrows before writing
        drop(select_stmt);

        // Wrap all mutations in a transaction for atomicity
        let tx = self.conn.unchecked_transaction()?;

        for (chunk_id, embedding_json) in rows {
            let embedding: Vec<f32> = match serde_json::from_str(&embedding_json) {
                Ok(v) => v,
                Err(_) => continue, // Skip malformed rows
            };

            // Ensure the embedding matches expected dimensions (pad or truncate)
            let embedding = normalize_dimensions(&embedding, dims as usize);

            // Convert to binary blob (little-endian f32 bytes)
            let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

            // Insert into vec_chunks (let SQLite assign rowid)
            tx.execute(
                "INSERT INTO vec_chunks(rowid, embedding) VALUES (NULL, ?1)",
                rusqlite::params![blob],
            )?;

            // Get the assigned rowid
            let vec_rowid: i64 = tx.last_insert_rowid();

            // Store the mapping
            tx.execute(
                "INSERT OR REPLACE INTO chunk_vec_map (chunk_id, vec_rowid) VALUES (?1, ?2)",
                (&chunk_id, vec_rowid),
            )?;
        }

        // Commit transaction after all inserts succeed
        tx.commit()?;

        Ok(())
    }

    /// Check whether the sqlite-vec extension is available.
    ///
    /// Queries `vec_version()` which is provided by the extension itself,
    /// independent of whether any virtual tables have been created.
    pub fn has_vec_extension(&self) -> bool {
        self.conn.prepare("SELECT vec_version()").is_ok()
    }

    /// Get a reference to the underlying connection (for CRUD modules).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Get a mutable reference to the underlying connection.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Clear all data from the database.
    pub fn clear_database(&self) -> Result<(), VectorCodeError> {
        let tx = self.conn.unchecked_transaction()?;
        if crate::store::vectors::has_vec_extension(&tx) {
            tx.execute("DELETE FROM vec_chunks", [])?;
            tx.execute("DELETE FROM chunk_vec_map", [])?;
        } else {
            tx.execute("DELETE FROM vectors_data", [])?;
        }
        tx.execute("DELETE FROM chunks", [])?;
        // Safety net: rebuild FTS5 index after chunks are deleted.
        // Triggers handle individual row cleanup, but rebuild ensures consistency.
        tx.execute_batch("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')")?;
        tx.execute("DELETE FROM files", [])?;
        tx.execute("DELETE FROM meta", [])?;
        tx.execute("DELETE FROM graph_edges", [])?;
        tx.execute("DELETE FROM graph_nodes", [])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_creates_database() {
        let db = Database::open_in_memory().unwrap();
        // Verify we can execute a query
        let result: String = db
            .conn()
            .query_row("SELECT 'hello'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn open_in_file_creates_database() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        let result: String = db
            .conn()
            .query_row("SELECT 'world'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, "world");
        assert!(db_path.exists(), "Database file should exist");
    }

    #[test]
    fn wal_mode_is_set_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("wal_test.db");
        let db = Database::open(&db_path).unwrap();
        let mode: String = db
            .conn()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "WAL mode must be set on open");
    }

    #[test]
    fn init_schema_creates_all_tables() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(384).unwrap();

        let tables: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(
            tables.contains(&"meta".to_string()),
            "meta table missing: {tables:?}"
        );
        assert!(
            tables.contains(&"chunks".to_string()),
            "chunks table missing: {tables:?}"
        );
        assert!(
            tables.contains(&"files".to_string()),
            "files table missing: {tables:?}"
        );
        assert!(
            tables.contains(&"vectors_data".to_string()),
            "vectors_data table missing: {tables:?}"
        );
    }

    #[test]
    fn init_schema_sets_user_version() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(384).unwrap();

        let version: u32 = db
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4, "Schema version must be 4 after init");
    }

    #[test]
    fn init_schema_is_idempotent() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(384).unwrap();
        // Second call should succeed without error
        db.init_schema(384).unwrap();

        let version: u32 = db
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
    }

    #[test]
    fn init_schema_creates_indexes_on_chunks() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(384).unwrap();

        let indexes: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='chunks' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(
            indexes.contains(&"idx_chunks_file_path".to_string()),
            "idx_chunks_file_path missing: {indexes:?}"
        );
        assert!(
            indexes.contains(&"idx_chunks_language".to_string()),
            "idx_chunks_language missing: {indexes:?}"
        );
        assert!(
            indexes.contains(&"idx_chunks_content_hash".to_string()),
            "idx_chunks_content_hash missing: {indexes:?}"
        );
    }

    #[test]
    fn chunks_table_has_correct_columns() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(384).unwrap();

        let columns: Vec<String> = db
            .conn()
            .prepare("PRAGMA table_info(chunks)")
            .unwrap()
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(columns.contains(&"id".to_string()));
        assert!(columns.contains(&"file_path".to_string()));
        assert!(columns.contains(&"start_line".to_string()));
        assert!(columns.contains(&"end_line".to_string()));
        assert!(columns.contains(&"content".to_string()));
        assert!(columns.contains(&"content_hash".to_string()));
        assert!(columns.contains(&"language".to_string()));
        assert!(columns.contains(&"symbol".to_string()));
        assert!(columns.contains(&"parent_context".to_string()));
    }

    #[test]
    fn files_table_has_correct_columns() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(384).unwrap();

        let columns: Vec<String> = db
            .conn()
            .prepare("PRAGMA table_info(files)")
            .unwrap()
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(columns.contains(&"path".to_string()));
        assert!(columns.contains(&"mtime".to_string()));
        assert!(columns.contains(&"size".to_string()));
        assert!(columns.contains(&"hash".to_string()));
        assert!(columns.contains(&"indexed_at".to_string()));
    }

    #[test]
    fn open_fails_for_invalid_path() {
        let result = Database::open(std::path::Path::new("/nonexistent/dir/db.sqlite"));
        assert!(result.is_err(), "Opening invalid path must fail");
    }

    #[test]
    fn has_vec_extension_returns_true_after_open() {
        let db = Database::open_in_memory().unwrap();
        assert!(
            db.has_vec_extension(),
            "sqlite-vec extension must be available after database open"
        );
    }

    #[test]
    fn vec_version_returns_valid_string() {
        let db = Database::open_in_memory().unwrap();
        let version: String = db
            .conn()
            .query_row("SELECT vec_version()", [], |row| row.get(0))
            .expect("vec_version() must be available when sqlite-vec is registered");
        assert!(
            version.starts_with('v') || version.starts_with('0'),
            "vec_version() must return a valid version string, got: {version}"
        );
    }

    #[test]
    fn vec0_virtual_table_can_be_created_and_queried() {
        let db = Database::open_in_memory().unwrap();
        // Create a vec0 virtual table with 4-dimensional float vectors
        db.conn()
            .execute_batch("CREATE VIRTUAL TABLE test_vec USING vec0(embedding float[4])")
            .expect("vec0 virtual table creation must succeed with sqlite-vec loaded");

        // Insert a vector using rowid and binary blob (f32 le bytes)
        let vec_data: Vec<u8> = [1.0f32, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        db.conn()
            .execute(
                "INSERT INTO test_vec(rowid, embedding) VALUES (1, ?1)",
                rusqlite::params![vec_data],
            )
            .expect("Insert into vec0 must succeed");

        // Query with KNN match
        let query_blob: Vec<u8> = [0.9f32, 0.1, 0.0, 0.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let rowid: i64 = db
            .conn()
            .prepare(
                "SELECT rowid FROM test_vec WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
            )
            .unwrap()
            .query_row(rusqlite::params![query_blob], |row| row.get(0))
            .expect("KNN query on vec0 must return results");
        assert_eq!(rowid, 1, "Nearest neighbor must be the inserted row");
    }

    // ─── Phase 5: vec_chunks wiring tests ──────────────────────────────

    #[test]
    fn init_schema_creates_vec_chunks_virtual_table() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        // vec_chunks should exist as a virtual table in sqlite_master
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='vec_chunks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "vec_chunks virtual table must be created when sqlite-vec extension is available"
        );
    }

    #[test]
    fn init_schema_creates_chunk_vec_map_table() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let tables: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(
            tables.contains(&"chunk_vec_map".to_string()),
            "chunk_vec_map table missing: {tables:?}"
        );
    }

    #[test]
    fn init_schema_v3_sets_user_version_to_3() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let version: u32 = db
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(
            version, 4,
            "Schema version must be 4 after init with FTS5 support"
        );
    }

    #[test]
    fn init_schema_migrates_v1_vectors_data_to_vec_chunks() {
        let db = Database::open_in_memory().unwrap();

        // Manually create a v1 schema (without vec_chunks)
        db.conn()
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS chunks (
                    id             TEXT PRIMARY KEY,
                    file_path      TEXT NOT NULL,
                    start_line     INTEGER NOT NULL,
                    end_line       INTEGER NOT NULL,
                    byte_start     INTEGER NOT NULL,
                    byte_end       INTEGER NOT NULL,
                    symbol         TEXT,
                    kind           TEXT NOT NULL,
                    content        TEXT NOT NULL,
                    parent_context TEXT,
                    language       TEXT NOT NULL,
                    file_mtime     INTEGER NOT NULL,
                    content_hash   TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS files (
                    path       TEXT PRIMARY KEY,
                    mtime      INTEGER NOT NULL,
                    size       INTEGER NOT NULL,
                    hash       TEXT NOT NULL,
                    indexed_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS vectors_data (
                    chunk_id  TEXT PRIMARY KEY,
                    embedding TEXT NOT NULL,
                    FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
                );
                ",
            )
            .unwrap();

        // Set user_version to 1 (v1 schema)
        db.conn().pragma_update(None, "user_version", 1).unwrap();

        // Insert a test chunk and its vector (v1 format: JSON array)
        db.conn()
            .execute(
                "INSERT INTO chunks (id, file_path, start_line, end_line, byte_start, byte_end, \
                 kind, content, language, file_mtime, content_hash) \
                 VALUES (?1, ?2, 1, 5, 0, 20, 'function', 'fn test() {}', 'rust', 0, 'hash1')",
                ("chunk_migrate_1", "src/test.rs"),
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO vectors_data (chunk_id, embedding) VALUES (?1, ?2)",
                ("chunk_migrate_1", "[1.0, 0.0, 0.0, 0.0]"),
            )
            .unwrap();

        // Now run init_schema — should migrate v1 → v2 → v3
        db.init_schema(4).unwrap();

        // Verify user_version is now 4
        let version: u32 = db
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4, "Schema version must be 4 after migration");

        // Verify vec_chunks exists
        let vec_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='vec_chunks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(vec_count, 1, "vec_chunks must exist after migration");

        // Verify the vector was migrated: search for it in vec_chunks
        let query_blob: Vec<u8> = [1.0f32, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let mapping_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunk_vec_map WHERE chunk_id = ?1",
                ["chunk_migrate_1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            mapping_count, 1,
            "chunk_vec_map must have the migrated chunk_id"
        );

        // Verify we can find the migrated vector via KNN query
        let rowid: i64 = db
            .conn()
            .prepare(
                "SELECT m.vec_rowid FROM (\
                    SELECT rowid FROM vec_chunks \
                    WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1\
                ) v \
                JOIN chunk_vec_map m ON v.rowid = m.vec_rowid",
            )
            .unwrap()
            .query_row(rusqlite::params![query_blob], |row| row.get(0))
            .expect("Migrated vector must be findable via KNN query");
        assert!(rowid > 0, "Migrated vector must have a valid rowid");
    }

    #[test]
    fn vec0_with_cosine_distance_metric_works() {
        let db = Database::open_in_memory().unwrap();
        db.conn()
            .execute_batch(
                "CREATE VIRTUAL TABLE test_cosine USING vec0(\
                    embedding float[4] distance_metric=cosine\
                )",
            )
            .expect("vec0 with distance_metric=cosine must succeed");

        // Insert two vectors: one aligned with query, one orthogonal
        let aligned: Vec<u8> = [1.0f32, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let orthogonal: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        db.conn()
            .execute(
                "INSERT INTO test_cosine(rowid, embedding) VALUES (1, ?1)",
                rusqlite::params![aligned],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO test_cosine(rowid, embedding) VALUES (2, ?1)",
                rusqlite::params![orthogonal],
            )
            .unwrap();

        // Query with same direction as vector 1
        let query: Vec<u8> = [1.0f32, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let (rowid, distance): (i64, f32) = db
            .conn()
            .prepare(
                "SELECT rowid, distance FROM test_cosine \
                 WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
            )
            .unwrap()
            .query_row(rusqlite::params![query], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();

        assert_eq!(rowid, 1, "Nearest neighbor must be the aligned vector");
        assert!(
            distance.abs() < 0.01,
            "Cosine distance of identical vectors should be ~0.0, got {distance}"
        );
    }

    #[test]
    fn clear_database_removes_all_data() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        // Insert some metadata
        db.conn()
            .execute(
                "INSERT INTO meta (key, value) VALUES ('test_key', 'test_value')",
                [],
            )
            .unwrap();

        // Insert some chunks
        db.conn()
            .execute(
                "INSERT INTO chunks (id, file_path, start_line, end_line, byte_start, byte_end, content, language, kind, file_mtime, content_hash) \
                 VALUES ('chunk_1', 'file.rs', 1, 5, 0, 10, 'content', 'rust', 'line_block', 0, '')",
                [],
            )
            .unwrap();

        // Verify data exists
        let chunk_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(chunk_count, 1);

        // Verify FTS5 has the data (via trigger)
        let fts_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'content'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_count, 1, "FTS5 must have the chunk before clear");

        // Clear database
        db.clear_database().unwrap();

        // Verify all tables are empty
        let chunk_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(chunk_count, 0);

        let meta_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(meta_count, 0);

        // Verify FTS5 is also empty after clear
        let fts_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'content'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_count, 0, "FTS5 must be empty after clear_database");
    }

    // ─── T2: FTS5 virtual table tests ───────────────────────────────────

    #[test]
    fn init_schema_creates_chunks_fts() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        // FTS5 virtual table should appear in sqlite_master as type='table'
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='chunks_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "chunks_fts FTS5 virtual table must be created by init_schema"
        );
    }

    // ─── T3: FTS5 triggers + migration tests ────────────────────────────

    #[test]
    fn fts_trigger_fires_on_insert() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        // Insert a chunk
        db.conn()
            .execute(
                "INSERT INTO chunks (id, file_path, start_line, end_line, byte_start, byte_end, \
                 symbol, kind, content, language, file_mtime, content_hash) \
                 VALUES ('c1', 'src/lib.rs', 1, 10, 0, 50, 'my_func', 'function', \
                 'fn my_func() {}', 'rust', 0, 'hash1')",
                [],
            )
            .unwrap();

        // FTS5 should have the row
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'my_func'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "FTS5 must contain the inserted chunk via trigger");
    }

    #[test]
    fn fts_trigger_fires_on_delete() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        db.conn()
            .execute(
                "INSERT INTO chunks (id, file_path, start_line, end_line, byte_start, byte_end, \
                 symbol, kind, content, language, file_mtime, content_hash) \
                 VALUES ('c2', 'src/lib.rs', 1, 10, 0, 50, 'delete_me', 'function', \
                 'fn delete_me() {}', 'rust', 0, 'hash2')",
                [],
            )
            .unwrap();

        // Verify it's in FTS5
        let count_before: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'delete_me'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_before, 1);

        // Delete the chunk
        db.conn()
            .execute("DELETE FROM chunks WHERE id = 'c2'", [])
            .unwrap();

        // FTS5 should no longer have it
        let count_after: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'delete_me'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after, 0, "FTS5 must be cleaned after chunk delete");
    }

    #[test]
    fn fts_trigger_fires_on_update() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        db.conn()
            .execute(
                "INSERT INTO chunks (id, file_path, start_line, end_line, byte_start, byte_end, \
                 symbol, kind, content, language, file_mtime, content_hash) \
                 VALUES ('c3', 'src/lib.rs', 1, 10, 0, 50, 'old_name', 'function', \
                 'fn old_name() {}', 'rust', 0, 'hash3')",
                [],
            )
            .unwrap();

        // Update the symbol
        db.conn()
            .execute(
                "UPDATE chunks SET symbol = 'new_name', content = 'fn new_name() {}' WHERE id = 'c3'",
                [],
            )
            .unwrap();

        // Old name should be gone from FTS5
        let old_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'old_name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            old_count, 0,
            "Old symbol must be removed from FTS5 after update"
        );

        // New name should be present
        let new_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'new_name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_count, 1, "New symbol must be in FTS5 after update");
    }

    #[test]
    fn init_schema_v2_to_v3_migration_backfills_chunks() {
        let db = Database::open_in_memory().unwrap();

        // Create v2 schema manually (without FTS5)
        db.conn()
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS chunks (
                    id             TEXT PRIMARY KEY,
                    file_path      TEXT NOT NULL,
                    start_line     INTEGER NOT NULL,
                    end_line       INTEGER NOT NULL,
                    byte_start     INTEGER NOT NULL,
                    byte_end       INTEGER NOT NULL,
                    symbol         TEXT,
                    kind           TEXT NOT NULL,
                    content        TEXT NOT NULL,
                    parent_context TEXT,
                    language       TEXT NOT NULL,
                    file_mtime     INTEGER NOT NULL,
                    content_hash   TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS files (
                    path       TEXT PRIMARY KEY,
                    mtime      INTEGER NOT NULL,
                    size       INTEGER NOT NULL,
                    hash       TEXT NOT NULL,
                    indexed_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS vectors_data (
                    chunk_id  TEXT PRIMARY KEY,
                    embedding TEXT NOT NULL,
                    FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS chunk_vec_map (
                    chunk_id  TEXT PRIMARY KEY,
                    vec_rowid INTEGER NOT NULL,
                    FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
                );
                ",
            )
            .unwrap();

        // Set user_version to 2 (v2 schema, pre-FTS5)
        db.conn().pragma_update(None, "user_version", 2).unwrap();

        // Insert a test chunk (simulating existing data before migration)
        db.conn()
            .execute(
                "INSERT INTO chunks (id, file_path, start_line, end_line, byte_start, byte_end, \
                 symbol, kind, content, language, file_mtime, content_hash) \
                 VALUES ('migrate_chunk', 'src/main.rs', 1, 20, 0, 100, 'main_fn', 'function', \
                 'fn main_fn() { println!(\"hello\"); }', 'rust', 0, 'hash_m')",
                [],
            )
            .unwrap();

        // Run init_schema — should migrate v2 → v3, creating FTS5 + backfilling
        db.init_schema(4).unwrap();

        // Verify user_version is now 4
        let version: u32 = db
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4, "Schema version must be 4 after v2→v3 migration");

        // Verify the existing chunk is findable via FTS5 (backfilled)
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'main_fn'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "Existing chunk must be backfilled into FTS5 during migration"
        );
    }

    #[test]
    fn init_schema_v3_is_idempotent() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        // Run init_schema again — must not error
        db.init_schema(4).unwrap();

        let version: u32 = db
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4, "Schema version must remain 4 after re-run");

        // FTS5 table must still exist
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='chunks_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "chunks_fts must still exist after idempotent re-run"
        );
    }
}
