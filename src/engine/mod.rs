/// Engine module — AST-aware chunking and indexing.
pub mod chunker;
pub mod fusion;
pub mod indexer;
pub mod languages;
pub mod outliner;
pub mod searcher;
pub mod sparse_searcher;

pub use fusion::{rrf_fuse, HybridSearcher};
pub use indexer::{IndexReport, Indexer};
pub use searcher::{
    build_strategy, DenseSearcher, SearchMode, SearchOptions, SearchStrategy, Searcher,
};
pub use sparse_searcher::SparseSearcher;

// Re-export reranker types for engine consumers
pub use crate::reranker::{RerankDocument, Reranker};
