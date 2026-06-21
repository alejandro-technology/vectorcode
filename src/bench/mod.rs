//! Benchmark harness — reproducible code-search quality measurement (Fase 1.1-1.2).
//!
//! Architecture: Corpus (port) → Indexer → Searcher → Metrics → Report.
//! Two corpus adapters: `LocalCorpus` (test fixtures) and `GitCorpus` (clone repos).

pub mod corpus;
pub mod metrics;
pub mod report;
pub mod runner;
pub mod schema;
pub mod store_bench;
pub mod verdict;
