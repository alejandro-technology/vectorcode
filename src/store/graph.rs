use crate::store::db::Database;
use crate::types::{EdgeType, GraphEdge, GraphNode};
use anyhow::Result;
use rusqlite::params;

pub trait GraphStore {
    fn insert_nodes(&self, nodes: &[GraphNode]) -> Result<()>;
    fn insert_edges(&self, edges: &[GraphEdge]) -> Result<()>;
    fn get_callers(&self, symbol: &str) -> Result<Vec<GraphNode>>;
    fn get_callees(&self, symbol: &str) -> Result<Vec<GraphNode>>;
    fn get_dependents(&self, symbol: &str, file_path: Option<&str>) -> Result<Vec<GraphNode>>;
    fn get_imports(&self, symbol: &str, file_path: Option<&str>) -> Result<Vec<GraphNode>>;
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

/// Find callers of `symbol`, optionally disambiguating the callee by file_path.
pub fn get_callers_filtered(
    conn: &rusqlite::Connection,
    symbol: &str,
    file_path: Option<&str>,
) -> Result<Vec<GraphNode>> {
    let mut stmt = conn.prepare(
        "SELECT src.id, src.symbol, src.kind, src.file_path
         FROM graph_nodes src
         JOIN graph_edges e   ON src.id = e.source_id
         JOIN graph_nodes tgt ON e.target_symbol = tgt.symbol
         WHERE e.edge_type = 'Call' AND tgt.symbol = ?1
           AND (?2 IS NULL OR tgt.file_path = ?2)",
    )?;

    let node_iter = stmt.query_map(params![symbol, file_path], |row| {
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

/// Find nodes that depend on `symbol` via Import, Extends, or Reference edges.
pub fn get_dependents(
    conn: &rusqlite::Connection,
    symbol: &str,
    file_path: Option<&str>,
) -> Result<Vec<GraphNode>> {
    let mut stmt = conn.prepare(
        "SELECT src.id, src.symbol, src.kind, src.file_path
         FROM graph_nodes src
         JOIN graph_edges e   ON src.id = e.source_id
         JOIN graph_nodes tgt ON e.target_symbol = tgt.symbol
         WHERE e.edge_type IN ('Import','Extends','Reference')
           AND tgt.symbol = ?1
           AND (?2 IS NULL OR tgt.file_path = ?2)",
    )?;

    let node_iter = stmt.query_map(params![symbol, file_path], |row| {
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

/// Find outgoing Import edges from `symbol`, surfacing external targets as synthetic nodes.
pub fn get_imports(
    conn: &rusqlite::Connection,
    symbol: &str,
    file_path: Option<&str>,
) -> Result<Vec<GraphNode>> {
    let mut stmt = conn.prepare(
        "SELECT e.target_symbol,
                tgt.id, tgt.kind, tgt.file_path
         FROM graph_nodes src
         JOIN graph_edges e   ON src.id = e.source_id
         LEFT JOIN graph_nodes tgt ON e.target_symbol = tgt.symbol
         WHERE e.edge_type = 'Import'
           AND src.symbol = ?1
           AND (?2 IS NULL OR src.file_path = ?2)",
    )?;

    let node_iter = stmt.query_map(params![symbol, file_path], |row| {
        let target_symbol: String = row.get(0)?;
        let tgt_id: Option<String> = row.get(1)?;
        let tgt_kind: Option<String> = row.get(2)?;
        let tgt_file: Option<String> = row.get(3)?;

        Ok(GraphNode {
            id: tgt_id.unwrap_or_else(|| format!("ext:{target_symbol}")),
            symbol: target_symbol,
            kind: tgt_kind.unwrap_or_else(|| "external".to_string()),
            file_path: tgt_file.unwrap_or_default(),
        })
    })?;

    let mut result = Vec::new();
    for node in node_iter {
        result.push(node?);
    }
    Ok(result)
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

    fn get_dependents(&self, symbol: &str, file_path: Option<&str>) -> Result<Vec<GraphNode>> {
        get_dependents(self.conn(), symbol, file_path)
    }

    fn get_imports(&self, symbol: &str, file_path: Option<&str>) -> Result<Vec<GraphNode>> {
        get_imports(self.conn(), symbol, file_path)
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

    // ─── get_dependents tests ─────────────────────────────────────────────

    #[test]
    fn get_dependents_returns_importers_extenders_referencers() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let base = GraphNode {
            id: "base".into(),
            symbol: "Base".into(),
            kind: "class".into(),
            file_path: "src/base.rs".into(),
        };
        let importer = GraphNode {
            id: "importer".into(),
            symbol: "importer_mod".into(),
            kind: "module".into(),
            file_path: "src/importer.rs".into(),
        };
        let extender = GraphNode {
            id: "extender".into(),
            symbol: "Derived".into(),
            kind: "class".into(),
            file_path: "src/derived.rs".into(),
        };
        let referencer = GraphNode {
            id: "ref".into(),
            symbol: "uses_base".into(),
            kind: "function".into(),
            file_path: "src/user.rs".into(),
        };

        db.insert_nodes(&[
            base.clone(),
            importer.clone(),
            extender.clone(),
            referencer.clone(),
        ])
        .unwrap();

        db.insert_edges(&[
            GraphEdge {
                source_id: "importer".into(),
                target_symbol: "Base".into(),
                edge_type: EdgeType::Import,
            },
            GraphEdge {
                source_id: "extender".into(),
                target_symbol: "Base".into(),
                edge_type: EdgeType::Extends,
            },
            GraphEdge {
                source_id: "ref".into(),
                target_symbol: "Base".into(),
                edge_type: EdgeType::Reference,
            },
        ])
        .unwrap();

        let dependents = db.get_dependents("Base", None).unwrap();
        assert_eq!(dependents.len(), 3);
        let symbols: Vec<&str> = dependents.iter().map(|n| n.symbol.as_str()).collect();
        assert!(symbols.contains(&"importer_mod"));
        assert!(symbols.contains(&"Derived"));
        assert!(symbols.contains(&"uses_base"));
    }

    #[test]
    fn get_dependents_excludes_call_edges() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let target = GraphNode {
            id: "target".into(),
            symbol: "Foo".into(),
            kind: "function".into(),
            file_path: "src/foo.rs".into(),
        };
        let caller = GraphNode {
            id: "caller".into(),
            symbol: "bar".into(),
            kind: "function".into(),
            file_path: "src/bar.rs".into(),
        };
        let importer = GraphNode {
            id: "importer".into(),
            symbol: "baz".into(),
            kind: "function".into(),
            file_path: "src/baz.rs".into(),
        };

        db.insert_nodes(&[target, caller.clone(), importer.clone()])
            .unwrap();

        db.insert_edges(&[
            GraphEdge {
                source_id: "caller".into(),
                target_symbol: "Foo".into(),
                edge_type: EdgeType::Call,
            },
            GraphEdge {
                source_id: "importer".into(),
                target_symbol: "Foo".into(),
                edge_type: EdgeType::Import,
            },
        ])
        .unwrap();

        let dependents = db.get_dependents("Foo", None).unwrap();
        assert_eq!(dependents.len(), 1);
        assert_eq!(dependents[0].symbol, "baz");
    }

    #[test]
    fn get_dependents_file_path_disambiguates_target() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let foo = GraphNode {
            id: "foo".into(),
            symbol: "Foo".into(),
            kind: "class".into(),
            file_path: "src/foo.rs".into(),
        };
        let bar = GraphNode {
            id: "bar".into(),
            symbol: "Bar".into(),
            kind: "class".into(),
            file_path: "src/bar.rs".into(),
        };
        let dep_foo = GraphNode {
            id: "dep_foo".into(),
            symbol: "uses_foo".into(),
            kind: "function".into(),
            file_path: "src/user_foo.rs".into(),
        };
        let dep_bar = GraphNode {
            id: "dep_bar".into(),
            symbol: "uses_bar".into(),
            kind: "function".into(),
            file_path: "src/user_bar.rs".into(),
        };

        db.insert_nodes(&[foo, bar, dep_foo.clone(), dep_bar.clone()])
            .unwrap();

        db.insert_edges(&[
            GraphEdge {
                source_id: "dep_foo".into(),
                target_symbol: "Foo".into(),
                edge_type: EdgeType::Import,
            },
            GraphEdge {
                source_id: "dep_bar".into(),
                target_symbol: "Bar".into(),
                edge_type: EdgeType::Import,
            },
        ])
        .unwrap();

        // Without file_path: both dependents returned (different symbols)
        let all_foo = db.get_dependents("Foo", None).unwrap();
        assert_eq!(all_foo.len(), 1);
        assert_eq!(all_foo[0].symbol, "uses_foo");

        // With file_path matching the target: still returns the dependent
        let filtered = db.get_dependents("Foo", Some("src/foo.rs")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].symbol, "uses_foo");

        // With file_path NOT matching the target: no results
        let empty = db
            .get_dependents("Foo", Some("src/nonexistent.rs"))
            .unwrap();
        assert!(empty.is_empty());
    }

    // ─── get_imports tests ────────────────────────────────────────────────

    #[test]
    fn get_imports_returns_outgoing_targets() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let module = GraphNode {
            id: "mod".into(),
            symbol: "my_module".into(),
            kind: "module".into(),
            file_path: "src/my_module.rs".into(),
        };
        let bar = GraphNode {
            id: "bar".into(),
            symbol: "Bar".into(),
            kind: "class".into(),
            file_path: "src/bar.rs".into(),
        };
        let baz = GraphNode {
            id: "baz".into(),
            symbol: "Baz".into(),
            kind: "class".into(),
            file_path: "src/baz.rs".into(),
        };

        db.insert_nodes(&[module.clone(), bar.clone(), baz.clone()])
            .unwrap();

        db.insert_edges(&[
            GraphEdge {
                source_id: "mod".into(),
                target_symbol: "Bar".into(),
                edge_type: EdgeType::Import,
            },
            GraphEdge {
                source_id: "mod".into(),
                target_symbol: "Baz".into(),
                edge_type: EdgeType::Import,
            },
        ])
        .unwrap();

        let imports = db.get_imports("my_module", None).unwrap();
        assert_eq!(imports.len(), 2);
        let symbols: Vec<&str> = imports.iter().map(|n| n.symbol.as_str()).collect();
        assert!(symbols.contains(&"Bar"));
        assert!(symbols.contains(&"Baz"));
    }

    #[test]
    fn get_imports_left_join_surfaces_external() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let module = GraphNode {
            id: "mod".into(),
            symbol: "my_module".into(),
            kind: "module".into(),
            file_path: "src/my_module.rs".into(),
        };
        db.insert_nodes(&[module]).unwrap();

        // Import an external symbol (not in graph_nodes)
        db.insert_edges(&[GraphEdge {
            source_id: "mod".into(),
            target_symbol: "std::fmt::Display".into(),
            edge_type: EdgeType::Import,
        }])
        .unwrap();

        let imports = db.get_imports("my_module", None).unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].symbol, "std::fmt::Display");
        // External node should have synthetic id and empty file_path
        assert!(imports[0].id.starts_with("ext:"));
        assert_eq!(imports[0].file_path, "");
    }

    #[test]
    fn get_imports_file_path_disambiguates_source() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let mod_a = GraphNode {
            id: "mod_a".into(),
            symbol: "mod_a".into(),
            kind: "module".into(),
            file_path: "src/a.rs".into(),
        };
        let mod_b = GraphNode {
            id: "mod_b".into(),
            symbol: "mod_b".into(),
            kind: "module".into(),
            file_path: "src/b.rs".into(),
        };
        let bar = GraphNode {
            id: "bar".into(),
            symbol: "Bar".into(),
            kind: "class".into(),
            file_path: "src/bar.rs".into(),
        };
        let baz = GraphNode {
            id: "baz".into(),
            symbol: "Baz".into(),
            kind: "class".into(),
            file_path: "src/baz.rs".into(),
        };

        db.insert_nodes(&[mod_a, mod_b, bar, baz]).unwrap();

        db.insert_edges(&[
            GraphEdge {
                source_id: "mod_a".into(),
                target_symbol: "Bar".into(),
                edge_type: EdgeType::Import,
            },
            GraphEdge {
                source_id: "mod_b".into(),
                target_symbol: "Baz".into(),
                edge_type: EdgeType::Import,
            },
        ])
        .unwrap();

        // mod_a imports Bar
        let imports_a = db.get_imports("mod_a", None).unwrap();
        assert_eq!(imports_a.len(), 1);
        assert_eq!(imports_a[0].symbol, "Bar");

        // mod_b imports Baz
        let imports_b = db.get_imports("mod_b", None).unwrap();
        assert_eq!(imports_b.len(), 1);
        assert_eq!(imports_b[0].symbol, "Baz");

        // With file_path matching: still returns the import
        let filtered = db.get_imports("mod_a", Some("src/a.rs")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].symbol, "Bar");

        // With file_path NOT matching: no imports
        let empty = db.get_imports("mod_a", Some("src/nonexistent.rs")).unwrap();
        assert!(empty.is_empty());
    }

    // ─── get_callers_filtered tests ───────────────────────────────────────

    #[test]
    fn get_callers_filtered_disambiguates_target_file() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(4).unwrap();

        let foo = GraphNode {
            id: "foo".into(),
            symbol: "foo".into(),
            kind: "function".into(),
            file_path: "src/a.rs".into(),
        };
        let bar = GraphNode {
            id: "bar".into(),
            symbol: "bar".into(),
            kind: "function".into(),
            file_path: "src/b.rs".into(),
        };
        let caller = GraphNode {
            id: "caller".into(),
            symbol: "main".into(),
            kind: "function".into(),
            file_path: "src/main.rs".into(),
        };

        db.insert_nodes(&[foo, bar.clone(), caller.clone()])
            .unwrap();

        // caller calls both foo and bar
        db.insert_edges(&[
            GraphEdge {
                source_id: "caller".into(),
                target_symbol: "foo".into(),
                edge_type: EdgeType::Call,
            },
            GraphEdge {
                source_id: "caller".into(),
                target_symbol: "bar".into(),
                edge_type: EdgeType::Call,
            },
        ])
        .unwrap();

        // Without file_path: caller returned for foo
        let all = get_callers_filtered(db.conn(), "foo", None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].symbol, "main");

        // With file_path matching the target: still returns caller
        let filtered = get_callers_filtered(db.conn(), "foo", Some("src/a.rs")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].symbol, "main");

        // With file_path NOT matching the target: no callers
        let empty = get_callers_filtered(db.conn(), "foo", Some("src/nonexistent.rs")).unwrap();
        assert!(empty.is_empty());
    }
}
