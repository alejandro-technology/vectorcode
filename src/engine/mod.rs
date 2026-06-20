/// Engine module — AST-aware chunking and indexing.
pub mod chunker;
pub mod fusion;
pub mod graph_extractor;
pub mod graph_retriever;
pub mod indexer;
pub mod languages;
pub mod outliner;
pub mod router;
pub mod searcher;
pub mod sparse_searcher;

pub use fusion::{rrf_fuse, HybridSearcher};
pub use graph_retriever::GraphRetriever;
pub use indexer::{IndexReport, Indexer};
pub use router::{classify_query, GraphQuery, GraphQueryKind, RoutingDecision};
pub use searcher::{
    build_strategy, DenseSearcher, SearchMode, SearchOptions, SearchStrategy, Searcher,
};
pub use sparse_searcher::SparseSearcher;

// Re-export reranker types for engine consumers
pub use crate::reranker::{RerankDocument, Reranker};
