/// Engine module — AST-aware chunking and indexing.
pub mod chunker;
pub mod indexer;
pub mod languages;
pub mod outliner;
pub mod searcher;

pub use indexer::{IndexReport, Indexer};
pub use searcher::{SearchOptions, Searcher};
