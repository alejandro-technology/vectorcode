use serde::{Deserialize, Serialize};

/// Atomic unit of indexed code — spec §5.1.
///
/// Each chunk maps to one semantically meaningful block of source code
/// (function, class, impl block, etc.) extracted by the AST chunker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Chunk {
    /// Deterministic ID: blake3(file_path + ":" + byte_start + ":" + byte_end)
    pub id: String,

    /// Absolute path to the source file.
    pub file_path: String,

    /// Line range in the source file (1-indexed, inclusive).
    pub start_line: u32,
    pub end_line: u32,

    /// Byte offset range in the source file (0-indexed).
    pub byte_start: u32,
    pub byte_end: u32,

    /// Symbol name if available (e.g., "UserService.authenticate").
    pub symbol: Option<String>,

    /// AST node kind (e.g., "function_declaration", "class_declaration").
    pub kind: String,

    /// The source code content of this chunk.
    pub content: String,

    /// Parent context for retrieval enrichment (e.g., "class UserService").
    pub parent_context: Option<String>,

    /// Language identifier (e.g., "typescript", "python", "rust").
    pub language: String,

    /// File modification time at indexing (Unix timestamp seconds).
    pub file_mtime: i64,

    /// Content hash for change detection: blake3(content).
    pub content_hash: String,
}

/// Index metadata — spec §5.2.
///
/// Stored as a singleton row in the `meta` table. Records which provider
/// and model created this index, along with aggregate statistics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexMeta {
    /// Embedding provider: "onnx" | "gemini" | "ollama" | "openai"
    pub provider: String,

    /// Specific model identifier (e.g., "all-MiniLM-L6-v2").
    pub model: String,

    /// Vector dimensions — FIXED at index creation time.
    pub dimensions: u32,

    /// Timestamp of index creation (ISO 8601).
    pub created_at: String,

    /// Timestamp of last completed sync (ISO 8601).
    pub last_sync_at: Option<String>,

    /// Total files indexed.
    pub files_indexed: u32,

    /// Total chunks stored.
    pub chunks_stored: u32,

    /// VectorCode version that created this index.
    pub vectorcode_version: String,
}

/// Search result returned to the caller — spec §5.3.
///
/// Combines chunk metadata with the cosine similarity score.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol: Option<String>,
    pub kind: String,
    pub language: String,
    pub parent_context: Option<String>,
    pub content: String,

    /// Cosine similarity score (0.0–1.0, higher = more relevant).
    pub score: f32,
}

/// Compute a deterministic chunk ID from file path and byte range.
///
/// Uses blake3: `blake3("{file_path}:{byte_start}:{byte_end}")`.
/// This ensures the same chunk always gets the same ID across runs,
/// enabling idempotent re-indexing.
pub fn compute_chunk_id(file_path: &str, byte_start: u32, byte_end: u32) -> String {
    let input = format!("{file_path}:{byte_start}:{byte_end}");
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

/// Compute a content hash for change detection.
///
/// Uses blake3 over the raw content bytes.
pub fn compute_content_hash(content: &str) -> String {
    blake3::hash(content.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_id_is_deterministic() {
        let id1 = compute_chunk_id("src/main.rs", 100, 500);
        let id2 = compute_chunk_id("src/main.rs", 100, 500);
        assert_eq!(id1, id2, "Same inputs must produce same ID");
    }

    #[test]
    fn chunk_id_differs_for_different_byte_ranges() {
        let id1 = compute_chunk_id("src/main.rs", 100, 500);
        let id2 = compute_chunk_id("src/main.rs", 500, 900);
        assert_ne!(id1, id2, "Different byte ranges must produce different IDs");
    }

    #[test]
    fn chunk_id_differs_for_different_files() {
        let id1 = compute_chunk_id("src/main.rs", 100, 500);
        let id2 = compute_chunk_id("src/lib.rs", 100, 500);
        assert_ne!(id1, id2, "Different files must produce different IDs");
    }

    #[test]
    fn chunk_id_is_valid_hex_string() {
        let id = compute_chunk_id("test.ts", 0, 42);
        assert_eq!(id.len(), 64, "blake3 hex digest is 64 chars");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "ID must be hex: {id}"
        );
    }

    #[test]
    fn content_hash_changes_when_content_changes() {
        let h1 = compute_content_hash("fn hello() {}");
        let h2 = compute_content_hash("fn hello() { println!(\"hi\"); }");
        assert_ne!(h1, h2, "Different content must produce different hashes");
    }

    #[test]
    fn content_hash_is_stable() {
        let h1 = compute_content_hash("fn hello() {}");
        let h2 = compute_content_hash("fn hello() {}");
        assert_eq!(h1, h2, "Same content must produce same hash");
    }

    #[test]
    fn chunk_construction_and_field_access() {
        let chunk = Chunk {
            id: compute_chunk_id("src/auth.ts", 0, 200),
            file_path: "src/auth.ts".to_string(),
            start_line: 1,
            end_line: 15,
            byte_start: 0,
            byte_end: 200,
            symbol: Some("authenticate".to_string()),
            kind: "function_declaration".to_string(),
            content: "function authenticate() { ... }".to_string(),
            parent_context: None,
            language: "typescript".to_string(),
            file_mtime: 1718000000,
            content_hash: compute_content_hash("function authenticate() { ... }"),
        };

        assert_eq!(chunk.file_path, "src/auth.ts");
        assert_eq!(chunk.symbol.as_deref(), Some("authenticate"));
        assert_eq!(chunk.kind, "function_declaration");
        assert_eq!(chunk.language, "typescript");
        assert!(chunk.parent_context.is_none());
    }

    #[test]
    fn chunk_serialization_roundtrip() {
        let chunk = Chunk {
            id: "abc123".to_string(),
            file_path: "test.rs".to_string(),
            start_line: 10,
            end_line: 20,
            byte_start: 100,
            byte_end: 500,
            symbol: Some("my_fn".to_string()),
            kind: "function_item".to_string(),
            content: "fn my_fn() {}".to_string(),
            parent_context: Some("impl MyStruct".to_string()),
            language: "rust".to_string(),
            file_mtime: 1718000000,
            content_hash: "deadbeef".to_string(),
        };

        let json = serde_json::to_string(&chunk).unwrap();
        let deserialized: Chunk = serde_json::from_str(&json).unwrap();
        assert_eq!(chunk, deserialized);
    }

    #[test]
    fn index_meta_serialization_roundtrip() {
        let meta = IndexMeta {
            provider: "onnx".to_string(),
            model: "all-MiniLM-L6-v2".to_string(),
            dimensions: 384,
            created_at: "2026-06-10T20:00:00Z".to_string(),
            last_sync_at: Some("2026-06-10T20:05:00Z".to_string()),
            files_indexed: 100,
            chunks_stored: 500,
            vectorcode_version: "0.1.0".to_string(),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: IndexMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, deserialized);
    }

    #[test]
    fn search_result_score_is_preserved() {
        let result = SearchResult {
            file_path: "src/pay.ts".to_string(),
            start_line: 45,
            end_line: 92,
            symbol: Some("handleRetry".to_string()),
            kind: "method_definition".to_string(),
            language: "typescript".to_string(),
            parent_context: Some("class PaymentRetryHandler".to_string()),
            content: "async handleRetry() { ... }".to_string(),
            score: 0.87,
        };

        assert!((result.score - 0.87).abs() < f32::EPSILON);
        let json = serde_json::to_string(&result).unwrap();
        let deser: SearchResult = serde_json::from_str(&json).unwrap();
        assert!((deser.score - 0.87).abs() < f32::EPSILON);
    }

    #[test]
    fn index_meta_with_no_last_sync() {
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

        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("\"last_sync_at\":null"));
        let deser: IndexMeta = serde_json::from_str(&json).unwrap();
        assert!(deser.last_sync_at.is_none());
    }
}
