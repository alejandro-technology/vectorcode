//! `Store` trait — minimal sync-shaped port abstracting the four data planes
//! (chunks, vectors, lexical, graph) plus metadata.
//!
//! This is the foundational port for the phase-3 store evaluation (3.1). It
//! keeps the engine decoupled from `&rusqlite::Connection` and enables a
//! feature-gated LanceDB impl behind the same contract.
//!
//! Design constraints (see design obs #70):
//! - **Sync shape** — every method takes `&self` so the engine can hold
//!   `Arc<Store>` without an async cascade. LanceDB bridges async via a
//!   `tokio::runtime::Runtime` inside `LanceStore`.
//! - **Graph composition** — `Store::graph()` returns `&dyn GraphStore`. This
//!   preserves the existing `GraphStore` trait and tests.
//! - **Minimal port** — only the engine hot-path methods are abstracted; the
//!   full 53+ leak points stay free functions for now.

use std::collections::HashSet;
use std::path::Path;

use crate::error::VectorCodeError;
use crate::store::files::FileRecord;
use crate::store::graph::GraphStore;
use crate::types::{Chunk, SearchResult};

/// Sync-shaped trait abstracting the persisted data plane.
///
/// All methods take `&self` so the engine can hold an `Arc<dyn Store>` and
/// share across threads. State mutation must use interior mutability (the
/// `MockStore` uses `RwLock`; the production `SqliteStore` uses an internal
/// `tokio::sync::Mutex<Database>`).
pub trait Store: Send + Sync {
    // ─── Indexing ──────────────────────────────────────────────────────

    /// Insert or replace a chunk row. Triggers FTS5 sync in the sqlite impl.
    fn put_chunk(&self, chunk: &Chunk) -> Result<(), VectorCodeError>;

    /// Insert or update a file tracking record.
    fn put_file(&self, file: &FileRecord) -> Result<(), VectorCodeError>;

    /// Store the vector embedding for a chunk.
    fn put_vector(&self, chunk_id: &str, embedding: &[f32]) -> Result<(), VectorCodeError>;

    /// Insert an entry into the lexical index (FTS5 in sqlite).
    fn put_fts_entry(&self, chunk: &Chunk) -> Result<(), VectorCodeError>;

    /// Delete the vector embedding associated with a chunk.
    fn delete_vectors_for_chunk(&self, chunk_id: &str) -> Result<(), VectorCodeError>;

    /// Delete all chunks (and their vectors) for a given file path. Returns
    /// the number of chunks deleted.
    fn delete_chunks_for_file(&self, file_path: &str) -> Result<usize, VectorCodeError>;

    /// Delete chunks for files NOT in the valid set. Returns the number of
    /// chunks deleted.
    fn delete_stale_chunks(&self, valid_paths: &HashSet<String>) -> Result<usize, VectorCodeError>;

    // ─── Search ────────────────────────────────────────────────────────

    /// Dense vector search (cosine similarity). Returns chunks ordered by
    /// descending score, capped at `limit`. `path_filter` is a pre-escaped
    /// LIKE prefix (with trailing `%`).
    fn search_dense(
        &self,
        query_vec: &[f32],
        limit: usize,
        threshold: f32,
        path_filter: Option<&str>,
    ) -> Result<Vec<SearchResult>, VectorCodeError>;

    /// Sparse (FTS5/bm25) search. `path_filter` is a pre-escaped LIKE prefix
    /// (with trailing `%`).
    fn search_sparse(
        &self,
        query: &str,
        limit: usize,
        language: Option<&str>,
        path_filter: Option<&str>,
    ) -> Result<Vec<SearchResult>, VectorCodeError>;

    // ─── Graph ─────────────────────────────────────────────────────────

    /// Compose the existing `GraphStore` trait (callers/callees/dependents/imports).
    fn graph(&self) -> &dyn GraphStore;

    // ─── Metadata ──────────────────────────────────────────────────────

    /// Read a meta key (key-value singleton).
    fn get_meta(&self, key: &str) -> Result<Option<String>, VectorCodeError>;

    /// Write a meta key.
    fn set_meta(&self, key: &str, value: &str) -> Result<(), VectorCodeError>;

    /// Total chunk count in the store.
    fn count_chunks(&self) -> Result<u32, VectorCodeError>;

    /// Initialize the schema (idempotent). `dims` is the embedding dimension.
    fn init_schema(&self, dims: u32) -> Result<(), VectorCodeError>;
}

/// Factory for creating `Store` instances. Lets the benchmark harness and
/// engine swap backends at runtime without conditional compilation in caller
/// code.
pub trait StoreFactory: Send + Sync {
    /// Open (or create) a Store at the given path. For in-memory backends
    /// the path is ignored.
    fn create(&self, path: &Path) -> Result<Box<dyn Store>, VectorCodeError>;

    /// Backend identifier (e.g., "sqlite-vec", "lancedb", "memory").
    fn backend_name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trait is dyn-compatible (object-safe). Verify at compile time.
    fn _assert_object_safe(_s: Box<dyn Store>) {}

    /// The trait is Send + Sync.
    fn _assert_send_sync<T: Store + ?Sized>() {}
}
