use anyhow::Result;
use rusqlite::Connection;

use crate::VectorCodeError;

/// Current schema version — bump when migrating.
const SCHEMA_VERSION: u32 = 1;

/// SQLite database wrapper with WAL mode and schema management.
///
/// Spec §6: single file at `.vectorcode/index.db`, WAL mode (ST-1, ST-6).
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) a database at the given path with WAL mode.
    pub fn open(path: &std::path::Path) -> Result<Self, VectorCodeError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, VectorCodeError> {
        let conn = Connection::open_in_memory()?;
        Ok(Self { conn })
    }

    /// Initialize the full schema per spec §6.
    ///
    /// Creates `meta`, `chunks`, `files`, and `vectors_data` tables.
    /// The `vec_chunks` virtual table (sqlite-vec) is attempted; if the
    /// extension is not loaded, we fall back to `vectors_data` (ST-5 fallback).
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
            -- TODO: Replace with sqlite-vec virtual table when extension is loaded.
            CREATE TABLE IF NOT EXISTS vectors_data (
                chunk_id  TEXT PRIMARY KEY,
                embedding TEXT NOT NULL,
                FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
            );
            ",
        )?;

        // Attempt to create the sqlite-vec virtual table.
        // This will fail gracefully if the extension is not loaded.
        let vec_sql = format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(\
                chunk_id TEXT PRIMARY KEY,\
                embedding float[{dims}]\
            )"
        );
        let _vec_result = self.conn.execute_batch(&vec_sql);
        // We intentionally ignore errors here — the fallback table handles it.

        // Set schema version
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;

        Ok(())
    }

    /// Check whether the sqlite-vec extension is available.
    pub fn has_vec_extension(&self) -> bool {
        self.conn
            .prepare("SELECT 1 FROM vec_chunks LIMIT 0")
            .is_ok()
    }

    /// Get a reference to the underlying connection (for CRUD modules).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Get a mutable reference to the underlying connection.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
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
        assert_eq!(version, 1, "Schema version must be 1 after init");
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
        assert_eq!(version, 1);
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
}
