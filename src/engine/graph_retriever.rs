//! Graph retriever — SearchStrategy implementation for structural graph queries.
//!
//! Queries the knowledge graph and joins graph nodes to chunks to produce
//! SearchResult rows with content.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::engine::router::{classify_query, GraphQueryKind, RoutingDecision};
use crate::engine::searcher::{SearchMode, SearchOptions, SearchStrategy};
use crate::store::db::Database;
use crate::store::graph::GraphStore;
use crate::types::{GraphNode, SearchResult};

/// Graph-based search retriever.
pub struct GraphRetriever {
    db: Arc<tokio::sync::Mutex<Database>>,
}

impl GraphRetriever {
    /// Create a new GraphRetriever.
    pub fn new(db: Arc<tokio::sync::Mutex<Database>>) -> Self {
        Self { db }
    }

    /// Join a graph node to its chunk to get content and metadata.
    async fn node_to_search_result(&self, node: &GraphNode) -> Result<Option<SearchResult>> {
        let db = self.db.lock().await;
        let mut stmt = db.conn().prepare(
            "SELECT c.file_path, c.start_line, c.end_line, c.symbol, c.kind, c.content, c.parent_context, c.language
             FROM chunks c
             WHERE c.symbol = ?1 AND c.file_path = ?2 AND c.symbol IS NOT NULL
             ORDER BY c.start_line LIMIT 1",
        )?;

        let mut rows = stmt.query_map([&node.symbol, &node.file_path], |row| {
            Ok(SearchResult {
                repo_name: None,
                file_path: row.get(0)?,
                start_line: row.get(1)?,
                end_line: row.get(2)?,
                symbol: row.get(3)?,
                kind: row.get(4)?,
                content: row.get(5)?,
                parent_context: row.get(6)?,
                language: row.get(7)?,
                score: 0.0, // Will be set below
            })
        })?;

        if let Some(row) = rows.next() {
            let result = row?;
            // Assign synthetic score based on rank (will be set by caller)
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl SearchStrategy for GraphRetriever {
    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<SearchResult>> {
        // Classify the query
        let decision = classify_query(query);

        let nodes = match decision {
            RoutingDecision::Hybrid => {
                // Non-structural query forced into graph mode → empty
                return Ok(vec![]);
            }
            RoutingDecision::Graph(gq) => {
                let db = self.db.lock().await;
                match gq.kind {
                    GraphQueryKind::Callers => {
                        if let Some(ref fp) = gq.file_path {
                            crate::store::graph::get_callers_filtered(
                                db.conn(),
                                &gq.symbol,
                                Some(fp),
                            )?
                        } else {
                            db.get_callers(&gq.symbol)?
                        }
                    }
                    GraphQueryKind::Dependents => {
                        db.get_dependents(&gq.symbol, gq.file_path.as_deref())?
                    }
                    GraphQueryKind::Imports => {
                        db.get_imports(&gq.symbol, gq.file_path.as_deref())?
                    }
                }
            }
        };

        // Join nodes to chunks and assign scores
        let mut results = Vec::new();
        for (rank, node) in nodes.iter().enumerate() {
            if let Some(mut result) = self.node_to_search_result(node).await? {
                result.score = 1.0 / (rank + 1) as f32;
                results.push(result);
            }
        }

        // Apply limit
        results.truncate(options.limit);

        Ok(results)
    }

    fn mode(&self) -> SearchMode {
        SearchMode::Graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::Database;
    use crate::types::{EdgeType, GraphEdge};

    fn setup_test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(64).unwrap();
        db
    }

    #[tokio::test]
    async fn mode_returns_graph() {
        let db = setup_test_db();
        let retriever = GraphRetriever::new(Arc::new(tokio::sync::Mutex::new(db)));
        assert_eq!(retriever.mode(), SearchMode::Graph);
    }

    #[tokio::test]
    async fn non_structural_returns_empty() {
        let db = setup_test_db();
        let retriever = GraphRetriever::new(Arc::new(tokio::sync::Mutex::new(db)));
        let results = retriever
            .search("how does authentication work", SearchOptions::default())
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn callers_query_returns_results_with_content() {
        let db = setup_test_db();

        // Insert graph nodes
        let main = GraphNode {
            id: "main".into(),
            symbol: "main".into(),
            kind: "function".into(),
            file_path: "src/main.rs".into(),
        };
        let search = GraphNode {
            id: "search".into(),
            symbol: "search".into(),
            kind: "function".into(),
            file_path: "src/search.rs".into(),
        };
        db.insert_nodes(&[main.clone(), search.clone()]).unwrap();

        // Insert edge: main calls search
        db.insert_edges(&[GraphEdge {
            source_id: "main".into(),
            target_symbol: "search".into(),
            edge_type: EdgeType::Call,
        }])
        .unwrap();

        // Insert chunk for main
        use crate::store::chunks;
        use crate::types::Chunk;
        let chunk = Chunk {
            id: "chunk1".into(),
            file_path: "src/main.rs".into(),
            start_line: 1,
            end_line: 10,
            byte_start: 0,
            byte_end: 100,
            symbol: Some("main".into()),
            kind: "function".into(),
            content: "fn main() { search(); }".into(),
            parent_context: None,
            language: "rust".into(),
            file_mtime: 1234567890,
            content_hash: "hash".into(),
        };
        chunks::insert_chunk(db.conn(), &chunk).unwrap();

        let retriever = GraphRetriever::new(Arc::new(tokio::sync::Mutex::new(db)));
        let results = retriever
            .search("who calls search", SearchOptions::default())
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol.as_deref(), Some("main"));
        assert_eq!(results[0].file_path, "src/main.rs");
        assert!(results[0].content.contains("search"));
    }

    #[tokio::test]
    async fn scores_descending() {
        let db = setup_test_db();

        // Insert 3 nodes that all call "target"
        let target = GraphNode {
            id: "target".into(),
            symbol: "target".into(),
            kind: "function".into(),
            file_path: "src/target.rs".into(),
        };
        let caller1 = GraphNode {
            id: "caller1".into(),
            symbol: "caller1".into(),
            kind: "function".into(),
            file_path: "src/c1.rs".into(),
        };
        let caller2 = GraphNode {
            id: "caller2".into(),
            symbol: "caller2".into(),
            kind: "function".into(),
            file_path: "src/c2.rs".into(),
        };
        let caller3 = GraphNode {
            id: "caller3".into(),
            symbol: "caller3".into(),
            kind: "function".into(),
            file_path: "src/c3.rs".into(),
        };
        db.insert_nodes(&[target, caller1.clone(), caller2.clone(), caller3.clone()])
            .unwrap();

        db.insert_edges(&[
            GraphEdge {
                source_id: "caller1".into(),
                target_symbol: "target".into(),
                edge_type: EdgeType::Call,
            },
            GraphEdge {
                source_id: "caller2".into(),
                target_symbol: "target".into(),
                edge_type: EdgeType::Call,
            },
            GraphEdge {
                source_id: "caller3".into(),
                target_symbol: "target".into(),
                edge_type: EdgeType::Call,
            },
        ])
        .unwrap();

        // Insert chunks for each caller
        use crate::store::chunks;
        use crate::types::Chunk;
        for (i, caller) in [caller1, caller2, caller3].iter().enumerate() {
            let chunk = Chunk {
                id: format!("chunk{i}"),
                file_path: caller.file_path.clone(),
                start_line: 1,
                end_line: 10,
                byte_start: 0,
                byte_end: 100,
                symbol: Some(caller.symbol.clone()),
                kind: "function".into(),
                content: format!("fn {}() {{ target(); }}", caller.symbol),
                parent_context: None,
                language: "rust".into(),
                file_mtime: 1234567890,
                content_hash: "hash".into(),
            };
            chunks::insert_chunk(db.conn(), &chunk).unwrap();
        }

        let retriever = GraphRetriever::new(Arc::new(tokio::sync::Mutex::new(db)));
        let results = retriever
            .search("who calls target", SearchOptions::default())
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        // Scores should be descending: 1.0, 0.5, 0.333...
        assert!((results[0].score - 1.0).abs() < 0.01);
        assert!((results[1].score - 0.5).abs() < 0.01);
        assert!((results[2].score - 0.333).abs() < 0.01);
    }
}
