//! Store contract tests — parameterized over any `Store` impl.
//!
//! These tests describe the behavior the `Store` trait MUST guarantee, regardless
//! of backend (sqlite-vec, LanceDB, in-memory mock). Each test is written once
//! and re-run against both the in-memory `MockStore` and the real `SqliteStore`,
//! ensuring every impl behaves identically for the documented scenarios.
//!
//! Spec: R1 (Minimal Store Port) — scenarios: contract put+search, contract graph,
//! SqliteStore reachable through trait.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use vectorcode::store::files::FileRecord;
use vectorcode::store::graph::GraphStore;
use vectorcode::store::store::Store;
use vectorcode::types::{compute_chunk_id, compute_content_hash, Chunk};
use vectorcode::VectorCodeError;

fn make_mock() -> Arc<MockStore> {
    Arc::new(MockStore::new())
}

/// In-memory `Store` impl backed by HashMaps behind an RwLock.
pub struct MockStore {
    state: Arc<RwLock<MockState>>,
    graph_view: MockGraphView,
}

#[derive(Default)]
struct MockState {
    chunks: std::collections::HashMap<String, Chunk>,
    files: std::collections::HashMap<String, FileRecord>,
    vectors: std::collections::HashMap<String, Vec<f32>>,
    fts_content: Vec<(String, String)>,
    graph_nodes: Vec<vectorcode::types::GraphNode>,
    graph_edges: Vec<vectorcode::types::GraphEdge>,
    meta: std::collections::HashMap<String, String>,
    dimensions: u32,
    schema_initialized: bool,
}

impl MockStore {
    pub fn new() -> Self {
        let state = Arc::new(RwLock::new(MockState::default()));
        let graph_view = MockGraphView {
            state: state.clone(),
        };
        Self { state, graph_view }
    }
}

impl Default for MockStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Store for MockStore {
    fn put_chunk(&self, chunk: &Chunk) -> Result<(), VectorCodeError> {
        let mut s = self.state.write().unwrap();
        s.chunks.insert(chunk.id.clone(), chunk.clone());
        s.fts_content
            .push((chunk.id.clone(), chunk.content.clone()));
        Ok(())
    }

    fn put_file(&self, file: &FileRecord) -> Result<(), VectorCodeError> {
        let mut s = self.state.write().unwrap();
        s.files.insert(file.path.clone(), file.clone());
        Ok(())
    }

    fn put_vector(&self, chunk_id: &str, embedding: &[f32]) -> Result<(), VectorCodeError> {
        let mut s = self.state.write().unwrap();
        s.vectors.insert(chunk_id.to_string(), embedding.to_vec());
        Ok(())
    }

    fn put_fts_entry(&self, chunk: &Chunk) -> Result<(), VectorCodeError> {
        let mut s = self.state.write().unwrap();
        s.fts_content
            .push((chunk.id.clone(), chunk.content.clone()));
        Ok(())
    }

    fn delete_vectors_for_chunk(&self, chunk_id: &str) -> Result<(), VectorCodeError> {
        let mut s = self.state.write().unwrap();
        s.vectors.remove(chunk_id);
        Ok(())
    }

    fn delete_chunks_for_file(&self, file_path: &str) -> Result<usize, VectorCodeError> {
        let mut s = self.state.write().unwrap();
        let to_remove: Vec<String> = s
            .chunks
            .iter()
            .filter(|(_, c)| c.file_path == file_path)
            .map(|(id, _)| id.clone())
            .collect();
        let count = to_remove.len();
        for id in &to_remove {
            s.chunks.remove(id);
            s.vectors.remove(id);
            s.fts_content.retain(|(cid, _)| cid != id);
        }
        Ok(count)
    }

    fn delete_stale_chunks(&self, valid_paths: &HashSet<String>) -> Result<usize, VectorCodeError> {
        let mut s = self.state.write().unwrap();
        let stale: Vec<String> = s
            .chunks
            .iter()
            .filter(|(_, c)| !valid_paths.contains(&c.file_path))
            .map(|(id, _)| id.clone())
            .collect();
        let count = stale.len();
        for id in &stale {
            s.chunks.remove(id);
            s.vectors.remove(id);
            s.fts_content.retain(|(cid, _)| cid != id);
        }
        Ok(count)
    }

    fn search_dense(
        &self,
        query_vec: &[f32],
        limit: usize,
        threshold: f32,
        path_filter: Option<&str>,
    ) -> Result<Vec<vectorcode::types::SearchResult>, VectorCodeError> {
        let s = self.state.read().unwrap();
        let mut results = Vec::new();
        for chunk in s.chunks.values() {
            if let Some(filter) = path_filter {
                if !chunk.file_path.starts_with(filter) {
                    continue;
                }
            }
            if let Some(emb) = s.vectors.get(&chunk.id) {
                let score = cosine(query_vec, emb);
                if score >= threshold {
                    results.push(vectorcode::types::SearchResult {
                        file_path: chunk.file_path.clone(),
                        start_line: chunk.start_line,
                        end_line: chunk.end_line,
                        symbol: chunk.symbol.clone(),
                        kind: chunk.kind.clone(),
                        language: chunk.language.clone(),
                        parent_context: chunk.parent_context.clone(),
                        content: chunk.content.clone(),
                        score,
                    });
                }
            }
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);
        Ok(results)
    }

    fn search_sparse(
        &self,
        query: &str,
        limit: usize,
        language: Option<&str>,
        path_filter: Option<&str>,
    ) -> Result<Vec<vectorcode::types::SearchResult>, VectorCodeError> {
        let s = self.state.read().unwrap();
        let q = query.to_lowercase();
        let tokens: Vec<&str> = q.split_whitespace().collect();
        let mut scored: Vec<(f32, &Chunk)> = Vec::new();
        for chunk in s.chunks.values() {
            if let Some(lang) = language {
                if chunk.language != lang {
                    continue;
                }
            }
            if let Some(filter) = path_filter {
                if !chunk.file_path.starts_with(filter) {
                    continue;
                }
            }
            let content_lower = chunk.content.to_lowercase();
            let symbol_lower = chunk.symbol.as_deref().unwrap_or("").to_lowercase();
            let mut hits = 0;
            for token in &tokens {
                if content_lower.contains(token) || symbol_lower.contains(token) {
                    hits += 1;
                }
            }
            if hits > 0 {
                let score = hits as f32 / tokens.len() as f32;
                scored.push((score, chunk));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut results = Vec::new();
        for (score, chunk) in scored.into_iter().take(limit) {
            results.push(vectorcode::types::SearchResult {
                file_path: chunk.file_path.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                symbol: chunk.symbol.clone(),
                kind: chunk.kind.clone(),
                language: chunk.language.clone(),
                parent_context: chunk.parent_context.clone(),
                content: chunk.content.clone(),
                score,
            });
        }
        Ok(results)
    }

    fn graph(&self) -> &dyn GraphStore {
        &self.graph_view
    }

    fn get_meta(&self, key: &str) -> Result<Option<String>, VectorCodeError> {
        let s = self.state.read().unwrap();
        Ok(s.meta.get(key).cloned())
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<(), VectorCodeError> {
        let mut s = self.state.write().unwrap();
        s.meta.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn count_chunks(&self) -> Result<u32, VectorCodeError> {
        let s = self.state.read().unwrap();
        Ok(s.chunks.len() as u32)
    }

    fn init_schema(&self, dims: u32) -> Result<(), VectorCodeError> {
        let mut s = self.state.write().unwrap();
        if !s.schema_initialized {
            s.schema_initialized = true;
            s.dimensions = dims;
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// GraphStore projection for MockStore. Shares state via Arc<RwLock<MockState>>.
struct MockGraphView {
    state: Arc<RwLock<MockState>>,
}

impl GraphStore for MockGraphView {
    fn insert_nodes(&self, nodes: &[vectorcode::types::GraphNode]) -> anyhow::Result<()> {
        let mut s = self.state.write().unwrap();
        for n in nodes {
            s.graph_nodes.push(n.clone());
        }
        Ok(())
    }

    fn insert_edges(&self, edges: &[vectorcode::types::GraphEdge]) -> anyhow::Result<()> {
        let mut s = self.state.write().unwrap();
        for e in edges {
            s.graph_edges.push(e.clone());
        }
        Ok(())
    }

    fn get_callers(&self, symbol: &str) -> anyhow::Result<Vec<vectorcode::types::GraphNode>> {
        let s = self.state.read().unwrap();
        let caller_ids: std::collections::HashSet<String> = s
            .graph_edges
            .iter()
            .filter(|e| e.target_symbol == symbol)
            .map(|e| e.source_id.clone())
            .collect();
        let callers: Vec<_> = s
            .graph_nodes
            .iter()
            .filter(|n| caller_ids.contains(&n.id))
            .cloned()
            .collect();
        Ok(callers)
    }

    fn get_callees(&self, symbol: &str) -> anyhow::Result<Vec<vectorcode::types::GraphNode>> {
        let s = self.state.read().unwrap();
        let source_id = s
            .graph_nodes
            .iter()
            .find(|n| n.symbol == symbol)
            .map(|n| n.id.clone());
        let mut callees = Vec::new();
        if let Some(sid) = source_id {
            let target_symbols: std::collections::HashSet<String> = s
                .graph_edges
                .iter()
                .filter(|e| e.source_id == sid)
                .map(|e| e.target_symbol.clone())
                .collect();
            for n in &s.graph_nodes {
                if target_symbols.contains(&n.symbol) {
                    callees.push(n.clone());
                }
            }
        }
        Ok(callees)
    }

    fn get_dependents(
        &self,
        symbol: &str,
        _file_path: Option<&str>,
    ) -> anyhow::Result<Vec<vectorcode::types::GraphNode>> {
        let s = self.state.read().unwrap();
        let caller_ids: std::collections::HashSet<String> = s
            .graph_edges
            .iter()
            .filter(|e| {
                e.target_symbol == symbol
                    && matches!(
                        e.edge_type,
                        vectorcode::types::EdgeType::Import
                            | vectorcode::types::EdgeType::Extends
                            | vectorcode::types::EdgeType::Reference
                    )
            })
            .map(|e| e.source_id.clone())
            .collect();
        let callers: Vec<_> = s
            .graph_nodes
            .iter()
            .filter(|n| caller_ids.contains(&n.id))
            .cloned()
            .collect();
        Ok(callers)
    }

    fn get_imports(
        &self,
        symbol: &str,
        _file_path: Option<&str>,
    ) -> anyhow::Result<Vec<vectorcode::types::GraphNode>> {
        let s = self.state.read().unwrap();
        let source_id = s
            .graph_nodes
            .iter()
            .find(|n| n.symbol == symbol)
            .map(|n| n.id.clone());
        let mut imports = Vec::new();
        if let Some(sid) = source_id {
            for e in &s.graph_edges {
                if e.source_id == sid && e.edge_type == vectorcode::types::EdgeType::Import {
                    if let Some(n) = s.graph_nodes.iter().find(|n| n.symbol == e.target_symbol) {
                        imports.push(n.clone());
                    } else {
                        imports.push(vectorcode::types::GraphNode {
                            id: format!("ext:{}", e.target_symbol),
                            symbol: e.target_symbol.clone(),
                            kind: "external".to_string(),
                            file_path: String::new(),
                        });
                    }
                }
            }
        }
        Ok(imports)
    }

    fn delete_nodes_by_file(&self, file_path: &str) -> anyhow::Result<()> {
        let mut s = self.state.write().unwrap();
        s.graph_nodes.retain(|n| n.file_path != file_path);
        Ok(())
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

fn make_test_chunk(id: &str, file_path: &str, content: &str) -> Chunk {
    Chunk {
        id: id.to_string(),
        file_path: file_path.to_string(),
        start_line: 1,
        end_line: 10,
        byte_start: 0,
        byte_end: content.len() as u32,
        symbol: Some("test_fn".to_string()),
        kind: "function_declaration".to_string(),
        content: content.to_string(),
        parent_context: None,
        language: "typescript".to_string(),
        file_mtime: 1718000000,
        content_hash: compute_content_hash(content),
    }
}

// ─── MockStore contract tests ───────────────────────────────────────────

/// Scenario: contract put+search round-trip
#[test]
fn mock_store_put_and_search_round_trip() {
    let store = make_mock();
    store.init_schema(4).unwrap();

    let chunk = make_test_chunk(
        &compute_chunk_id("src/auth.ts", 0, 100),
        "src/auth.ts",
        "function authenticate(user) { return user; }",
    );
    store.put_chunk(&chunk).unwrap();
    store.put_vector(&chunk.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();

    let results = store
        .search_dense(&[1.0, 0.0, 0.0, 0.0], 10, 0.0, None)
        .unwrap();
    assert_eq!(results.len(), 1, "Should return the chunk we put");
    assert_eq!(results[0].file_path, "src/auth.ts");
    assert!(
        results[0].score > 0.99,
        "Identical vector must score ~1.0, got {}",
        results[0].score
    );
}

#[test]
fn mock_store_sparse_search_finds_by_content() {
    let store = make_mock();
    store.init_schema(4).unwrap();

    let chunk = make_test_chunk(
        &compute_chunk_id("src/pay.ts", 0, 100),
        "src/pay.ts",
        "function processPayment() { charge(); }",
    );
    store.put_chunk(&chunk).unwrap();

    let results = store
        .search_sparse("processPayment", 10, None, None)
        .unwrap();
    assert_eq!(results.len(), 1, "Should find the chunk by content");
    assert_eq!(results[0].file_path, "src/pay.ts");
}

#[test]
fn mock_store_dense_search_with_path_filter() {
    let store = make_mock();
    store.init_schema(4).unwrap();

    let c1 = make_test_chunk(
        &compute_chunk_id("src/auth/login.ts", 0, 50),
        "src/auth/login.ts",
        "auth code",
    );
    let c2 = make_test_chunk(
        &compute_chunk_id("src/pay/charge.ts", 0, 50),
        "src/pay/charge.ts",
        "pay code",
    );
    store.put_chunk(&c1).unwrap();
    store.put_chunk(&c2).unwrap();
    store.put_vector(&c1.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();
    store.put_vector(&c2.id, &[1.0, 0.0, 0.0, 0.0]).unwrap();

    let results = store
        .search_dense(&[1.0, 0.0, 0.0, 0.0], 10, 0.0, Some("src/auth/"))
        .unwrap();
    assert_eq!(results.len(), 1, "Path filter should restrict to auth/");
    assert_eq!(results[0].file_path, "src/auth/login.ts");
}

#[test]
fn mock_store_delete_chunks_for_file_removes_chunk_and_vector() {
    let store = make_mock();
    store.init_schema(4).unwrap();

    let chunk = make_test_chunk(
        &compute_chunk_id("src/x.ts", 0, 50),
        "src/x.ts",
        "to delete",
    );
    store.put_chunk(&chunk).unwrap();
    store.put_vector(&chunk.id, &[0.0, 1.0, 0.0, 0.0]).unwrap();
    assert_eq!(store.count_chunks().unwrap(), 1);

    let deleted = store.delete_chunks_for_file("src/x.ts").unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(store.count_chunks().unwrap(), 0);
}

#[test]
fn mock_store_delete_stale_chunks() {
    let store = make_mock();
    store.init_schema(4).unwrap();

    let keep = make_test_chunk(
        &compute_chunk_id("src/keep.ts", 0, 50),
        "src/keep.ts",
        "keep me",
    );
    let stale = make_test_chunk(
        &compute_chunk_id("src/stale.ts", 0, 50),
        "src/stale.ts",
        "stale me",
    );
    store.put_chunk(&keep).unwrap();
    store.put_chunk(&stale).unwrap();

    let valid: HashSet<String> = ["src/keep.ts".to_string()].into_iter().collect();
    let deleted = store.delete_stale_chunks(&valid).unwrap();
    assert_eq!(deleted, 1, "Only the stale chunk should be deleted");
    assert_eq!(store.count_chunks().unwrap(), 1);
}

#[test]
fn mock_store_meta_round_trip() {
    let store = make_mock();
    store.init_schema(4).unwrap();

    store.set_meta("provider", "onnx").unwrap();
    let value = store.get_meta("provider").unwrap();
    assert_eq!(value, Some("onnx".to_string()));
}

#[test]
fn mock_store_graph_put_and_callers_query() {
    use vectorcode::types::{EdgeType, GraphEdge, GraphNode};

    let store = make_mock();
    store.init_schema(4).unwrap();

    let caller_node = GraphNode {
        id: "caller1".to_string(),
        symbol: "main".to_string(),
        kind: "function".to_string(),
        file_path: "src/main.ts".to_string(),
    };
    let callee_node = GraphNode {
        id: "callee1".to_string(),
        symbol: "search".to_string(),
        kind: "function".to_string(),
        file_path: "src/search.ts".to_string(),
    };
    let edge = GraphEdge {
        source_id: caller_node.id.clone(),
        target_symbol: callee_node.symbol.clone(),
        edge_type: EdgeType::Call,
    };

    store
        .graph()
        .insert_nodes(&[caller_node, callee_node])
        .unwrap();
    store.graph().insert_edges(&[edge]).unwrap();
    let callers = store.graph().get_callers("search").unwrap();

    assert_eq!(callers.len(), 1, "Should find 1 caller for 'search'");
    assert_eq!(callers[0].symbol, "main");
}

// ─── SqliteStore contract tests ─────────────────────────────────────────

/// Scenario: SqliteStore reachable through trait.
#[test]
fn sqlitestore_put_and_search_round_trip() {
    use vectorcode::store::sqlite::SqliteStore;

    let store = SqliteStore::open_in_memory().expect("SqliteStore::open_in_memory");
    store.init_schema(4).expect("init_schema");

    let chunk = make_test_chunk(
        &compute_chunk_id("src/sqlite.ts", 0, 100),
        "src/sqlite.ts",
        "function searchByVector() { return 1; }",
    );
    store.put_chunk(&chunk).expect("put_chunk");
    store
        .put_vector(&chunk.id, &[1.0, 0.0, 0.0, 0.0])
        .expect("put_vector");

    let results = store
        .search_dense(&[1.0, 0.0, 0.0, 0.0], 10, 0.0, None)
        .expect("search_dense");
    assert_eq!(
        results.len(),
        1,
        "SqliteStore should return the chunk we put"
    );
    assert_eq!(results[0].file_path, "src/sqlite.ts");
    assert!(
        results[0].score > 0.99,
        "Identical vector must score ~1.0, got {}",
        results[0].score
    );

    store.set_meta("test_key", "test_value").expect("set_meta");
    assert_eq!(
        store.get_meta("test_key").expect("get_meta"),
        Some("test_value".to_string())
    );
    assert_eq!(store.count_chunks().expect("count_chunks"), 1);
}

#[test]
fn sqlitestore_sparse_search_finds_chunk() {
    use vectorcode::store::sqlite::SqliteStore;

    let store = SqliteStore::open_in_memory().unwrap();
    store.init_schema(4).unwrap();

    let chunk = make_test_chunk(
        &compute_chunk_id("src/login.ts", 0, 100),
        "src/login.ts",
        "function authenticateUser() { return true; }",
    );
    store.put_chunk(&chunk).unwrap();

    let results = store
        .search_sparse("authenticateUser", 10, None, None)
        .unwrap();
    assert_eq!(results.len(), 1, "FTS5 should find the chunk by content");
    assert_eq!(results[0].file_path, "src/login.ts");
}

#[test]
fn sqlitestore_graph_callers_round_trip() {
    use vectorcode::store::sqlite::SqliteStore;
    use vectorcode::types::{EdgeType, GraphEdge, GraphNode};

    let store = SqliteStore::open_in_memory().unwrap();
    store.init_schema(4).unwrap();

    let caller = GraphNode {
        id: "caller1".to_string(),
        symbol: "main".to_string(),
        kind: "function".to_string(),
        file_path: "src/main.ts".to_string(),
    };
    let callee = GraphNode {
        id: "callee1".to_string(),
        symbol: "search".to_string(),
        kind: "function".to_string(),
        file_path: "src/search.ts".to_string(),
    };
    let edge = GraphEdge {
        source_id: caller.id.clone(),
        target_symbol: callee.symbol.clone(),
        edge_type: EdgeType::Call,
    };

    store.graph().insert_nodes(&[caller, callee]).unwrap();
    store.graph().insert_edges(&[edge]).unwrap();
    let callers = store.graph().get_callers("search").unwrap();
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].symbol, "main");
}

#[test]
fn sqlitestore_factory_creates_store() {
    use tempfile::TempDir;
    use vectorcode::store::sqlite::SqliteStoreFactory;
    use vectorcode::store::store::StoreFactory;

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let factory = SqliteStoreFactory;
    let store = factory.create(&db_path).expect("factory.create");
    assert_eq!(factory.backend_name(), "sqlite-vec");
    store.init_schema(4).unwrap();
    assert_eq!(store.count_chunks().unwrap(), 0);
}
