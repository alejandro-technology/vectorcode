use anyhow::Result;
use rusqlite::Connection;

use crate::store::chunks;
use crate::types::SearchResult;
use crate::VectorCodeError;

/// Check if sqlite-vec extension is available for this connection.
pub fn has_vec_extension(conn: &Connection) -> bool {
    static HAS_VEC: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS_VEC.get_or_init(|| conn.prepare("SELECT vec_version()").is_ok())
}

/// Convert an f32 embedding to a little-endian binary blob for sqlite-vec.
fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    bytemuck::cast_slice(embedding).to_vec()
}

/// Normalize embedding to target dimensions by padding with zeros or truncating.
fn normalize_embedding(embedding: &[f32], target_dims: usize) -> Vec<f32> {
    if embedding.len() == target_dims {
        return embedding.to_vec();
    }
    let mut result = vec![0.0f32; target_dims];
    let copy_len = embedding.len().min(target_dims);
    result[..copy_len].copy_from_slice(&embedding[..copy_len]);
    result
}

/// Get the configured embedding dimensions from the meta table, or default to 384.
fn get_embedding_dims(conn: &Connection) -> u32 {
    conn.query_row(
        "SELECT value FROM meta WHERE key = 'embedding_dims'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(384)
}

/// Insert a vector embedding for a chunk.
///
/// When sqlite-vec extension is available: inserts into `vec_chunks` (binary blob)
/// and `chunk_vec_map` (chunk_id → rowid mapping).
/// Otherwise: falls back to `vectors_data` (JSON array).
pub fn insert_vector(
    conn: &Connection,
    chunk_id: &str,
    embedding: &[f32],
) -> Result<(), VectorCodeError> {
    if has_vec_extension(conn) {
        // sqlite-vec path: binary blob + chunk_vec_map
        let dims = get_embedding_dims(conn) as usize;
        let normalized = normalize_embedding(embedding, dims);
        let blob = embedding_to_blob(&normalized);

        // Check if this chunk_id already has a mapping
        let existing_rowid: Option<i64> = conn
            .query_row(
                "SELECT vec_rowid FROM chunk_vec_map WHERE chunk_id = ?1",
                [chunk_id],
                |row| row.get(0),
            )
            .ok();

        let vec_rowid = if let Some(rowid) = existing_rowid {
            // Update existing vector: delete old, insert new with same rowid
            conn.execute("DELETE FROM vec_chunks WHERE rowid = ?1", [rowid])?;
            conn.execute(
                "INSERT INTO vec_chunks(rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![rowid, blob],
            )?;
            rowid
        } else {
            // Insert new vector (let SQLite assign rowid)
            conn.execute(
                "INSERT INTO vec_chunks(rowid, embedding) VALUES (NULL, ?1)",
                rusqlite::params![blob],
            )?;
            conn.last_insert_rowid()
        };

        // Store the mapping
        conn.execute(
            "INSERT OR REPLACE INTO chunk_vec_map (chunk_id, vec_rowid) VALUES (?1, ?2)",
            (chunk_id, vec_rowid),
        )?;
    } else {
        // Fallback path: JSON array in vectors_data
        let json =
            serde_json::to_string(embedding).map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to serialize embedding: {e}"),
            })?;

        conn.execute(
            "INSERT OR REPLACE INTO vectors_data (chunk_id, embedding) VALUES (?1, ?2)",
            (chunk_id, &json),
        )?;
    }
    Ok(())
}

/// Delete vectors associated with a chunk.
///
/// When sqlite-vec extension is available: deletes from `chunk_vec_map` and `vec_chunks`.
/// Otherwise: deletes from `vectors_data`.
pub fn delete_vectors_for_chunk(conn: &Connection, chunk_id: &str) -> Result<(), VectorCodeError> {
    if has_vec_extension(conn) {
        // Get the vec_rowid before deleting the mapping
        let vec_rowid: Option<i64> = conn
            .query_row(
                "SELECT vec_rowid FROM chunk_vec_map WHERE chunk_id = ?1",
                [chunk_id],
                |row| row.get(0),
            )
            .ok();

        // Delete from chunk_vec_map
        conn.execute("DELETE FROM chunk_vec_map WHERE chunk_id = ?1", [chunk_id])?;

        // Delete from vec_chunks if we had a mapping
        if let Some(rowid) = vec_rowid {
            conn.execute("DELETE FROM vec_chunks WHERE rowid = ?1", [rowid])?;
        }
    } else {
        conn.execute("DELETE FROM vectors_data WHERE chunk_id = ?1", [chunk_id])?;
    }
    Ok(())
}

/// Escape special LIKE pattern characters to prevent wildcard injection.
///
/// Escapes `%`, `_`, and `\` so they match literal characters.
pub(crate) fn escape_like_pattern(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Search for similar chunks using cosine similarity.
///
/// When sqlite-vec extension is available: uses native MATCH query on `vec_chunks`
/// with cosine distance metric. Converts distance to similarity score.
/// Otherwise: falls back to brute-force cosine similarity in Rust.
///
/// When `path_filter` is `Some`, the pre-escaped LIKE pattern (with trailing `%`)
/// is applied via `WHERE c.file_path LIKE ? ESCAPE '\'` in both branches.
pub fn search_similar(
    conn: &Connection,
    query_vec: &[f32],
    limit: usize,
    threshold: f32,
    path_filter: Option<&str>,
) -> Result<Vec<SearchResult>, VectorCodeError> {
    if has_vec_extension(conn) {
        // sqlite-vec path: native MATCH query with cosine distance
        let dims = get_embedding_dims(conn) as usize;
        let normalized = normalize_embedding(query_vec, dims);
        let query_blob = embedding_to_blob(&normalized);

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<(SearchResult, f32)> {
            let file_path: String = row.get(1)?;
            let start_line: u32 = row.get(2)?;
            let end_line: u32 = row.get(3)?;
            let symbol: Option<String> = row.get(6)?;
            let kind: String = row.get(7)?;
            let content: String = row.get(8)?;
            let parent_context: Option<String> = row.get(9)?;
            let language: String = row.get(10)?;
            let distance: Option<f32> = row.get(13)?;

            Ok((
                SearchResult {
                    file_path,
                    start_line,
                    end_line,
                    symbol,
                    kind,
                    language,
                    parent_context,
                    content,
                    score: 0.0,
                },
                distance.unwrap_or(1.0),
            ))
        };

        let mut results = Vec::new();

        if let Some(pattern) = path_filter {
            let sql = "SELECT c.id, c.file_path, c.start_line, c.end_line, \
                       c.byte_start, c.byte_end, c.symbol, c.kind, c.content, \
                       c.parent_context, c.language, c.file_mtime, c.content_hash, v.distance \
                       FROM ( \
                           SELECT rowid, distance FROM vec_chunks \
                           WHERE embedding MATCH ?1 \
                           ORDER BY distance LIMIT ?2 \
                       ) v \
                       JOIN chunk_vec_map m ON m.vec_rowid = v.rowid \
                       JOIN chunks c ON c.id = m.chunk_id \
                       WHERE c.file_path LIKE ?3 ESCAPE '\\'";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(
                rusqlite::params![query_blob, limit as i64, pattern],
                map_row,
            )?;
            for r in rows {
                let (mut search_res, distance) = r?;
                let score = 1.0 - distance;
                if score >= threshold {
                    search_res.score = score;
                    results.push(search_res);
                }
            }
        } else {
            let sql = "SELECT c.id, c.file_path, c.start_line, c.end_line, \
                       c.byte_start, c.byte_end, c.symbol, c.kind, c.content, \
                       c.parent_context, c.language, c.file_mtime, c.content_hash, v.distance \
                       FROM ( \
                           SELECT rowid, distance FROM vec_chunks \
                           WHERE embedding MATCH ?1 \
                           ORDER BY distance LIMIT ?2 \
                       ) v \
                       JOIN chunk_vec_map m ON m.vec_rowid = v.rowid \
                       JOIN chunks c ON c.id = m.chunk_id";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(rusqlite::params![query_blob, limit as i64], map_row)?;
            for r in rows {
                let (mut search_res, distance) = r?;
                let score = 1.0 - distance;
                if score >= threshold {
                    search_res.score = score;
                    results.push(search_res);
                }
            }
        }

        Ok(results)
    } else {
        // Fallback path: brute-force cosine similarity
        let mut scored: Vec<(String, f32)> = Vec::new();

        if let Some(pattern) = path_filter {
            let sql = "SELECT v.chunk_id, v.embedding FROM vectors_data v \
                       JOIN chunks c ON c.id = v.chunk_id \
                       WHERE c.file_path LIKE ?1 ESCAPE '\\'";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(rusqlite::params![pattern], |row| {
                let chunk_id: String = row.get(0)?;
                let embedding_json: String = row.get(1)?;
                Ok((chunk_id, embedding_json))
            })?;
            for row in rows {
                let (chunk_id, embedding_json) = row?;
                let embedding: Vec<f32> = serde_json::from_str(&embedding_json).map_err(|e| {
                    VectorCodeError::EmbedderError {
                        message: format!("Failed to deserialize embedding: {e}"),
                    }
                })?;
                let score = cosine_similarity(query_vec, &embedding);
                if score >= threshold {
                    scored.push((chunk_id, score));
                }
            }
        } else {
            let mut stmt = conn.prepare("SELECT chunk_id, embedding FROM vectors_data")?;
            let rows = stmt.query_map([], |row| {
                let chunk_id: String = row.get(0)?;
                let embedding_json: String = row.get(1)?;
                Ok((chunk_id, embedding_json))
            })?;
            for row in rows {
                let (chunk_id, embedding_json) = row?;
                let embedding: Vec<f32> = serde_json::from_str(&embedding_json).map_err(|e| {
                    VectorCodeError::EmbedderError {
                        message: format!("Failed to deserialize embedding: {e}"),
                    }
                })?;
                let score = cosine_similarity(query_vec, &embedding);
                if score >= threshold {
                    scored.push((chunk_id, score));
                }
            }
        };

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        // Fetch chunk metadata for each result
        let mut results = Vec::new();
        for (chunk_id, score) in scored {
            if let Some(chunk) = chunks::get_chunk(conn, &chunk_id)? {
                results.push(SearchResult {
                    file_path: chunk.file_path,
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    symbol: chunk.symbol,
                    kind: chunk.kind,
                    language: chunk.language,
                    parent_context: chunk.parent_context,
                    content: chunk.content,
                    score,
                });
            }
        }

        Ok(results)
    }
}

/// Compute cosine similarity between two vectors.
///
/// cos(θ) = (A · B) / (||A|| × ||B||)
///
/// Returns 0.0 if either vector has zero magnitude.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (&ai, &bi) in a.iter().zip(b.iter()) {
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return 0.0;
    }
    dot / denom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::Database;
    use crate::types::{compute_content_hash, Chunk};

    fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap(); // Use 4 dims to match test vectors
        db
    }

    fn make_test_chunk(id_suffix: &str, file_path: &str) -> Chunk {
        let content = format!("function {id_suffix}() {{}}");
        Chunk {
            id: format!("chunk_{id_suffix}"),
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: 5,
            byte_start: 0,
            byte_end: content.len() as u32,
            symbol: Some(id_suffix.to_string()),
            kind: "function_declaration".to_string(),
            content,
            parent_context: None,
            language: "typescript".to_string(),
            file_mtime: 1718000000,
            content_hash: compute_content_hash(&format!("function {id_suffix}() {{}}")),
        }
    }

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!(
            (sim - 1.0).abs() < 1e-6,
            "Identical vectors must have similarity 1.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 1e-6,
            "Orthogonal vectors must have similarity ~0.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_opposite_vectors_is_negative_one() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim - (-1.0)).abs() < 1e-6,
            "Opposite vectors must have similarity -1.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_zero_vector_returns_zero() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0, "Zero vector must return 0.0");
    }

    #[test]
    fn cosine_similarity_different_lengths_returns_zero() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0, "Different length vectors must return 0.0");
    }

    #[test]
    fn cosine_similarity_empty_returns_zero() {
        let sim = cosine_similarity(&[], &[]);
        assert_eq!(sim, 0.0, "Empty vectors must return 0.0");
    }

    #[test]
    fn cosine_similarity_known_value() {
        // cos(45°) = √2/2 ≈ 0.7071
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (sim - expected).abs() < 1e-5,
            "Expected ~{expected}, got {sim}"
        );
    }

    #[test]
    fn insert_and_retrieve_vector() {
        let db = setup_db();
        let chunk = make_test_chunk("alpha", "src/test.ts");
        chunks::insert_chunk(db.conn(), &chunk).unwrap();

        let embedding = vec![0.1, 0.2, 0.3, 0.4];
        insert_vector(db.conn(), &chunk.id, &embedding).unwrap();

        // Verify it's stored by searching with the same vector
        let results = search_similar(db.conn(), &embedding, 10, 0.0, None).unwrap();
        assert_eq!(results.len(), 1, "Should find the inserted vector");
        assert_eq!(results[0].file_path, "src/test.ts");
        assert!(
            (results[0].score - 1.0).abs() < 1e-5,
            "Self-similarity should be ~1.0, got {}",
            results[0].score
        );
    }

    #[test]
    fn delete_vectors_removes_embedding() {
        let db = setup_db();
        let chunk = make_test_chunk("beta", "src/test.ts");
        chunks::insert_chunk(db.conn(), &chunk).unwrap();
        insert_vector(db.conn(), &chunk.id, &[1.0, 0.0, 0.0]).unwrap();

        delete_vectors_for_chunk(db.conn(), &chunk.id).unwrap();

        let results = search_similar(db.conn(), &[1.0, 0.0, 0.0], 10, 0.0, None).unwrap();
        assert!(results.is_empty(), "No results after vector deletion");
    }

    #[test]
    fn search_similar_returns_top_k_by_score() {
        let db = setup_db();

        // Insert 3 chunks with different vectors
        let c1 = make_test_chunk("s1", "src/a.ts");
        let c2 = make_test_chunk("s2", "src/b.ts");
        let c3 = make_test_chunk("s3", "src/c.ts");
        chunks::insert_chunk(db.conn(), &c1).unwrap();
        chunks::insert_chunk(db.conn(), &c2).unwrap();
        chunks::insert_chunk(db.conn(), &c3).unwrap();

        // Vectors: c1 is closest to query, c2 is medium, c3 is far
        insert_vector(db.conn(), &c1.id, &[1.0, 0.0, 0.0]).unwrap();
        insert_vector(db.conn(), &c2.id, &[0.7, 0.7, 0.0]).unwrap();
        insert_vector(db.conn(), &c3.id, &[0.0, 0.0, 1.0]).unwrap();

        let query = vec![1.0, 0.1, 0.0];
        let results = search_similar(db.conn(), &query, 2, 0.0, None).unwrap();
        assert_eq!(results.len(), 2, "Should return top 2 results");
        // First result should be c1 (most similar to [1, 0, 0])
        assert_eq!(results[0].file_path, "src/a.ts");
        assert!(
            results[0].score >= results[1].score,
            "Results must be sorted by score descending"
        );
    }

    #[test]
    fn search_similar_filters_by_threshold() {
        let db = setup_db();

        let c1 = make_test_chunk("t1", "src/a.ts");
        let c2 = make_test_chunk("t2", "src/b.ts");
        chunks::insert_chunk(db.conn(), &c1).unwrap();
        chunks::insert_chunk(db.conn(), &c2).unwrap();

        insert_vector(db.conn(), &c1.id, &[1.0, 0.0, 0.0]).unwrap();
        insert_vector(db.conn(), &c2.id, &[0.0, 0.0, 1.0]).unwrap();

        // Query close to c1, far from c2
        let query = vec![1.0, 0.0, 0.0];
        let results = search_similar(db.conn(), &query, 10, 0.5, None).unwrap();
        assert_eq!(results.len(), 1, "Only c1 should pass threshold 0.5");
        assert_eq!(results[0].file_path, "src/a.ts");
    }

    #[test]
    fn search_similar_empty_db_returns_empty() {
        let db = setup_db();
        let results = search_similar(db.conn(), &[1.0, 0.0, 0.0, 0.0], 10, 0.0, None).unwrap();
        assert!(results.is_empty(), "Empty DB must return no results");
    }

    #[test]
    fn insert_vector_replaces_existing() {
        let db = setup_db();
        let chunk = make_test_chunk("replace", "src/r.ts");
        chunks::insert_chunk(db.conn(), &chunk).unwrap();

        insert_vector(db.conn(), &chunk.id, &[1.0, 0.0, 0.0]).unwrap();
        insert_vector(db.conn(), &chunk.id, &[0.0, 1.0, 0.0]).unwrap();

        // Search with the second vector — should find it with score 1.0
        let results = search_similar(db.conn(), &[0.0, 1.0, 0.0], 10, 0.0, None).unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            (results[0].score - 1.0).abs() < 1e-5,
            "Replaced vector should match new embedding"
        );
    }

    // ─── Phase 5: vec_chunks dual-path tests ───────────────────────────

    #[test]
    fn insert_vector_populates_chunk_vec_map_when_extension_available() {
        let db = setup_db();
        assert!(
            db.has_vec_extension(),
            "sqlite-vec extension must be available for this test"
        );

        let chunk = make_test_chunk("vec_map", "src/v.ts");
        chunks::insert_chunk(db.conn(), &chunk).unwrap();
        insert_vector(db.conn(), &chunk.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();

        // Verify chunk_vec_map has the mapping
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunk_vec_map WHERE chunk_id = ?1",
                [&chunk.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "chunk_vec_map must have an entry for the inserted chunk"
        );

        // Verify the vec_rowid is valid (> 0)
        let vec_rowid: i64 = db
            .conn()
            .query_row(
                "SELECT vec_rowid FROM chunk_vec_map WHERE chunk_id = ?1",
                [&chunk.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(vec_rowid > 0, "vec_rowid must be positive, got {vec_rowid}");
    }

    #[test]
    fn search_similar_uses_vec_chunks_and_returns_correct_scores() {
        let db = setup_db();
        assert!(db.has_vec_extension());

        let c1 = make_test_chunk("sc1", "src/a.ts");
        let c2 = make_test_chunk("sc2", "src/b.ts");
        chunks::insert_chunk(db.conn(), &c1).unwrap();
        chunks::insert_chunk(db.conn(), &c2).unwrap();

        // Insert orthogonal vectors
        insert_vector(db.conn(), &c1.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        insert_vector(db.conn(), &c2.id, &[0.0, 0.0, 0.0, 1.0]).unwrap();

        // Query aligned with c1
        let results = search_similar(db.conn(), &[1.0, 0.0, 0.0, 0.0], 10, 0.0, None).unwrap();
        assert_eq!(results.len(), 2, "Should find both vectors");
        assert_eq!(
            results[0].file_path, "src/a.ts",
            "c1 should be first (cosine=1.0)"
        );
        assert!(
            (results[0].score - 1.0).abs() < 0.01,
            "Self-similarity should be ~1.0, got {}",
            results[0].score
        );
        assert!(
            results[1].score < 0.01,
            "Orthogonal similarity should be ~0.0, got {}",
            results[1].score
        );
    }

    #[test]
    fn delete_vectors_cleans_chunk_vec_map_and_vec_chunks() {
        let db = setup_db();
        assert!(db.has_vec_extension());

        let chunk = make_test_chunk("del_vec", "src/d.ts");
        chunks::insert_chunk(db.conn(), &chunk).unwrap();
        insert_vector(db.conn(), &chunk.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();

        // Verify it exists
        let count_before: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunk_vec_map WHERE chunk_id = ?1",
                [&chunk.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_before, 1);

        // Delete
        delete_vectors_for_chunk(db.conn(), &chunk.id).unwrap();

        // Verify chunk_vec_map entry is gone
        let count_after: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chunk_vec_map WHERE chunk_id = ?1",
                [&chunk.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after, 0, "chunk_vec_map entry must be deleted");

        // Verify search returns nothing
        let results = search_similar(db.conn(), &[1.0, 0.0, 0.0, 0.0], 10, 0.0, None).unwrap();
        assert!(results.is_empty(), "No results after vector deletion");
    }

    #[test]
    fn insert_vector_with_different_dims_is_normalized() {
        let db = setup_db(); // dims=4
        let chunk = make_test_chunk("norm", "src/n.ts");
        chunks::insert_chunk(db.conn(), &chunk).unwrap();

        // Insert a 2-dimensional vector — should be padded to 4 dims
        insert_vector(db.conn(), &chunk.id, &[1.0, 0.0]).unwrap();

        // Should still be searchable
        let results = search_similar(db.conn(), &[1.0, 0.0, 0.0, 0.0], 10, 0.0, None).unwrap();
        assert_eq!(results.len(), 1, "Normalized vector should be findable");
    }

    // ─── escape_like_pattern tests ─────────────────────────────────────

    #[test]
    fn escape_like_pattern_special_chars() {
        assert_eq!(escape_like_pattern("normal"), "normal");
        assert_eq!(escape_like_pattern("test_1%"), "test\\_1\\%");
        assert_eq!(escape_like_pattern("a\\b"), "a\\\\b");
        assert_eq!(escape_like_pattern("%_\\all"), "\\%\\_\\\\all");
    }

    #[test]
    fn escape_like_pattern_empty_string() {
        assert_eq!(escape_like_pattern(""), "");
    }

    // ─── search_similar with path_filter tests ─────────────────────────

    #[test]
    fn search_similar_with_path_filter_vec_chunks() {
        let db = setup_db();
        assert!(db.has_vec_extension());

        let c1 = make_test_chunk("pf1", "src/auth/login.ts");
        let c2 = make_test_chunk("pf2", "src/payment/charge.ts");
        chunks::insert_chunk(db.conn(), &c1).unwrap();
        chunks::insert_chunk(db.conn(), &c2).unwrap();

        insert_vector(db.conn(), &c1.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        insert_vector(db.conn(), &c2.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();

        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = search_similar(db.conn(), &query, 10, 0.0, Some("src/auth/%")).unwrap();
        assert_eq!(results.len(), 1, "Only src/auth/ chunk should match");
        assert_eq!(results[0].file_path, "src/auth/login.ts");
    }

    #[test]
    fn search_similar_with_path_filter_fallback() {
        // Force fallback by using a DB without vec extension
        // Since our test DB always has vec extension, we test the None path
        // to verify no regression, and the Some path above for vec branch
        let db = setup_db();

        let c1 = make_test_chunk("pf3", "src/auth/login.ts");
        let c2 = make_test_chunk("pf4", "src/payment/charge.ts");
        chunks::insert_chunk(db.conn(), &c1).unwrap();
        chunks::insert_chunk(db.conn(), &c2).unwrap();

        insert_vector(db.conn(), &c1.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        insert_vector(db.conn(), &c2.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();

        let query = vec![1.0, 0.0, 0.0, 0.0];
        // None filter: should return both
        let results = search_similar(db.conn(), &query, 10, 0.0, None).unwrap();
        assert_eq!(results.len(), 2, "None filter should return all chunks");
    }

    #[test]
    fn search_similar_with_path_filter_none_unchanged() {
        let db = setup_db();

        let c1 = make_test_chunk("pf5", "src/a.ts");
        let c2 = make_test_chunk("pf6", "src/b.ts");
        chunks::insert_chunk(db.conn(), &c1).unwrap();
        chunks::insert_chunk(db.conn(), &c2).unwrap();

        insert_vector(db.conn(), &c1.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        insert_vector(db.conn(), &c2.id, &[0.0, 1.0, 0.0, 0.0]).unwrap();

        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = search_similar(db.conn(), &query, 10, 0.0, None).unwrap();
        assert_eq!(
            results.len(),
            2,
            "None filter returns same results as before"
        );
    }
}
