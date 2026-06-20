use crate::engine::languages::SupportedLanguage;
use crate::types::{EdgeType, GraphEdge, GraphNode};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

/// Extract nodes and edges using Tree-sitter queries.
pub fn extract_graph(
    source: &str,
    file_path: &str,
    language: SupportedLanguage,
) -> (Vec<GraphNode>, Vec<GraphEdge>) {
    let ts_lang = match language.tree_sitter_language() {
        Some(lang) => lang,
        None => return (Vec::new(), Vec::new()),
    };

    let (nodes_query_str, edges_query_str) = match language {
        SupportedLanguage::Rust => (RUST_NODES_QUERY, RUST_EDGES_QUERY),
        SupportedLanguage::TypeScript
        | SupportedLanguage::JavaScript
        | SupportedLanguage::Tsx
        | SupportedLanguage::Jsx => (TS_NODES_QUERY, TS_EDGES_QUERY),
        SupportedLanguage::Python => (PYTHON_NODES_QUERY, PYTHON_EDGES_QUERY),
        _ => return (Vec::new(), Vec::new()), // Unsupported for now
    };

    let nodes_query = match Query::new(&ts_lang, nodes_query_str) {
        Ok(q) => q,
        Err(_) => return (Vec::new(), Vec::new()),
    };

    let edges_query = match Query::new(&ts_lang, edges_query_str) {
        Ok(q) => q,
        Err(_) => return (Vec::new(), Vec::new()),
    };

    let mut parser = Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return (Vec::new(), Vec::new());
    }

    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return (Vec::new(), Vec::new()),
    };

    let root_node = tree.root_node();
    let mut cursor = QueryCursor::new();

    let mut nodes = Vec::new();
    let mut node_ranges: Vec<(usize, usize, String)> = Vec::new(); // (start, end, id)

    // Add module/file node as fallback
    let module_id = blake3::hash(file_path.as_bytes()).to_hex().to_string();
    nodes.push(GraphNode {
        id: module_id.clone(),
        symbol: std::path::Path::new(file_path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        kind: "module".to_string(),
        file_path: file_path.to_string(),
    });

    // Extract Nodes
    let name_idx = nodes_query.capture_index_for_name("name");
    let mut matches = cursor.matches(&nodes_query, root_node, source.as_bytes());

    while let Some(m) = matches.next() {
        let mut symbol_name = None;
        let mut def_node = None;
        let mut kind = "definition".to_string();

        for cap in m.captures {
            if Some(cap.index) == name_idx {
                if let Ok(text) = cap.node.utf8_text(source.as_bytes()) {
                    symbol_name = Some(text.to_string());
                }
            } else {
                let capture_name = nodes_query.capture_names()[cap.index as usize];
                if capture_name.starts_with("def.") {
                    def_node = Some(cap.node);
                    kind = capture_name.replace("def.", "");
                }
            }
        }

        if let (Some(sym), Some(node)) = (symbol_name, def_node) {
            let id_input = format!("{}|{}", file_path, sym);
            let id = blake3::hash(id_input.as_bytes()).to_hex().to_string();

            nodes.push(GraphNode {
                id: id.clone(),
                symbol: sym,
                kind,
                file_path: file_path.to_string(),
            });
            node_ranges.push((node.start_byte(), node.end_byte(), id));
        }
    }

    // Sort node ranges by length (end - start) so smaller ranges (inner nodes) are matched first
    node_ranges.sort_by_key(|(s, e, _)| e - s);

    // Extract Edges
    let mut edges = Vec::new();
    let target_idx = edges_query.capture_index_for_name("target");

    let mut edges_cursor = QueryCursor::new();
    let mut edge_matches = edges_cursor.matches(&edges_query, root_node, source.as_bytes());

    while let Some(m) = edge_matches.next() {
        let mut target_symbol = None;
        let mut edge_type = EdgeType::Call;
        let mut edge_byte_pos = 0;

        for cap in m.captures {
            if Some(cap.index) == target_idx {
                if let Ok(text) = cap.node.utf8_text(source.as_bytes()) {
                    target_symbol = Some(text.to_string());
                    edge_byte_pos = cap.node.start_byte();
                }
            } else {
                let capture_name = edges_query.capture_names()[cap.index as usize];
                if capture_name == "import" {
                    edge_type = EdgeType::Import;
                } else if capture_name == "call" {
                    edge_type = EdgeType::Call;
                }
            }
        }

        if let Some(target) = target_symbol {
            // Find enclosing node
            let mut source_id = module_id.clone();
            for (start, end, id) in &node_ranges {
                if edge_byte_pos >= *start && edge_byte_pos <= *end {
                    source_id = id.clone();
                    break;
                }
            }

            edges.push(GraphEdge {
                source_id,
                target_symbol: target,
                edge_type,
            });
        }
    }

    (nodes, edges)
}

const RUST_NODES_QUERY: &str = r#"
(function_item name: (identifier) @name) @def.function
(impl_item type: (type_identifier) @name) @def.impl
(struct_item name: (type_identifier) @name) @def.struct
(trait_item name: (type_identifier) @name) @def.trait
"#;

const RUST_EDGES_QUERY: &str = r#"
(call_expression function: (identifier) @target) @call
(call_expression function: (field_expression field: (field_identifier) @target)) @call
(call_expression function: (scoped_identifier name: (identifier) @target)) @call
(macro_invocation macro: (identifier) @target) @call
(macro_invocation macro: (scoped_identifier name: (identifier) @target)) @call
(use_declaration argument: (scoped_identifier name: (identifier) @target)) @import
(use_declaration argument: (identifier) @target) @import
"#;

const TS_NODES_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @def.function
(method_definition name: (property_identifier) @name) @def.method
(class_declaration name: (identifier) @name) @def.class
(interface_declaration name: (identifier) @name) @def.interface
"#;

const TS_EDGES_QUERY: &str = r#"
(call_expression function: (identifier) @target) @call
(call_expression function: (member_expression property: (property_identifier) @target)) @call
(import_specifier name: (identifier) @target) @import
(import_clause (identifier) @target) @import
"#;

const PYTHON_NODES_QUERY: &str = r#"
(function_definition name: (identifier) @name) @def.function
(class_definition name: (identifier) @name) @def.class
"#;

const PYTHON_EDGES_QUERY: &str = r#"
(call function: (identifier) @target) @call
(call function: (attribute attribute: (identifier) @target)) @call
(import_from_statement name: (dotted_name (identifier) @target)) @import
(import_statement name: (dotted_name (identifier) @target)) @import
(aliased_import name: (dotted_name (identifier) @target)) @import
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::languages::SupportedLanguage;

    #[test]
    fn test_rust_graph_extraction() {
        let code = r#"
        use std::collections::HashMap;

        struct MyStruct;

        impl MyStruct {
            fn do_something() {
                println!("hello");
                helper_func();
            }
        }

        fn helper_func() {
            let x = 1;
        }
        "#;

        let (nodes, edges) = extract_graph(code, "src/main.rs", SupportedLanguage::Rust);

        // Should have module node, struct, impl, do_something, and helper_func.
        assert!(nodes
            .iter()
            .any(|n| n.symbol == "MyStruct" && n.kind == "struct"));
        assert!(nodes
            .iter()
            .any(|n| n.symbol == "do_something" && n.kind == "function"));
        assert!(nodes
            .iter()
            .any(|n| n.symbol == "helper_func" && n.kind == "function"));

        // Should have call to println, helper_func
        assert!(edges
            .iter()
            .any(|e| e.target_symbol == "println" && e.edge_type == EdgeType::Call));
        assert!(edges
            .iter()
            .any(|e| e.target_symbol == "helper_func" && e.edge_type == EdgeType::Call));

        // Should have import of HashMap
        assert!(edges
            .iter()
            .any(|e| e.target_symbol == "HashMap" && e.edge_type == EdgeType::Import));
    }
}
