pub mod chunks;
pub mod db;
pub mod files;
pub mod fts;
pub mod graph;
pub mod meta;
pub mod sqlite;
pub mod vectors;

// `store` module contains the `Store` trait — same name as the parent module.
// Intentional per design: `crate::store::store::Store`. Allow the lint.
#[allow(clippy::module_inception)]
pub mod store;
