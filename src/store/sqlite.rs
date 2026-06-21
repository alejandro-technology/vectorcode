//! `SqliteStore` — the `Store` impl backed by the existing sqlite-vec schema.
//!
//! This is the production impl. It composes the existing `Database` struct
//! (which already implements `GraphStore` via `src/store/graph.rs`) and
//! delegates the indexing/search methods to the existing free functions in
//! `src/store/{chunks, vectors, fts, files, meta}.rs`.
//!
//! Concurrency model: the underlying `Connection` is `!Sync`, so we wrap it
//! in a `tokio::sync::Mutex` (matching what the engine already does). The
//! trait's `&self` methods acquire the lock internally via `block_in_place`.
//!
//! Graph access: `Store::graph()` returns a `&dyn GraphStore` view into a
//! `SqliteGraphView` field. The view holds its own `Arc<Mutex<Database>>`
//! reference, so each `GraphStore` method locks the mutex independently. This
//! keeps the trait's `&self` → `&dyn GraphStore` signature clean.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::VectorCodeError;
use crate::store::db::Database;
use crate::store::files::FileRecord;
use crate::store::graph::GraphStore;
use crate::store::store::{Store, StoreFactory};
use crate::types::{Chunk, EdgeType, GraphEdge, GraphNode, SearchResult};

/// Concrete `Store` impl backed by sqlite + sqlite-vec.
pub struct SqliteStore {
    db: Arc<Mutex<Database>>,
    graph_view: SqliteGraphView,
}

impl SqliteStore {
    /// Wrap an existing `Database` in a `SqliteStore`.
    pub fn from_database(db: Database) -> Self {
        let db_arc = Arc::new(Mutex::new(db));
        let graph_view = SqliteGraphView { db: db_arc.clone() };
        Self {
            db: db_arc,
            graph_view,
        }
    }

    /// Open an in-memory database (for tests and small-scale benchmarks).
    pub fn open_in_memory() -> Result<Self, VectorCodeError> {
        let db = Database::open_in_memory()?;
        Ok(Self::from_database(db))
    }

    /// Open or create a database at the given path.
    pub fn open(path: &Path) -> Result<Self, VectorCodeError> {
        let db = Database::open(path)?;
        Ok(Self::from_database(db))
    }

    /// Borrow the inner database (for tests/bench that need raw access).
    pub fn database(&self) -> Arc<Mutex<Database>> {
        self.db.clone()
    }
}

/// Lock the database from a `&self` Store method. Handles both
/// "inside a tokio runtime" and "no runtime" contexts. Returns an
/// `OwnedMutexGuard<Database>` which is `'static` and can be moved into
/// inner closures/blocks freely.
fn lock_db(
    db: Arc<Mutex<Database>>,
) -> Result<tokio::sync::OwnedMutexGuard<Database>, VectorCodeError> {
    let handle = tokio::runtime::Handle::try_current();
    if let Ok(handle) = handle {
        Ok(handle.block_on(async move { db.lock_owned().await }))
    } else {
        // No runtime: convert the sync-context lock into an owned guard.
        // We do this by spawning a new current-thread runtime just to
        // await lock_owned. This is a one-shot cost and the guard outlives
        // the dropped runtime.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VectorCodeError::EmbedderError {
                message: format!("failed to create tokio runtime: {e}"),
            })?;
        Ok(rt.block_on(async move { db.lock_owned().await }))
    }
}

impl Store for SqliteStore {
    fn put_chunk(&self, chunk: &Chunk) -> Result<(), VectorCodeError> {
        let db = self.db.clone();
        let chunk = chunk.clone();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::chunks::insert_chunk(guard.conn(), &chunk)
        })
    }

    fn put_file(&self, file: &FileRecord) -> Result<(), VectorCodeError> {
        let db = self.db.clone();
        let file = file.clone();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::files::upsert_file(
                guard.conn(),
                &file.path,
                file.mtime,
                file.size,
                &file.hash,
                file.indexed_at,
            )
        })
    }

    fn put_vector(&self, chunk_id: &str, embedding: &[f32]) -> Result<(), VectorCodeError> {
        let db = self.db.clone();
        let chunk_id = chunk_id.to_string();
        let embedding = embedding.to_vec();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::vectors::insert_vector(guard.conn(), &chunk_id, &embedding)
        })
    }

    fn put_fts_entry(&self, chunk: &Chunk) -> Result<(), VectorCodeError> {
        // FTS5 entries are kept in sync via triggers on `chunks` in the
        // existing schema; put_fts_entry is a no-op alias for the trigger
        // side-effect of put_chunk.
        self.put_chunk(chunk)
    }

    fn delete_vectors_for_chunk(&self, chunk_id: &str) -> Result<(), VectorCodeError> {
        let db = self.db.clone();
        let chunk_id = chunk_id.to_string();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::vectors::delete_vectors_for_chunk(guard.conn(), &chunk_id)
        })
    }

    fn delete_chunks_for_file(&self, file_path: &str) -> Result<usize, VectorCodeError> {
        let db = self.db.clone();
        let file_path = file_path.to_string();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::chunks::delete_chunks_for_file(guard.conn(), &file_path)
        })
    }

    fn delete_stale_chunks(&self, valid_paths: &HashSet<String>) -> Result<usize, VectorCodeError> {
        let db = self.db.clone();
        let valid_paths = valid_paths.clone();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::chunks::delete_stale_chunks(guard.conn(), &valid_paths)
        })
    }

    fn search_dense(
        &self,
        query_vec: &[f32],
        limit: usize,
        threshold: f32,
        path_filter: Option<&str>,
    ) -> Result<Vec<SearchResult>, VectorCodeError> {
        let db = self.db.clone();
        let query_vec = query_vec.to_vec();
        let path_filter = path_filter.map(|s| s.to_string());
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::vectors::search_similar(
                guard.conn(),
                &query_vec,
                limit,
                threshold,
                path_filter.as_deref(),
            )
        })
    }

    fn search_sparse(
        &self,
        query: &str,
        limit: usize,
        language: Option<&str>,
        path_filter: Option<&str>,
    ) -> Result<Vec<SearchResult>, VectorCodeError> {
        let db = self.db.clone();
        let query = query.to_string();
        let language = language.map(|s| s.to_string());
        let path_filter = path_filter.map(|s| s.to_string());
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::fts::search_sparse(
                guard.conn(),
                &query,
                limit,
                language.as_deref(),
                path_filter.as_deref(),
            )
        })
    }

    fn graph(&self) -> &dyn GraphStore {
        &self.graph_view
    }

    fn get_meta(&self, key: &str) -> Result<Option<String>, VectorCodeError> {
        let db = self.db.clone();
        let key = key.to_string();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::meta::read_meta(guard.conn(), &key)
        })
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<(), VectorCodeError> {
        let db = self.db.clone();
        let key = key.to_string();
        let value = value.to_string();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::meta::write_meta(guard.conn(), &key, &value)
        })
    }

    fn count_chunks(&self) -> Result<u32, VectorCodeError> {
        let db = self.db.clone();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::meta::count_chunks(guard.conn())
        })
    }

    fn init_schema(&self, dims: u32) -> Result<(), VectorCodeError> {
        let db = self.db.clone();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            guard.init_schema(dims)
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Factory for creating `SqliteStore` instances.
pub struct SqliteStoreFactory;

impl StoreFactory for SqliteStoreFactory {
    fn create(&self, path: &Path) -> Result<Box<dyn Store>, VectorCodeError> {
        let store = SqliteStore::open(path)?;
        Ok(Box::new(store))
    }

    fn backend_name(&self) -> &'static str {
        "sqlite-vec"
    }
}

/// `GraphStore` projection for `SqliteStore`. Holds its own `Arc<Mutex<Database>>`
/// so it can be returned as `&dyn GraphStore` without lifetime gymnastics.
struct SqliteGraphView {
    db: Arc<Mutex<Database>>,
}

impl GraphStore for SqliteGraphView {
    fn insert_nodes(&self, nodes: &[GraphNode]) -> anyhow::Result<()> {
        let db = self.db.clone();
        let nodes = nodes.to_vec();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::graph::insert_nodes(guard.conn(), &nodes)
        })
    }

    fn insert_edges(&self, edges: &[GraphEdge]) -> anyhow::Result<()> {
        let db = self.db.clone();
        let edges = edges.to_vec();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::graph::insert_edges(guard.conn(), &edges)
        })
    }

    fn get_callers(&self, symbol: &str) -> anyhow::Result<Vec<GraphNode>> {
        let db = self.db.clone();
        let symbol = symbol.to_string();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::graph::get_callers_filtered(guard.conn(), &symbol, None)
        })
    }

    fn get_callees(&self, symbol: &str) -> anyhow::Result<Vec<GraphNode>> {
        let db = self.db.clone();
        let symbol = symbol.to_string();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            let mut stmt = guard.conn().prepare(
                "SELECT n_target.id, n_target.symbol, n_target.kind, n_target.file_path
                 FROM graph_nodes n_source
                 JOIN graph_edges e ON n_source.id = e.source_id
                 JOIN graph_nodes n_target ON e.target_symbol = n_target.symbol
                 WHERE n_source.symbol = ?1 AND e.edge_type = 'Call'",
            )?;
            let node_iter = stmt.query_map(rusqlite::params![symbol], |row| {
                Ok(GraphNode {
                    id: row.get(0)?,
                    symbol: row.get(1)?,
                    kind: row.get(2)?,
                    file_path: row.get(3)?,
                })
            })?;
            let mut result = Vec::new();
            for n in node_iter {
                result.push(n?);
            }
            Ok::<_, anyhow::Error>(result)
        })
    }

    fn get_dependents(
        &self,
        symbol: &str,
        file_path: Option<&str>,
    ) -> anyhow::Result<Vec<GraphNode>> {
        let db = self.db.clone();
        let symbol = symbol.to_string();
        let file_path = file_path.map(|s| s.to_string());
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::graph::get_dependents(guard.conn(), &symbol, file_path.as_deref())
        })
    }

    fn get_imports(&self, symbol: &str, file_path: Option<&str>) -> anyhow::Result<Vec<GraphNode>> {
        let db = self.db.clone();
        let symbol = symbol.to_string();
        let file_path = file_path.map(|s| s.to_string());
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::graph::get_imports(guard.conn(), &symbol, file_path.as_deref())
        })
    }

    fn delete_nodes_by_file(&self, file_path: &str) -> anyhow::Result<()> {
        let db = self.db.clone();
        let file_path = file_path.to_string();
        tokio::task::block_in_place(move || {
            let guard = lock_db(db)?;
            crate::store::graph::delete_nodes_by_file(guard.conn(), &file_path)
        })
    }
}

// Suppress unused warning for EdgeType import — kept for future use.
#[allow(dead_code)]
fn _edge_type_marker(_t: EdgeType) {}
