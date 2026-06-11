use anyhow::Result;
use rusqlite::Connection;

use crate::store::chunks;
use crate::types::SearchResult;
use crate::VectorCodeError;

/// Insert a vector embedding for a chunk.
///
/// Stores the embedding as a JSON array in the `vectors_data` fallback table.
/// TODO: When sqlite-vec extension is available, insert into vec_chunks instead.
pub fn insert_vector(
    conn: &Connection,
    chunk_id: &str,
    embedding: &[f32],
) -> Result<(), VectorCodeError> {
    let json = serde_json::to_string(embedding).map_err(|e| VectorCodeError::EmbedderError {
        message: format!("Failed to serialize embedding: {e}"),
    })?;

    conn.execute(
        "INSERT OR REPLACE INTO vectors_data (chunk_id, embedding) VALUES (?1, ?2)",
        (chunk_id, &json),
    )?;
    Ok(())
}

/// Delete vectors associated with a chunk.
pub fn delete_vectors_for_chunk(conn: &Connection, chunk_id: &str) -> Result<(), VectorCodeError> {
    conn.execute("DELETE FROM vectors_data WHERE chunk_id = ?1", [chunk_id])?;
    Ok(())
}

/// Search for similar chunks using cosine similarity.
///
/// Fallback implementation: loads all vectors, computes cosine similarity
/// in Rust, filters by threshold, sorts descending, returns top `limit`.
///
/// TODO: Replace with sqlite-vec MATCH when extension is loaded (ST-5).
pub fn search_similar(
    conn: &Connection,
    query_vec: &[f32],
    limit: usize,
    threshold: f32,
) -> Result<Vec<SearchResult>, VectorCodeError> {
    // Load all vectors with their chunk IDs
    let mut stmt = conn.prepare("SELECT chunk_id, embedding FROM vectors_data")?;
    let rows = stmt.query_map([], |row| {
        let chunk_id: String = row.get(0)?;
        let embedding_json: String = row.get(1)?;
        Ok((chunk_id, embedding_json))
    })?;

    let mut scored: Vec<(String, f32)> = Vec::new();
    for row in rows {
        let (chunk_id, embedding_json) = row?;
        let embedding: Vec<f32> =
            serde_json::from_str(&embedding_json).map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to deserialize embedding: {e}"),
            })?;

        let score = cosine_similarity(query_vec, &embedding);
        if score >= threshold {
            scored.push((chunk_id, score));
        }
    }

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
        db.init_schema(384).unwrap();
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
        let results = search_similar(db.conn(), &embedding, 10, 0.0).unwrap();
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

        let results = search_similar(db.conn(), &[1.0, 0.0, 0.0], 10, 0.0).unwrap();
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
        let results = search_similar(db.conn(), &query, 2, 0.0).unwrap();
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
        let results = search_similar(db.conn(), &query, 10, 0.5).unwrap();
        assert_eq!(results.len(), 1, "Only c1 should pass threshold 0.5");
        assert_eq!(results[0].file_path, "src/a.ts");
    }

    #[test]
    fn search_similar_empty_db_returns_empty() {
        let db = setup_db();
        let results = search_similar(db.conn(), &[1.0, 0.0], 10, 0.0).unwrap();
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
        let results = search_similar(db.conn(), &[0.0, 1.0, 0.0], 10, 0.0).unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            (results[0].score - 1.0).abs() < 1e-5,
            "Replaced vector should match new embedding"
        );
    }
}
