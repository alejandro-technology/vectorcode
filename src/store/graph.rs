use crate::store::db::Database;
use crate::types::{EdgeType, GraphEdge, GraphNode};
use anyhow::Result;
use rusqlite::params;

pub trait GraphStore {
    fn insert_nodes(&self, nodes: &[GraphNode]) -> Result<()>;
    fn insert_edges(&self, edges: &[GraphEdge]) -> Result<()>;
    fn get_callers(&self, symbol: &str) -> Result<Vec<GraphNode>>;
    fn get_callees(&self, symbol: &str) -> Result<Vec<GraphNode>>;
    fn delete_nodes_by_file(&self, file_path: &str) -> Result<()>;
}

pub fn delete_nodes_by_file(conn: &rusqlite::Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM graph_nodes WHERE file_path = ?1",
        params![file_path],
    )?;
    Ok(())
}

pub fn insert_nodes(conn: &rusqlite::Connection, nodes: &[GraphNode]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO graph_nodes (id, symbol, kind, file_path) VALUES (?1, ?2, ?3, ?4)",
    )?;

    for node in nodes {
        stmt.execute(params![node.id, node.symbol, node.kind, node.file_path])?;
    }
    Ok(())
}

pub fn insert_edges(conn: &rusqlite::Connection, edges: &[GraphEdge]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO graph_edges (source_id, target_symbol, edge_type) VALUES (?1, ?2, ?3)",
    )?;

    for edge in edges {
        let edge_type_str = match edge.edge_type {
            EdgeType::Call => "Call",
            EdgeType::Import => "Import",
            EdgeType::Reference => "Reference",
            EdgeType::Extends => "Extends",
        };
        stmt.execute(params![edge.source_id, edge.target_symbol, edge_type_str])?;
    }
    Ok(())
}

impl GraphStore for Database {
    fn delete_nodes_by_file(&self, file_path: &str) -> Result<()> {
        delete_nodes_by_file(self.conn(), file_path)
    }
    fn insert_nodes(&self, nodes: &[GraphNode]) -> Result<()> {
        insert_nodes(self.conn(), nodes)
    }

    fn insert_edges(&self, edges: &[GraphEdge]) -> Result<()> {
        insert_edges(self.conn(), edges)
    }

    fn get_callers(&self, symbol: &str) -> Result<Vec<GraphNode>> {
        // Late resolution: find graph_edges where target_symbol == symbol, and join with graph_nodes to get the caller (source_id).
        let mut stmt = self.conn().prepare(
            "SELECT n.id, n.symbol, n.kind, n.file_path
             FROM graph_nodes n
             JOIN graph_edges e ON n.id = e.source_id
             WHERE e.target_symbol = ?1 AND e.edge_type = 'Call'",
        )?;

        let node_iter = stmt.query_map(params![symbol], |row| {
            Ok(GraphNode {
                id: row.get(0)?,
                symbol: row.get(1)?,
                kind: row.get(2)?,
                file_path: row.get(3)?,
            })
        })?;

        let mut result = Vec::new();
        for node in node_iter {
            result.push(node?);
        }
        Ok(result)
    }

    fn get_callees(&self, symbol: &str) -> Result<Vec<GraphNode>> {
        // Late resolution: find graph_nodes where symbol == symbol, find their out-edges, and join to target graph_nodes by target_symbol.
        let mut stmt = self.conn().prepare(
            "SELECT n_target.id, n_target.symbol, n_target.kind, n_target.file_path
             FROM graph_nodes n_source
             JOIN graph_edges e ON n_source.id = e.source_id
             JOIN graph_nodes n_target ON e.target_symbol = n_target.symbol
             WHERE n_source.symbol = ?1 AND e.edge_type = 'Call'",
        )?;

        let node_iter = stmt.query_map(params![symbol], |row| {
            Ok(GraphNode {
                id: row.get(0)?,
                symbol: row.get(1)?,
                kind: row.get(2)?,
                file_path: row.get(3)?,
            })
        })?;

        let mut result = Vec::new();
        for node in node_iter {
            result.push(node?);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::Database;

    #[test]
    fn test_graph_store() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let n1 = GraphNode {
            id: "1".into(),
            symbol: "caller_func".into(),
            kind: "function".into(),
            file_path: "src/main.rs".into(),
        };
        let n2 = GraphNode {
            id: "2".into(),
            symbol: "callee_func".into(),
            kind: "function".into(),
            file_path: "src/lib.rs".into(),
        };

        db.insert_nodes(&[n1.clone(), n2.clone()]).unwrap();

        let edge = GraphEdge {
            source_id: "1".into(),
            target_symbol: "callee_func".into(),
            edge_type: EdgeType::Call,
        };

        db.insert_edges(&[edge]).unwrap();

        let callers = db.get_callers("callee_func").unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].symbol, "caller_func");

        let callees = db.get_callees("caller_func").unwrap();
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].symbol, "callee_func");
    }
}
