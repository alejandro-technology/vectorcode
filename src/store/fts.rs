//! FTS5 sparse search — lexical search over indexed code chunks.
//!
//! Provides `sanitize_fts_query` to strip FTS5 special characters and
//! `search_sparse` to execute bm25-weighted FTS5 MATCH queries.

use rusqlite::Connection;

use crate::error::VectorCodeError;
use crate::types::SearchResult;

/// Strip FTS5 special characters from a query string.
///
/// FTS5 interprets `*`, `^`, `"`, `(`, `)`, `:`, `;` as operators.
/// If left in user queries, they cause MATCH syntax errors.
/// After stripping, collapse whitespace and trim.
pub(crate) fn sanitize_fts_query(query: &str) -> String {
    let stripped: String = query
        .chars()
        .filter(|c| !matches!(c, '*' | '^' | '"' | '(' | ')' | ':' | ';'))
        .collect();

    // Collapse whitespace and trim
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Execute a sparse FTS5 search with bm25 ranking.
///
/// Sanitizes the query, executes a MATCH against `chunks_fts`, JOINs back
/// to `chunks` for full metadata, and normalizes bm25 scores to [0, 1).
///
/// bm25 returns negative values (lower = better). Normalization:
/// `(-bm25) / (1 - bm25)` maps to [0, 1) where higher = better,
/// matching the `SearchResult.score` contract.
pub(crate) fn search_sparse(
    conn: &Connection,
    query: &str,
    limit: usize,
    language: Option<&str>,
    path_filter: Option<&str>,
) -> Result<Vec<SearchResult>, VectorCodeError> {
    let sanitized = sanitize_fts_query(query);
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }

    let sql = r#"
        SELECT c.file_path, c.start_line, c.end_line, c.symbol, c.kind,
               c.content, c.parent_context, c.language,
               (-bm25(chunks_fts, 10.0, 5.0, 2.0, 1.0))
               / (1.0 - bm25(chunks_fts, 10.0, 5.0, 2.0, 1.0)) AS score
        FROM chunks_fts
        JOIN chunks c ON c.rowid = chunks_fts.rowid
        WHERE chunks_fts MATCH ?1
          AND (?2 IS NULL OR c.language = ?2)
          AND (?3 IS NULL OR c.file_path LIKE ?3 ESCAPE '\')
        ORDER BY score DESC
        LIMIT ?4
    "#;

    let path_like = path_filter.map(|p| {
        let escaped = p
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("{escaped}%")
    });

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        rusqlite::params![sanitized, language, path_like, limit],
        |row| {
            Ok(SearchResult {
                file_path: row.get(0)?,
                start_line: row.get(1)?,
                end_line: row.get(2)?,
                symbol: row.get(3)?,
                kind: row.get(4)?,
                content: row.get(5)?,
                parent_context: row.get(6)?,
                language: row.get(7)?,
                score: row.get(8)?,
            })
        },
    )?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── sanitize_fts_query tests ──────────────────────────────────────

    #[test]
    fn sanitize_strips_asterisk() {
        assert_eq!(sanitize_fts_query("foo*bar"), "foobar");
    }

    #[test]
    fn sanitize_strips_caret() {
        assert_eq!(sanitize_fts_query("foo^bar"), "foobar");
    }

    #[test]
    fn sanitize_strips_double_quotes() {
        assert_eq!(sanitize_fts_query(r#"foo"bar""#), "foobar");
    }

    #[test]
    fn sanitize_strips_parentheses() {
        assert_eq!(sanitize_fts_query("foo(bar)"), "foobar");
    }

    #[test]
    fn sanitize_strips_colon() {
        assert_eq!(sanitize_fts_query("foo:bar"), "foobar");
    }

    #[test]
    fn sanitize_strips_semicolon() {
        assert_eq!(sanitize_fts_query("foo;bar"), "foobar");
    }

    #[test]
    fn sanitize_strips_all_special_chars_combined() {
        assert_eq!(sanitize_fts_query(r#"foo*^"();:bar"#), "foobar");
    }

    #[test]
    fn sanitize_collapses_whitespace_after_stripping() {
        // Special chars surrounded by spaces → spaces collapse
        assert_eq!(sanitize_fts_query("foo * * * bar"), "foo bar");
    }

    #[test]
    fn sanitize_special_chars_between_words_with_spaces() {
        // When special chars are between words with spaces, words remain separate
        assert_eq!(sanitize_fts_query("foo * bar"), "foo bar");
    }

    #[test]
    fn sanitize_empty_string_returns_empty() {
        assert_eq!(sanitize_fts_query(""), "");
    }

    #[test]
    fn sanitize_only_special_chars_returns_empty() {
        assert_eq!(sanitize_fts_query("*^\"():;"), "");
    }

    #[test]
    fn sanitize_normal_query_unchanged() {
        assert_eq!(sanitize_fts_query("authenticate user"), "authenticate user");
    }

    #[test]
    fn sanitize_preserves_single_word() {
        assert_eq!(sanitize_fts_query("authenticateUser"), "authenticateUser");
    }

    #[test]
    fn sanitize_trims_leading_trailing_whitespace() {
        assert_eq!(sanitize_fts_query("  hello world  "), "hello world");
    }

    // ─── search_sparse tests (need DB with FTS5 schema) ───────────────

    fn setup_db_with_fts() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Create chunks table
        conn.execute_batch(
            "CREATE TABLE chunks (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                byte_start INTEGER NOT NULL,
                byte_end INTEGER NOT NULL,
                symbol TEXT,
                kind TEXT NOT NULL,
                content TEXT NOT NULL,
                parent_context TEXT,
                language TEXT NOT NULL,
                file_mtime INTEGER NOT NULL,
                content_hash TEXT NOT NULL
            )",
        )
        .unwrap();

        // Create FTS5 virtual table (matching db.rs schema)
        conn.execute_batch(
            "CREATE VIRTUAL TABLE chunks_fts USING fts5(
                symbol, content, file_path, language,
                content='chunks', content_rowid='rowid',
                tokenize='unicode61 remove_diacritics 1'
            )",
        )
        .unwrap();

        // Create triggers
        conn.execute_batch(
            "CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, symbol, content, file_path, language)
                VALUES (new.rowid, COALESCE(new.symbol, ''), new.content, new.file_path, new.language);
            END;
            CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, symbol, content, file_path, language)
                VALUES ('delete', old.rowid, COALESCE(old.symbol, ''), old.content, old.file_path, old.language);
            END;
            CREATE TRIGGER chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, symbol, content, file_path, language)
                VALUES ('delete', old.rowid, COALESCE(old.symbol, ''), old.content, old.file_path, old.language);
                INSERT INTO chunks_fts(rowid, symbol, content, file_path, language)
                VALUES (new.rowid, COALESCE(new.symbol, ''), new.content, new.file_path, new.language);
            END;",
        )
        .unwrap();

        conn
    }

    fn insert_chunk(
        conn: &Connection,
        id: &str,
        file_path: &str,
        symbol: Option<&str>,
        content: &str,
        language: &str,
    ) {
        conn.execute(
            "INSERT INTO chunks (id, file_path, start_line, end_line, byte_start, byte_end, symbol, kind, content, parent_context, language, file_mtime, content_hash)
             VALUES (?1, ?2, 1, 10, 0, 100, ?3, 'function_declaration', ?4, NULL, ?5, 1718000000, 'hash')",
            rusqlite::params![id, file_path, symbol, content, language],
        )
        .unwrap();
    }

    #[test]
    fn search_sparse_empty_query_returns_empty() {
        let conn = setup_db_with_fts();
        let results = search_sparse(&conn, "", 10, None, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_sparse_only_special_chars_returns_empty() {
        let conn = setup_db_with_fts();
        insert_chunk(
            &conn,
            "c1",
            "src/auth.ts",
            Some("authenticateUser"),
            "function authenticateUser() {}",
            "typescript",
        );
        let results = search_sparse(&conn, "*^\"():;", 10, None, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_sparse_finds_chunk_by_symbol() {
        let conn = setup_db_with_fts();
        insert_chunk(
            &conn,
            "c1",
            "src/auth.ts",
            Some("authenticateUser"),
            "function authenticateUser() {}",
            "typescript",
        );

        let results = search_sparse(&conn, "authenticateUser", 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "src/auth.ts");
        assert_eq!(results[0].symbol.as_deref(), Some("authenticateUser"));
        assert!(
            results[0].score >= 0.0 && results[0].score < 1.0,
            "Score should be normalized to [0,1), got {}",
            results[0].score
        );
    }

    #[test]
    fn search_sparse_finds_chunk_by_content() {
        let conn = setup_db_with_fts();
        insert_chunk(
            &conn,
            "c1",
            "src/pay.ts",
            Some("handlePayment"),
            "function handlePayment() { processCharge(); }",
            "typescript",
        );

        let results = search_sparse(&conn, "processCharge", 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].content,
            "function handlePayment() { processCharge(); }"
        );
    }

    #[test]
    fn search_sparse_filters_by_language() {
        let conn = setup_db_with_fts();
        insert_chunk(
            &conn,
            "c1",
            "src/a.ts",
            Some("handler"),
            "function handler() {}",
            "typescript",
        );
        insert_chunk(
            &conn,
            "c2",
            "src/b.py",
            Some("handler"),
            "def handler(): pass",
            "python",
        );

        let results = search_sparse(&conn, "handler", 10, Some("python"), None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].language, "python");
    }

    #[test]
    fn search_sparse_filters_by_path() {
        let conn = setup_db_with_fts();
        insert_chunk(
            &conn,
            "c1",
            "src/auth/login.ts",
            Some("login"),
            "function login() {}",
            "typescript",
        );
        insert_chunk(
            &conn,
            "c2",
            "src/pay/charge.ts",
            Some("charge"),
            "function charge() {}",
            "typescript",
        );

        let results = search_sparse(&conn, "function", 10, None, Some("src/auth")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "src/auth/login.ts");
    }

    #[test]
    fn search_sparse_respects_limit() {
        let conn = setup_db_with_fts();
        for i in 0..5 {
            insert_chunk(
                &conn,
                &format!("c{i}"),
                &format!("src/file_{i}.ts"),
                Some("handler"),
                &format!("function handler_{i}() {{ /* handler number {i} */ }}"),
                "typescript",
            );
        }

        let results = search_sparse(&conn, "handler", 2, None, None).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_sparse_sanitizes_special_chars_in_query() {
        let conn = setup_db_with_fts();
        insert_chunk(
            &conn,
            "c1",
            "src/auth.ts",
            Some("authenticateUser"),
            "function authenticateUser() {}",
            "typescript",
        );

        // This would crash FTS5 without sanitization
        let results = search_sparse(&conn, "authenticateUser*", 10, None, None).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_sparse_score_is_normalized() {
        let conn = setup_db_with_fts();
        insert_chunk(
            &conn,
            "c1",
            "src/auth.ts",
            Some("authenticateUser"),
            "function authenticateUser() {}",
            "typescript",
        );

        let results = search_sparse(&conn, "authenticateUser", 10, None, None).unwrap();
        assert!(!results.is_empty());
        // bm25 returns negative values; our normalization maps to [0, 1)
        for r in &results {
            assert!(r.score >= 0.0, "Score should be >= 0, got {}", r.score);
            assert!(r.score < 1.0, "Score should be < 1, got {}", r.score);
        }
    }
}
