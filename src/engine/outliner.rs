//! AST-based file outliner — extracts top-level symbols for quick navigation.

use serde::Serialize;

use super::languages::SupportedLanguage;
use tree_sitter::Parser;

/// A single outline item extracted from a source file.
#[derive(Debug, Clone, Serialize)]
pub struct OutlineItem {
    /// Display kind: "fn", "struct", "enum", "trait", "impl", "class", "function", "def", etc.
    pub kind: String,
    /// Symbol name (function name, struct name, etc.)
    pub name: String,
    /// Signature text (from node start to body start)
    pub signature: String,
    /// 1-indexed start line
    pub start_line: u32,
    /// Visibility modifier (e.g., "pub", "pub(crate)", "export")
    pub visibility: Option<String>,
}

thread_local! {
    static THREAD_PARSER: std::cell::RefCell<Parser> = std::cell::RefCell::new(Parser::new());
}

/// Outline information about a node type: (display_kind, body_child_kind).
/// body_child_kind is None when there's no body (e.g., type aliases).
struct OutlineNodeInfo {
    display_kind: &'static str,
    body_child_kind: Option<&'static str>,
}

/// Return outlineable node types and their display info for a language.
fn outlineable_node_types(language: SupportedLanguage) -> Vec<(&'static str, OutlineNodeInfo)> {
    match language {
        SupportedLanguage::Rust => vec![
            (
                "function_item",
                OutlineNodeInfo {
                    display_kind: "fn",
                    body_child_kind: Some("block"),
                },
            ),
            (
                "struct_item",
                OutlineNodeInfo {
                    display_kind: "struct",
                    body_child_kind: Some("field_declaration_list"),
                },
            ),
            (
                "enum_item",
                OutlineNodeInfo {
                    display_kind: "enum",
                    body_child_kind: Some("enum_variant_list"),
                },
            ),
            (
                "trait_item",
                OutlineNodeInfo {
                    display_kind: "trait",
                    body_child_kind: Some("declaration_list"),
                },
            ),
            (
                "impl_item",
                OutlineNodeInfo {
                    display_kind: "impl",
                    body_child_kind: Some("declaration_list"),
                },
            ),
        ],
        SupportedLanguage::TypeScript | SupportedLanguage::Tsx => vec![
            (
                "function_declaration",
                OutlineNodeInfo {
                    display_kind: "function",
                    body_child_kind: Some("statement_block"),
                },
            ),
            (
                "class_declaration",
                OutlineNodeInfo {
                    display_kind: "class",
                    body_child_kind: Some("class_body"),
                },
            ),
            (
                "interface_declaration",
                OutlineNodeInfo {
                    display_kind: "interface",
                    body_child_kind: Some("interface_body"),
                },
            ),
            (
                "type_alias_declaration",
                OutlineNodeInfo {
                    display_kind: "type",
                    body_child_kind: None,
                },
            ),
        ],
        SupportedLanguage::JavaScript | SupportedLanguage::Jsx => vec![
            (
                "function_declaration",
                OutlineNodeInfo {
                    display_kind: "function",
                    body_child_kind: Some("statement_block"),
                },
            ),
            (
                "class_declaration",
                OutlineNodeInfo {
                    display_kind: "class",
                    body_child_kind: Some("class_body"),
                },
            ),
        ],
        SupportedLanguage::Python => vec![
            (
                "function_definition",
                OutlineNodeInfo {
                    display_kind: "def",
                    body_child_kind: Some("block"),
                },
            ),
            (
                "class_definition",
                OutlineNodeInfo {
                    display_kind: "class",
                    body_child_kind: Some("block"),
                },
            ),
        ],
        _ => vec![],
    }
}

/// Extract the name from an AST node by finding identifier children.
fn extract_name(node: &tree_sitter::Node, source: &str) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "name" | "type_identifier" | "property_identifier" => {
                return source[child.byte_range()].to_string();
            }
            _ => {}
        }
    }
    // For impl blocks, try to find the target type
    if node.kind() == "impl_item" {
        let mut cursor2 = node.walk();
        for child in node.children(&mut cursor2) {
            if child.kind() == "type_identifier" {
                return source[child.byte_range()].to_string();
            }
        }
    }
    String::new()
}

/// Extract visibility modifier from an AST node (Rust-specific).
fn extract_visibility(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            return Some(source[child.byte_range()].to_string());
        }
    }
    None
}

/// Extract signature: source slice from node start to body child start.
fn extract_signature(
    node: &tree_sitter::Node,
    source: &str,
    body_child_kind: Option<&str>,
) -> String {
    if let Some(body_kind) = body_child_kind {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == body_kind {
                let sig = &source[node.start_byte()..child.start_byte()];
                return sig.trim_end().to_string();
            }
        }
    }
    // No body child found — use the full node text
    source[node.byte_range()].trim_end().to_string()
}

/// Outline a source file, returning top-level symbols.
///
/// Uses tree-sitter to parse the source and extract outline items for
/// supported languages (Rust, TypeScript, Python). Returns an empty
/// vector for unsupported languages or parse failures.
pub fn outline_file(
    source: &str,
    _file_path: &str,
    language: SupportedLanguage,
) -> Vec<OutlineItem> {
    let ts_language = match language.tree_sitter_language() {
        Some(lang) => lang,
        None => return Vec::new(),
    };

    let outline_types = outlineable_node_types(language);
    if outline_types.is_empty() {
        return Vec::new();
    }

    let parse_res = THREAD_PARSER.with(|parser_cell| {
        let mut parser = parser_cell.borrow_mut();
        if parser.set_language(&ts_language).is_err() {
            return None;
        }
        parser.parse(source, None)
    });

    let tree = match parse_res {
        Some(tree) => tree,
        None => return Vec::new(),
    };

    let mut items = Vec::new();
    let root = tree.root_node();

    for child in root.children(&mut root.walk()) {
        process_node(&child, source, &outline_types, &mut items);
    }

    items
}

/// Process a single AST node, descending into wrapper nodes as needed.
fn process_node(
    node: &tree_sitter::Node,
    source: &str,
    outline_types: &[(&str, OutlineNodeInfo)],
    items: &mut Vec<OutlineItem>,
) {
    let kind = node.kind();

    // Descend into export_statement (TypeScript)
    if kind == "export_statement" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            process_node(&child, source, outline_types, items);
        }
        return;
    }

    // Descend into decorated_definition (Python)
    if kind == "decorated_definition" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            process_node(&child, source, outline_types, items);
        }
        return;
    }

    // Check if this node is an outlineable type
    if let Some((_kind_str, info)) = outline_types.iter().find(|(k, _)| *k == kind) {
        let name = extract_name(node, source);
        let signature = extract_signature(node, source, info.body_child_kind);
        let visibility = extract_visibility(node, source);
        let start_line = (node.start_position().row + 1) as u32;

        items.push(OutlineItem {
            kind: info.display_kind.to_string(),
            name,
            signature,
            start_line,
            visibility,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_rust_file_with_structs_and_fns() {
        let source = r#"
pub fn calculate_sum(a: i32, b: i32) -> i32 {
    a + b
}

pub struct Calculator {
    value: i32,
}

enum Operation {
    Add,
    Subtract,
}

trait Computable {
    fn compute(&self) -> i32;
}

impl Calculator {
    pub fn new() -> Self {
        Self { value: 0 }
    }
}
"#;
        let items = outline_file(source, "test.rs", SupportedLanguage::Rust);
        assert!(
            items.len() >= 4,
            "Should find at least 4 items (fn, struct, enum, trait, impl), got {}",
            items.len()
        );

        let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        assert!(kinds.contains(&"fn"), "Should have fn, got: {kinds:?}");
        assert!(kinds.contains(&"struct"), "Should have struct");
        assert!(kinds.contains(&"enum"), "Should have enum");
        assert!(kinds.contains(&"trait"), "Should have trait");
        assert!(kinds.contains(&"impl"), "Should have impl");

        let calc_fn = items.iter().find(|i| i.name == "calculate_sum").unwrap();
        assert_eq!(calc_fn.kind, "fn");
        assert_eq!(calc_fn.visibility.as_deref(), Some("pub"));
        assert!(calc_fn.signature.contains("calculate_sum"));
        assert!(calc_fn.start_line > 0);
    }

    #[test]
    fn outline_typescript_file_with_classes_and_interfaces() {
        let source = r#"
export class Calculator {
    private value: number;

    constructor(initial: number) {
        this.value = initial;
    }

    add(n: number): number {
        return this.value + n;
    }
}

export interface Computable {
    compute(): number;
}

export function calculate(a: number, b: number): number {
    return a + b;
}

export type Result = {
    value: number;
};
"#;
        let items = outline_file(source, "test.ts", SupportedLanguage::TypeScript);
        assert!(
            items.len() >= 3,
            "Should find at least 3 items (class, interface, function), got {}",
            items.len()
        );

        let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        assert!(
            kinds.contains(&"class"),
            "Should have class, got: {kinds:?}"
        );
        assert!(kinds.contains(&"interface"), "Should have interface");
        assert!(kinds.contains(&"function"), "Should have function");

        let calc_class = items.iter().find(|i| i.name == "Calculator").unwrap();
        assert_eq!(calc_class.kind, "class");
        assert!(calc_class.signature.contains("Calculator"));
    }

    #[test]
    fn outline_python_file_with_class_and_defs() {
        let source = r#"
import functools

@functools.lru_cache()
def calculate_sum(a: int, b: int) -> int:
    return a + b

class Calculator:
    def __init__(self, initial: int = 0):
        self.value = initial

    def add(self, n: int) -> int:
        return self.value + n
"#;
        let items = outline_file(source, "test.py", SupportedLanguage::Python);
        assert!(
            items.len() >= 2,
            "Should find at least 2 items (def, class), got {}",
            items.len()
        );

        let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        assert!(kinds.contains(&"def"), "Should have def, got: {kinds:?}");
        assert!(kinds.contains(&"class"), "Should have class");

        let calc_fn = items.iter().find(|i| i.name == "calculate_sum").unwrap();
        assert_eq!(calc_fn.kind, "def");
        assert!(calc_fn.signature.contains("calculate_sum"));
    }

    #[test]
    fn outline_unknown_language_returns_empty() {
        let source = "package main\n\nfunc main() {}\n";
        let items = outline_file(source, "main.go", SupportedLanguage::Go);
        assert!(
            items.is_empty(),
            "Go is not supported for outlining, should return empty"
        );
    }

    #[test]
    fn outline_empty_source_returns_empty() {
        let items = outline_file("", "empty.rs", SupportedLanguage::Rust);
        assert!(items.is_empty(), "Empty source should return empty outline");
    }
}
