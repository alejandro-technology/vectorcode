pub mod bench;
pub mod cli;
pub mod config;
pub mod embedder;
pub mod engine;
pub mod error;
pub mod mcp;
pub mod reranker;
pub mod store;
pub mod types;
pub mod watcher;

// Re-exports for convenience
pub use config::load_config;
pub use config::schema::Config;
pub use error::VectorCodeError;
pub use store::db::Database;
pub use store::graph::GraphStore;
pub use types::{compute_chunk_id, compute_content_hash, Chunk, IndexMeta, SearchResult};
