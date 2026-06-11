//! Meta table operations — read/write index metadata (spec §5.2).
//!
//! The `meta` table is a key-value store. `write_index_meta` writes all
//! fields of `IndexMeta` as individual rows. `read_index_meta` reads them back.

use anyhow::Result;
use rusqlite::Connection;

use crate::types::IndexMeta;
use crate::VectorCodeError;

/// Keys used in the meta table.
const KEY_PROVIDER: &str = "provider";
const KEY_MODEL: &str = "model";
const KEY_DIMENSIONS: &str = "dimensions";
const KEY_CREATED_AT: &str = "created_at";
const KEY_LAST_SYNC_AT: &str = "last_sync_at";
const KEY_FILES_INDEXED: &str = "files_indexed";
const KEY_CHUNKS_STORED: &str = "chunks_stored";
const KEY_VECTORCODE_VERSION: &str = "vectorcode_version";

/// Write a single key-value pair to the meta table.
pub fn write_meta(conn: &Connection, key: &str, value: &str) -> Result<(), VectorCodeError> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        (key, value),
    )?;
    Ok(())
}

/// Read a single value from the meta table, or None if the key doesn't exist.
pub fn read_meta(conn: &Connection, key: &str) -> Result<Option<String>, VectorCodeError> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query_map([key], |row| row.get(0))?;
    match rows.next() {
        Some(Ok(value)) => Ok(Some(value)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Write a full IndexMeta to the meta table.
pub fn write_index_meta(conn: &Connection, meta: &IndexMeta) -> Result<(), VectorCodeError> {
    write_meta(conn, KEY_PROVIDER, &meta.provider)?;
    write_meta(conn, KEY_MODEL, &meta.model)?;
    write_meta(conn, KEY_DIMENSIONS, &meta.dimensions.to_string())?;
    write_meta(conn, KEY_CREATED_AT, &meta.created_at)?;
    write_meta(
        conn,
        KEY_LAST_SYNC_AT,
        meta.last_sync_at.as_deref().unwrap_or(""),
    )?;
    write_meta(conn, KEY_FILES_INDEXED, &meta.files_indexed.to_string())?;
    write_meta(conn, KEY_CHUNKS_STORED, &meta.chunks_stored.to_string())?;
    write_meta(conn, KEY_VECTORCODE_VERSION, &meta.vectorcode_version)?;
    Ok(())
}

/// Read a full IndexMeta from the meta table.
///
/// Returns `None` if the provider key is missing (index not initialized).
/// Returns an error if required keys are missing but provider exists (corrupt meta).
pub fn read_index_meta(conn: &Connection) -> Result<Option<IndexMeta>, VectorCodeError> {
    let provider = match read_meta(conn, KEY_PROVIDER)? {
        Some(v) => v,
        None => return Ok(None),
    };

    let model = read_meta_required(conn, KEY_MODEL)?;
    let dims_str = read_meta_required(conn, KEY_DIMENSIONS)?;
    let dimensions: u32 = dims_str
        .parse()
        .map_err(|_| VectorCodeError::EmbedderError {
            message: format!("Invalid dimensions in meta: {dims_str}"),
        })?;
    let created_at = read_meta_required(conn, KEY_CREATED_AT)?;
    let last_sync_at = read_meta(conn, KEY_LAST_SYNC_AT)?.filter(|v| !v.is_empty());
    let files_str = read_meta_required(conn, KEY_FILES_INDEXED)?;
    let files_indexed: u32 = files_str
        .parse()
        .map_err(|_| VectorCodeError::EmbedderError {
            message: format!("Invalid files_indexed in meta: {files_str}"),
        })?;
    let chunks_str = read_meta_required(conn, KEY_CHUNKS_STORED)?;
    let chunks_stored: u32 = chunks_str
        .parse()
        .map_err(|_| VectorCodeError::EmbedderError {
            message: format!("Invalid chunks_stored in meta: {chunks_str}"),
        })?;
    let vectorcode_version = read_meta_required(conn, KEY_VECTORCODE_VERSION)?;

    Ok(Some(IndexMeta {
        provider,
        model,
        dimensions,
        created_at,
        last_sync_at,
        files_indexed,
        chunks_stored,
        vectorcode_version,
    }))
}

/// Update the statistics fields in meta (files_indexed, chunks_stored, last_sync_at).
pub fn update_meta_stats(
    conn: &Connection,
    files_indexed: u32,
    chunks_stored: u32,
    last_sync_at: &str,
) -> Result<(), VectorCodeError> {
    write_meta(conn, KEY_FILES_INDEXED, &files_indexed.to_string())?;
    write_meta(conn, KEY_CHUNKS_STORED, &chunks_stored.to_string())?;
    write_meta(conn, KEY_LAST_SYNC_AT, last_sync_at)?;
    Ok(())
}

/// Count total chunks in the database.
pub fn count_chunks(conn: &Connection) -> Result<u32, VectorCodeError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
    Ok(count as u32)
}

/// Count total tracked files in the database.
pub fn count_files(conn: &Connection) -> Result<u32, VectorCodeError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
    Ok(count as u32)
}

/// Helper: read a required meta key, returning an error if missing.
fn read_meta_required(conn: &Connection, key: &str) -> Result<String, VectorCodeError> {
    read_meta(conn, key)?.ok_or_else(|| VectorCodeError::EmbedderError {
        message: format!("Missing required meta key: {key}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::Database;

    fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(384).unwrap();
        db
    }

    #[test]
    fn write_and_read_meta() {
        let db = setup_db();
        write_meta(db.conn(), "test_key", "test_value").unwrap();
        let value = read_meta(db.conn(), "test_key").unwrap();
        assert_eq!(value, Some("test_value".to_string()));
    }

    #[test]
    fn read_missing_meta_returns_none() {
        let db = setup_db();
        let value = read_meta(db.conn(), "nonexistent").unwrap();
        assert!(value.is_none(), "Missing key must return None");
    }

    #[test]
    fn write_meta_overwrites_existing() {
        let db = setup_db();
        write_meta(db.conn(), "key", "first").unwrap();
        write_meta(db.conn(), "key", "second").unwrap();
        let value = read_meta(db.conn(), "key").unwrap();
        assert_eq!(value, Some("second".to_string()));
    }

    #[test]
    fn write_and_read_index_meta_roundtrip() {
        let db = setup_db();
        let meta = IndexMeta {
            provider: "onnx".to_string(),
            model: "all-MiniLM-L6-v2".to_string(),
            dimensions: 384,
            created_at: "2026-06-10T20:00:00Z".to_string(),
            last_sync_at: Some("2026-06-10T20:05:00Z".to_string()),
            files_indexed: 42,
            chunks_stored: 200,
            vectorcode_version: "0.1.0".to_string(),
        };

        write_index_meta(db.conn(), &meta).unwrap();
        let read_back = read_index_meta(db.conn()).unwrap().unwrap();

        assert_eq!(read_back.provider, "onnx");
        assert_eq!(read_back.model, "all-MiniLM-L6-v2");
        assert_eq!(read_back.dimensions, 384);
        assert_eq!(read_back.created_at, "2026-06-10T20:00:00Z");
        assert_eq!(
            read_back.last_sync_at,
            Some("2026-06-10T20:05:00Z".to_string())
        );
        assert_eq!(read_back.files_indexed, 42);
        assert_eq!(read_back.chunks_stored, 200);
        assert_eq!(read_back.vectorcode_version, "0.1.0");
    }

    #[test]
    fn read_index_meta_empty_db_returns_none() {
        let db = setup_db();
        let result = read_index_meta(db.conn()).unwrap();
        assert!(result.is_none(), "Empty meta table must return None");
    }

    #[test]
    fn write_index_meta_with_no_last_sync() {
        let db = setup_db();
        let meta = IndexMeta {
            provider: "gemini".to_string(),
            model: "gemini-embedding-001".to_string(),
            dimensions: 768,
            created_at: "2026-06-10T20:00:00Z".to_string(),
            last_sync_at: None,
            files_indexed: 0,
            chunks_stored: 0,
            vectorcode_version: "0.1.0".to_string(),
        };

        write_index_meta(db.conn(), &meta).unwrap();
        let read_back = read_index_meta(db.conn()).unwrap().unwrap();
        assert!(read_back.last_sync_at.is_none());
    }

    #[test]
    fn update_meta_stats_changes_values() {
        let db = setup_db();
        let meta = IndexMeta {
            provider: "onnx".to_string(),
            model: "all-MiniLM-L6-v2".to_string(),
            dimensions: 384,
            created_at: "2026-06-10T20:00:00Z".to_string(),
            last_sync_at: None,
            files_indexed: 0,
            chunks_stored: 0,
            vectorcode_version: "0.1.0".to_string(),
        };
        write_index_meta(db.conn(), &meta).unwrap();

        update_meta_stats(db.conn(), 10, 50, "2026-06-10T21:00:00Z").unwrap();

        let read_back = read_index_meta(db.conn()).unwrap().unwrap();
        assert_eq!(read_back.files_indexed, 10);
        assert_eq!(read_back.chunks_stored, 50);
        assert_eq!(
            read_back.last_sync_at,
            Some("2026-06-10T21:00:00Z".to_string())
        );
        // created_at unchanged
        assert_eq!(read_back.created_at, "2026-06-10T20:00:00Z");
    }

    #[test]
    fn count_chunks_returns_correct_count() {
        let db = setup_db();
        assert_eq!(count_chunks(db.conn()).unwrap(), 0);

        let chunk = crate::types::Chunk {
            id: "test1".to_string(),
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 10,
            byte_start: 0,
            byte_end: 100,
            symbol: None,
            kind: "function_item".to_string(),
            content: "fn test() {}".to_string(),
            parent_context: None,
            language: "rust".to_string(),
            file_mtime: 1000,
            content_hash: "abc".to_string(),
        };
        crate::store::chunks::insert_chunk(db.conn(), &chunk).unwrap();
        assert_eq!(count_chunks(db.conn()).unwrap(), 1);
    }

    #[test]
    fn count_files_returns_correct_count() {
        let db = setup_db();
        assert_eq!(count_files(db.conn()).unwrap(), 0);

        crate::store::files::upsert_file(db.conn(), "a.rs", 100, 50, "h1", 100).unwrap();
        crate::store::files::upsert_file(db.conn(), "b.rs", 200, 60, "h2", 200).unwrap();
        assert_eq!(count_files(db.conn()).unwrap(), 2);
    }
}
