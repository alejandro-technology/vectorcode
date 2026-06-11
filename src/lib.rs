pub mod config;
pub mod error;
pub mod types;

// Re-exports for convenience
pub use config::load_config;
pub use config::schema::Config;
pub use error::VectorCodeError;
pub use types::{compute_chunk_id, compute_content_hash, Chunk, IndexMeta, SearchResult};
