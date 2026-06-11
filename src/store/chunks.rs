use std::collections::HashSet;

use anyhow::Result;
use rusqlite::Connection;

use crate::types::Chunk;
use crate::VectorCodeError;

/// Insert a chunk into the database.
///
/// Uses INSERT OR REPLACE to handle re-indexing of the same chunk ID.
pub fn insert_chunk(conn: &Connection, chunk: &Chunk) -> Result<(), VectorCodeError> {
    conn.execute(
        "INSERT OR REPLACE INTO chunks
         (id, file_path, start_line, end_line, byte_start, byte_end,
          symbol, kind, content, parent_context, language, file_mtime, content_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        (
            &chunk.id,
            &chunk.file_path,
            chunk.start_line,
            chunk.end_line,
            chunk.byte_start,
            chunk.byte_end,
            &chunk.symbol,
            &chunk.kind,
            &chunk.content,
            &chunk.parent_context,
            &chunk.language,
            chunk.file_mtime,
            &chunk.content_hash,
        ),
    )?;
    Ok(())
}

/// Get a chunk by ID, or None if not found.
pub fn get_chunk(conn: &Connection, id: &str) -> Result<Option<Chunk>, VectorCodeError> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, start_line, end_line, byte_start, byte_end,
                symbol, kind, content, parent_context, language, file_mtime, content_hash
         FROM chunks WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], row_to_chunk)?;
    match rows.next() {
        Some(Ok(chunk)) => Ok(Some(chunk)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Delete a chunk by ID. Also deletes associated vector data.
pub fn delete_chunk(conn: &Connection, id: &str) -> Result<(), VectorCodeError> {
    // Delete vector data first (foreign key may not cascade if FK enforcement is off)
    // Use the vectors module to handle both vec_chunks and vectors_data paths
    crate::store::vectors::delete_vectors_for_chunk(conn, id)?;
    conn.execute("DELETE FROM chunks WHERE id = ?1", [id])?;
    Ok(())
}

/// Delete all chunks (and associated vectors) for a given file path.
///
/// Returns the number of chunks deleted.
pub fn delete_chunks_for_file(
    conn: &Connection,
    file_path: &str,
) -> Result<usize, VectorCodeError> {
    let has_vec = crate::store::vectors::has_vec_extension(conn);

    if has_vec {
        // Delete vec_chunks rows via chunk_vec_map mapping
        conn.execute(
            "DELETE FROM vec_chunks WHERE rowid IN (
                SELECT vec_rowid FROM chunk_vec_map
                WHERE chunk_id IN (SELECT id FROM chunks WHERE file_path = ?1)
            )",
            [file_path],
        )?;
        conn.execute(
            "DELETE FROM chunk_vec_map WHERE chunk_id IN (SELECT id FROM chunks WHERE file_path = ?1)",
            [file_path],
        )?;
    } else {
        conn.execute(
            "DELETE FROM vectors_data WHERE chunk_id IN (SELECT id FROM chunks WHERE file_path = ?1)",
            [file_path],
        )?;
    }

    let count = conn.execute("DELETE FROM chunks WHERE file_path = ?1", [file_path])?;
    Ok(count)
}

/// List all chunks for a given file path.
pub fn list_chunks_by_file(
    conn: &Connection,
    file_path: &str,
) -> Result<Vec<Chunk>, VectorCodeError> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, start_line, end_line, byte_start, byte_end,
                symbol, kind, content, parent_context, language, file_mtime, content_hash
         FROM chunks WHERE file_path = ?1 ORDER BY start_line",
    )?;
    let rows = stmt.query_map([file_path], row_to_chunk)?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Check if a chunk exists with the given content hash (for incremental indexing).
///
/// Returns true if a chunk with this ID exists AND has the same content_hash.
/// This allows skipping re-embedding for unchanged chunks.
pub fn chunk_exists_with_hash(
    conn: &Connection,
    id: &str,
    content_hash: &str,
) -> Result<bool, VectorCodeError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks WHERE id = ?1 AND content_hash = ?2",
        (id, content_hash),
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Delete chunks for files that are NOT in the given set of valid paths.
///
/// Used during incremental sync to clean up chunks from deleted files.
/// Returns the number of chunks deleted.
pub fn delete_stale_chunks(
    conn: &Connection,
    valid_paths: &HashSet<String>,
) -> Result<usize, VectorCodeError> {
    // Get all distinct file paths in chunks
    let mut stmt = conn.prepare("SELECT DISTINCT file_path FROM chunks")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut all_paths: Vec<String> = Vec::new();
    for row in rows {
        all_paths.push(row?);
    }

    let mut deleted = 0;
    let has_vec = crate::store::vectors::has_vec_extension(conn);
    for path in &all_paths {
        if !valid_paths.contains(path) {
            // Delete vectors for chunks in this file
            if has_vec {
                // Delete vec_chunks rows via chunk_vec_map mapping
                conn.execute(
                    "DELETE FROM vec_chunks WHERE rowid IN (
                        SELECT vec_rowid FROM chunk_vec_map
                        WHERE chunk_id IN (SELECT id FROM chunks WHERE file_path = ?1)
                    )",
                    [path],
                )?;
                conn.execute(
                    "DELETE FROM chunk_vec_map WHERE chunk_id IN (SELECT id FROM chunks WHERE file_path = ?1)",
                    [path],
                )?;
            } else {
                conn.execute(
                    "DELETE FROM vectors_data WHERE chunk_id IN (SELECT id FROM chunks WHERE file_path = ?1)",
                    [path],
                )?;
            }
            let count = conn.execute("DELETE FROM chunks WHERE file_path = ?1", [path])?;
            deleted += count;
        }
    }

    // Safety net: clean up any orphaned vec_chunks rows whose chunk_id no longer
    // exists in chunks (catches edge cases from partial failures or prior bugs)
    if has_vec {
        conn.execute(
            "DELETE FROM vec_chunks WHERE rowid IN (
                SELECT vec_rowid FROM chunk_vec_map WHERE chunk_id NOT IN (
                    SELECT id FROM chunks
                )
            )",
            [],
        )?;
    }

    Ok(deleted)
}

/// Map a rusqlite Row to a Chunk struct.
fn row_to_chunk(row: &rusqlite::Row) -> rusqlite::Result<Chunk> {
    Ok(Chunk {
        id: row.get(0)?,
        file_path: row.get(1)?,
        start_line: row.get(2)?,
        end_line: row.get(3)?,
        byte_start: row.get(4)?,
        byte_end: row.get(5)?,
        symbol: row.get(6)?,
        kind: row.get(7)?,
        content: row.get(8)?,
        parent_context: row.get(9)?,
        language: row.get(10)?,
        file_mtime: row.get(11)?,
        content_hash: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::Database;
    use crate::types::{compute_chunk_id, compute_content_hash};

    fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(384).unwrap();
        db
    }

    fn make_chunk(file_path: &str, byte_start: u32, byte_end: u32, content: &str) -> Chunk {
        Chunk {
            id: compute_chunk_id(file_path, byte_start, byte_end),
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: 10,
            byte_start,
            byte_end,
            symbol: Some("test_fn".to_string()),
            kind: "function_declaration".to_string(),
            content: content.to_string(),
            parent_context: None,
            language: "typescript".to_string(),
            file_mtime: 1718000000,
            content_hash: compute_content_hash(content),
        }
    }

    #[test]
    fn insert_and_get_chunk() {
        let db = setup_db();
        let chunk = make_chunk("src/test.ts", 0, 200, "function test_fn() {}");
        insert_chunk(db.conn(), &chunk).unwrap();

        let retrieved = get_chunk(db.conn(), &chunk.id).unwrap();
        assert!(retrieved.is_some(), "Chunk should exist after insert");
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, chunk.id);
        assert_eq!(retrieved.file_path, "src/test.ts");
        assert_eq!(retrieved.content, "function test_fn() {}");
        assert_eq!(retrieved.symbol, Some("test_fn".to_string()));
        assert_eq!(retrieved.language, "typescript");
    }

    #[test]
    fn get_nonexistent_chunk_returns_none() {
        let db = setup_db();
        let result = get_chunk(db.conn(), "nonexistent_id").unwrap();
        assert!(result.is_none(), "Nonexistent chunk must return None");
    }

    #[test]
    fn insert_chunk_with_null_symbol_and_parent() {
        let db = setup_db();
        let mut chunk = make_chunk("src/test.ts", 0, 100, "let x = 1;");
        chunk.symbol = None;
        chunk.parent_context = None;
        insert_chunk(db.conn(), &chunk).unwrap();

        let retrieved = get_chunk(db.conn(), &chunk.id).unwrap().unwrap();
        assert!(retrieved.symbol.is_none(), "Symbol should be None");
        assert!(
            retrieved.parent_context.is_none(),
            "Parent context should be None"
        );
    }

    #[test]
    fn delete_chunk_removes_it() {
        let db = setup_db();
        let chunk = make_chunk("src/del.ts", 0, 100, "fn delete_me()");
        insert_chunk(db.conn(), &chunk).unwrap();
        assert!(get_chunk(db.conn(), &chunk.id).unwrap().is_some());

        delete_chunk(db.conn(), &chunk.id).unwrap();
        assert!(
            get_chunk(db.conn(), &chunk.id).unwrap().is_none(),
            "Chunk must be gone after delete"
        );
    }

    #[test]
    fn list_chunks_by_file_returns_ordered() {
        let db = setup_db();
        let c1 = make_chunk("src/multi.ts", 0, 100, "fn first()");
        let c2 = Chunk {
            id: compute_chunk_id("src/multi.ts", 100, 200),
            start_line: 11,
            end_line: 20,
            byte_start: 100,
            byte_end: 200,
            ..make_chunk("src/multi.ts", 100, 200, "fn second()")
        };
        let c3 = Chunk {
            id: compute_chunk_id("src/multi.ts", 200, 300),
            start_line: 21,
            end_line: 30,
            byte_start: 200,
            byte_end: 300,
            ..make_chunk("src/multi.ts", 200, 300, "fn third()")
        };
        insert_chunk(db.conn(), &c1).unwrap();
        insert_chunk(db.conn(), &c2).unwrap();
        insert_chunk(db.conn(), &c3).unwrap();

        // Also insert a chunk for a different file
        let other = make_chunk("src/other.ts", 0, 50, "fn other()");
        insert_chunk(db.conn(), &other).unwrap();

        let chunks = list_chunks_by_file(db.conn(), "src/multi.ts").unwrap();
        assert_eq!(chunks.len(), 3, "Should have 3 chunks for this file");
        assert_eq!(chunks[0].byte_start, 0);
        assert_eq!(chunks[1].byte_start, 100);
        assert_eq!(chunks[2].byte_start, 200);
    }

    #[test]
    fn list_chunks_for_unknown_file_returns_empty() {
        let db = setup_db();
        let chunks = list_chunks_by_file(db.conn(), "no_such_file.ts").unwrap();
        assert!(chunks.is_empty(), "Unknown file must return empty vec");
    }

    #[test]
    fn chunk_exists_with_hash_true_when_match() {
        let db = setup_db();
        let chunk = make_chunk("src/hash.ts", 0, 100, "content");
        insert_chunk(db.conn(), &chunk).unwrap();

        assert!(
            chunk_exists_with_hash(db.conn(), &chunk.id, &chunk.content_hash).unwrap(),
            "Should find chunk with matching hash"
        );
    }

    #[test]
    fn chunk_exists_with_hash_false_when_hash_differs() {
        let db = setup_db();
        let chunk = make_chunk("src/hash.ts", 0, 100, "content");
        insert_chunk(db.conn(), &chunk).unwrap();

        assert!(
            !chunk_exists_with_hash(db.conn(), &chunk.id, "different_hash").unwrap(),
            "Should not match when hash differs"
        );
    }

    #[test]
    fn chunk_exists_with_hash_false_when_id_missing() {
        let db = setup_db();
        assert!(
            !chunk_exists_with_hash(db.conn(), "no_such_id", "any_hash").unwrap(),
            "Should return false for nonexistent ID"
        );
    }

    #[test]
    fn delete_stale_chunks_removes_orphaned_files() {
        let db = setup_db();
        let c1 = make_chunk("src/keep.ts", 0, 100, "keep me");
        let c2 = make_chunk("src/delete_me.ts", 0, 100, "delete me");
        insert_chunk(db.conn(), &c1).unwrap();
        insert_chunk(db.conn(), &c2).unwrap();

        let valid_paths: HashSet<String> = ["src/keep.ts".to_string()].into_iter().collect();
        let deleted = delete_stale_chunks(db.conn(), &valid_paths).unwrap();
        assert_eq!(deleted, 1, "Should delete 1 stale chunk");

        assert!(
            get_chunk(db.conn(), &c1.id).unwrap().is_some(),
            "Kept chunk must remain"
        );
        assert!(
            get_chunk(db.conn(), &c2.id).unwrap().is_none(),
            "Stale chunk must be gone"
        );
    }

    #[test]
    fn delete_stale_chunks_with_all_valid_deletes_nothing() {
        let db = setup_db();
        let c1 = make_chunk("src/a.ts", 0, 100, "a");
        let c2 = make_chunk("src/b.ts", 0, 100, "b");
        insert_chunk(db.conn(), &c1).unwrap();
        insert_chunk(db.conn(), &c2).unwrap();

        let valid_paths: HashSet<String> = ["src/a.ts".to_string(), "src/b.ts".to_string()]
            .into_iter()
            .collect();
        let deleted = delete_stale_chunks(db.conn(), &valid_paths).unwrap();
        assert_eq!(
            deleted, 0,
            "No chunks should be deleted when all paths valid"
        );
    }

    #[test]
    fn insert_chunk_replaces_existing() {
        let db = setup_db();
        let chunk = make_chunk("src/replace.ts", 0, 100, "original");
        insert_chunk(db.conn(), &chunk).unwrap();

        let updated = Chunk {
            content: "updated content".to_string(),
            content_hash: compute_content_hash("updated content"),
            ..chunk.clone()
        };
        insert_chunk(db.conn(), &updated).unwrap();

        let retrieved = get_chunk(db.conn(), &chunk.id).unwrap().unwrap();
        assert_eq!(retrieved.content, "updated content");
    }
}
