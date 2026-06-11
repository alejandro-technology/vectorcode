use anyhow::Result;
use rusqlite::Connection;

use crate::VectorCodeError;

/// A tracked file record from the `files` table — spec §6, ST-7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    pub path: String,
    pub mtime: i64,
    pub size: i64,
    pub hash: String,
    pub indexed_at: i64,
}

/// Insert or update a file record.
pub fn upsert_file(
    conn: &Connection,
    path: &str,
    mtime: i64,
    size: i64,
    hash: &str,
    indexed_at: i64,
) -> Result<(), VectorCodeError> {
    conn.execute(
        "INSERT INTO files (path, mtime, size, hash, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(path) DO UPDATE SET
            mtime = excluded.mtime,
            size = excluded.size,
            hash = excluded.hash,
            indexed_at = excluded.indexed_at",
        (path, mtime, size, hash, indexed_at),
    )?;
    Ok(())
}

/// Get a file record by path, or None if not tracked.
pub fn get_file(conn: &Connection, path: &str) -> Result<Option<FileRecord>, VectorCodeError> {
    let mut stmt =
        conn.prepare("SELECT path, mtime, size, hash, indexed_at FROM files WHERE path = ?1")?;
    let mut rows = stmt.query_map([path], |row| {
        Ok(FileRecord {
            path: row.get(0)?,
            mtime: row.get(1)?,
            size: row.get(2)?,
            hash: row.get(3)?,
            indexed_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(Ok(record)) => Ok(Some(record)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// List all tracked files.
pub fn list_all_files(conn: &Connection) -> Result<Vec<FileRecord>, VectorCodeError> {
    let mut stmt =
        conn.prepare("SELECT path, mtime, size, hash, indexed_at FROM files ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        Ok(FileRecord {
            path: row.get(0)?,
            mtime: row.get(1)?,
            size: row.get(2)?,
            hash: row.get(3)?,
            indexed_at: row.get(4)?,
        })
    })?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Remove a file record by path.
pub fn remove_file(conn: &Connection, path: &str) -> Result<(), VectorCodeError> {
    conn.execute("DELETE FROM files WHERE path = ?1", [path])?;
    Ok(())
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
    fn upsert_and_get_file() {
        let db = setup_db();
        upsert_file(
            db.conn(),
            "src/main.rs",
            1718000000,
            1024,
            "abc123",
            1718000100,
        )
        .unwrap();

        let record = get_file(db.conn(), "src/main.rs").unwrap();
        assert!(record.is_some(), "File should exist after upsert");
        let record = record.unwrap();
        assert_eq!(record.path, "src/main.rs");
        assert_eq!(record.mtime, 1718000000);
        assert_eq!(record.size, 1024);
        assert_eq!(record.hash, "abc123");
        assert_eq!(record.indexed_at, 1718000100);
    }

    #[test]
    fn get_nonexistent_file_returns_none() {
        let db = setup_db();
        let record = get_file(db.conn(), "nonexistent.rs").unwrap();
        assert!(record.is_none(), "Nonexistent file must return None");
    }

    #[test]
    fn upsert_updates_existing_record() {
        let db = setup_db();
        upsert_file(db.conn(), "src/lib.rs", 1000, 500, "hash1", 1000).unwrap();
        upsert_file(db.conn(), "src/lib.rs", 2000, 600, "hash2", 2000).unwrap();

        let record = get_file(db.conn(), "src/lib.rs").unwrap().unwrap();
        assert_eq!(record.mtime, 2000, "mtime should be updated");
        assert_eq!(record.size, 600, "size should be updated");
        assert_eq!(record.hash, "hash2", "hash should be updated");
        assert_eq!(record.indexed_at, 2000, "indexed_at should be updated");
    }

    #[test]
    fn list_all_files_returns_all_records() {
        let db = setup_db();
        upsert_file(db.conn(), "src/b.rs", 100, 50, "h1", 100).unwrap();
        upsert_file(db.conn(), "src/a.rs", 200, 60, "h2", 200).unwrap();
        upsert_file(db.conn(), "src/c.rs", 300, 70, "h3", 300).unwrap();

        let files = list_all_files(db.conn()).unwrap();
        assert_eq!(files.len(), 3, "Should have 3 files");
        // Ordered by path
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!(files[1].path, "src/b.rs");
        assert_eq!(files[2].path, "src/c.rs");
    }

    #[test]
    fn list_all_files_empty_returns_empty_vec() {
        let db = setup_db();
        let files = list_all_files(db.conn()).unwrap();
        assert!(files.is_empty(), "Empty DB must return empty vec");
    }

    #[test]
    fn remove_file_deletes_record() {
        let db = setup_db();
        upsert_file(db.conn(), "src/del.rs", 100, 50, "h1", 100).unwrap();
        assert!(get_file(db.conn(), "src/del.rs").unwrap().is_some());

        remove_file(db.conn(), "src/del.rs").unwrap();
        assert!(
            get_file(db.conn(), "src/del.rs").unwrap().is_none(),
            "File must be gone after remove"
        );
    }

    #[test]
    fn remove_nonexistent_file_is_noop() {
        let db = setup_db();
        // Should not error
        remove_file(db.conn(), "does_not_exist.rs").unwrap();
    }
}
