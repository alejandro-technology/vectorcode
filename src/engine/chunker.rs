use super::languages::SupportedLanguage;
/// AST-aware chunking system.
use crate::types::{compute_chunk_id, compute_content_hash, Chunk};
use tree_sitter::Parser;

/// Minimum chunk size in bytes — chunks smaller than this are skipped.
const MIN_CHUNK_SIZE: usize = 100;

/// Maximum chunk size in bytes — chunks larger than this are split.
const MAX_CHUNK_SIZE: usize = 2000;

/// Line-based chunking window size (number of lines).
const LINE_WINDOW_SIZE: usize = 50;

/// Line-based chunking overlap (number of lines).
const LINE_OVERLAP: usize = 10;

/// Chunkable AST node types per language.
fn chunkable_node_types(language: SupportedLanguage) -> &'static [&'static str] {
    match language {
        SupportedLanguage::TypeScript | SupportedLanguage::Tsx => &[
            "function_declaration",
            "arrow_function",
            "method_definition",
            "class_declaration",
            "interface_declaration",
            "type_alias_declaration",
            "enum_declaration",
            "export_statement",
        ],
        SupportedLanguage::JavaScript | SupportedLanguage::Jsx => &[
            "function_declaration",
            "arrow_function",
            "method_definition",
            "class_declaration",
            "export_statement",
        ],
        SupportedLanguage::Python => &[
            "function_definition",
            "class_definition",
            "decorated_definition",
        ],
        SupportedLanguage::Rust => &[
            "function_item",
            "impl_item",
            "struct_item",
            "enum_item",
            "trait_item",
            "mod_item",
        ],
        SupportedLanguage::Go => &[
            "function_declaration",
            "method_declaration",
            "type_declaration",
        ],
        SupportedLanguage::Java => &[
            "method_declaration",
            "class_declaration",
            "interface_declaration",
            "enum_declaration",
        ],
        SupportedLanguage::CSharp => &[
            "method_declaration",
            "class_declaration",
            "interface_declaration",
            "enum_declaration",
            "namespace_declaration",
        ],
        SupportedLanguage::C => &["function_definition", "struct_specifier"],
        SupportedLanguage::Cpp => &[
            "function_definition",
            "class_specifier",
            "struct_specifier",
            "namespace_definition",
        ],
        SupportedLanguage::Ruby => &["method", "class", "module", "singleton_method"],
        SupportedLanguage::Swift => &[
            "function_declaration",
            "class_declaration",
            "protocol_declaration",
            "enum_declaration",
        ],
        SupportedLanguage::Kotlin => &[
            "function_declaration",
            "class_declaration",
            "object_declaration",
        ],
        SupportedLanguage::Unknown => &[],
    }
}

/// Chunk a source file into semantic units.
pub fn chunk_file(source: &str, file_path: &str, language: SupportedLanguage) -> Vec<Chunk> {
    let ts_language = match language.tree_sitter_language() {
        Some(lang) => lang,
        None => return line_based_chunks(source, file_path, language),
    };

    let mut parser = Parser::new();
    if parser.set_language(&ts_language).is_err() {
        return line_based_chunks(source, file_path, language);
    }

    let tree = match parser.parse(source, None) {
        Some(tree) => tree,
        None => return line_based_chunks(source, file_path, language),
    };

    let chunkable_types = chunkable_node_types(language);
    let mut chunks = Vec::new();

    let root = tree.root_node();
    for child in root.children(&mut root.walk()) {
        if chunkable_types.contains(&child.kind()) {
            let node_source = &source[child.byte_range()];
            let size = node_source.len();

            if size < MIN_CHUNK_SIZE {
                continue;
            } else if size <= MAX_CHUNK_SIZE {
                chunks.push(make_chunk(&child, source, file_path, language, None));
            } else {
                let sub_chunks =
                    split_large_node(&child, source, file_path, language, chunkable_types);
                chunks.extend(sub_chunks);
            }
        }
    }

    if chunks.is_empty() {
        line_based_chunks(source, file_path, language)
    } else {
        chunks
    }
}

/// Create a Chunk from an AST node.
fn make_chunk(
    node: &tree_sitter::Node,
    source: &str,
    file_path: &str,
    language: SupportedLanguage,
    parent_context: Option<String>,
) -> Chunk {
    let content = &source[node.byte_range()];
    let symbol = extract_symbol(node, source);
    let kind = node.kind().to_string();
    let start_line = (node.start_position().row + 1) as u32;
    let end_line = (node.end_position().row + 1) as u32;
    let byte_start = node.start_byte() as u32;
    let byte_end = node.end_byte() as u32;

    Chunk {
        id: compute_chunk_id(file_path, byte_start, byte_end),
        file_path: file_path.to_string(),
        start_line,
        end_line,
        byte_start,
        byte_end,
        symbol,
        kind,
        content: content.to_string(),
        parent_context,
        language: language.as_str().to_string(),
        file_mtime: 0, // Will be set by indexer
        content_hash: compute_content_hash(content),
    }
}

/// Extract symbol name from an AST node.
fn extract_symbol(node: &tree_sitter::Node, source: &str) -> Option<String> {
    // For export statements, look inside for the actual declaration
    if node.kind() == "export_statement" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(sym) = extract_symbol(&child, source) {
                return Some(sym);
            }
        }
        return None;
    }

    // Try to find an identifier child
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier"
            || child.kind() == "name"
            || child.kind() == "property_identifier"
            || child.kind() == "type_identifier"
        {
            return Some(source[child.byte_range()].to_string());
        }
    }
    None
}

/// Extract parent context (enclosing scope signature).
fn extract_parent_context(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let parent = node.parent()?;
    let parent_kind = parent.kind();

    // For methods, include the class name
    if parent_kind == "class_declaration"
        || parent_kind == "class_definition"
        || parent_kind == "impl_item"
    {
        let symbol = extract_symbol(&parent, source);
        if let Some(name) = symbol {
            return Some(format!(
                "{} {}",
                parent_kind
                    .replace("_declaration", "")
                    .replace("_definition", "")
                    .replace("_item", ""),
                name
            ));
        }
    }

    None
}

/// Recursively split a large node by its children.
fn split_large_node(
    node: &tree_sitter::Node,
    source: &str,
    file_path: &str,
    language: SupportedLanguage,
    chunkable_types: &[&str],
) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let parent_context = extract_parent_context(node, source);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if chunkable_types.contains(&child.kind()) {
            let child_source = &source[child.byte_range()];
            if child_source.len() <= MAX_CHUNK_SIZE {
                chunks.push(make_chunk(
                    &child,
                    source,
                    file_path,
                    language,
                    parent_context.clone(),
                ));
            } else {
                let sub_chunks =
                    split_large_node(&child, source, file_path, language, chunkable_types);
                chunks.extend(sub_chunks);
            }
        }
    }

    // If no chunkable children, fall back to line-based splitting of this node
    if chunks.is_empty() {
        let node_source = &source[node.byte_range()];
        let mut line_offsets = Vec::new();
        let mut current_offset = 0;
        for line in node_source.lines() {
            line_offsets.push(current_offset);
            let next_bytes = &node_source[current_offset + line.len()..];
            let newline_len = if next_bytes.starts_with("\r\n") {
                2
            } else if next_bytes.starts_with('\n') {
                1
            } else {
                0
            };
            current_offset += line.len() + newline_len;
        }

        let lines: Vec<&str> = node_source.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let end = (i + LINE_WINDOW_SIZE).min(lines.len());
            let chunk_content = lines[i..end].join("\n");
            if chunk_content.len() >= MIN_CHUNK_SIZE {
                let byte_start = (node.start_byte() + line_offsets[i]) as u32;
                let byte_end = (node.start_byte() + (if end < lines.len() { line_offsets[end] } else { node_source.len() })) as u32;
                let content_hash = compute_content_hash(&chunk_content);
                chunks.push(Chunk {
                    id: compute_chunk_id(file_path, byte_start, byte_end),
                    file_path: file_path.to_string(),
                    start_line: (node.start_position().row + 1 + i) as u32,
                    end_line: (node.start_position().row + 1 + end - 1) as u32,
                    byte_start,
                    byte_end,
                    symbol: None,
                    kind: node.kind().to_string(),
                    content: chunk_content,
                    parent_context: parent_context.clone(),
                    language: language.as_str().to_string(),
                    file_mtime: 0,
                    content_hash,
                });
            }
            i += LINE_WINDOW_SIZE - LINE_OVERLAP;
        }
    }

    chunks
}

/// Line-based chunking fallback for unsupported languages.
fn line_based_chunks(source: &str, file_path: &str, language: SupportedLanguage) -> Vec<Chunk> {
    let mut line_offsets = Vec::new();
    let mut current_offset = 0;
    for line in source.lines() {
        line_offsets.push(current_offset);
        let next_bytes = &source[current_offset + line.len()..];
        let newline_len = if next_bytes.starts_with("\r\n") {
            2
        } else if next_bytes.starts_with('\n') {
            1
        } else {
            0
        };
        current_offset += line.len() + newline_len;
    }

    let lines: Vec<&str> = source.lines().collect();
    let mut chunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let end = (i + LINE_WINDOW_SIZE).min(lines.len());
        let chunk_content = lines[i..end].join("\n");

        if !chunk_content.trim().is_empty() {
            let byte_start = line_offsets[i] as u32;
            let byte_end = (if end < lines.len() { line_offsets[end] } else { source.len() }) as u32;
            let content_hash = compute_content_hash(&chunk_content);

            chunks.push(Chunk {
                id: compute_chunk_id(file_path, byte_start, byte_end),
                file_path: file_path.to_string(),
                start_line: (i + 1) as u32,
                end_line: end as u32,
                byte_start,
                byte_end,
                symbol: None,
                kind: "line_block".to_string(),
                content: chunk_content,
                parent_context: None,
                language: language.as_str().to_string(),
                file_mtime: 0,
                content_hash,
            });
        }

        if end >= lines.len() {
            break;
        }
        i += LINE_WINDOW_SIZE - LINE_OVERLAP;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_typescript_function() {
        let source = r#"
export function calculateSum(a: number, b: number): number {
    const result = a + b;
    console.log("Result:", result);
    return result;
}
"#;
        let chunks = chunk_file(source, "test.ts", SupportedLanguage::TypeScript);
        assert!(!chunks.is_empty(), "Should produce at least one chunk");
        // export_statement wraps function_declaration
        assert!(chunks[0].kind == "export_statement" || chunks[0].kind == "function_declaration");
        assert_eq!(chunks[0].language, "typescript");
        assert!(chunks[0].content.contains("calculateSum"));
    }

    #[test]
    fn chunk_typescript_class() {
        let source = r#"
export class Calculator {
    private value: number;

    constructor(initial: number = 0) {
        this.value = initial;
    }

    add(n: number): Calculator {
        this.value += n;
        return this;
    }

    getResult(): number {
        return this.value;
    }
}
"#;
        let chunks = chunk_file(source, "calc.ts", SupportedLanguage::TypeScript);
        assert!(!chunks.is_empty(), "Should produce at least one chunk");
        // export_statement wraps class_declaration
        assert!(chunks[0].kind == "export_statement" || chunks[0].kind == "class_declaration");
        assert!(chunks[0].content.contains("Calculator"));
    }

    #[test]
    fn chunk_python_function() {
        let source = r#"
def calculate_sum(a: int, b: int) -> int:
    """Calculate the sum of two numbers."""
    result = a + b
    print(f"Result: {result}")
    return result
"#;
        let chunks = chunk_file(source, "test.py", SupportedLanguage::Python);
        assert!(!chunks.is_empty(), "Should produce at least one chunk");
        assert_eq!(chunks[0].kind, "function_definition");
        assert_eq!(chunks[0].language, "python");
        assert!(chunks[0].content.contains("calculate_sum"));
    }

    #[test]
    fn chunk_python_class() {
        let source = r#"
class Calculator:
    def __init__(self, initial: int = 0):
        self.value = initial

    def add(self, n: int) -> 'Calculator':
        self.value += n
        return self

    def get_result(self) -> int:
        return self.value
"#;
        let chunks = chunk_file(source, "calc.py", SupportedLanguage::Python);
        assert!(!chunks.is_empty(), "Should produce at least one chunk");
        assert_eq!(chunks[0].kind, "class_definition");
        assert!(chunks[0].content.contains("Calculator"));
    }

    #[test]
    fn chunk_rust_function() {
        let source = r#"
pub fn calculate_sum(a: i32, b: i32) -> i32 {
    let result = a + b;
    println!("Result: {}", result);
    result
}
"#;
        let chunks = chunk_file(source, "test.rs", SupportedLanguage::Rust);
        assert!(!chunks.is_empty(), "Should produce at least one chunk");
        assert_eq!(chunks[0].kind, "function_item");
        assert_eq!(chunks[0].language, "rust");
        assert!(chunks[0].content.contains("calculate_sum"));
    }

    #[test]
    fn chunk_rust_struct() {
        let source = r#"
pub struct Calculator {
    value: i32,
    name: String,
    description: String,
    created_at: u64,
    updated_at: u64,
}

impl Calculator {
    pub fn new(initial: i32) -> Self {
        Self { 
            value: initial,
            name: String::new(),
            description: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    pub fn add(&mut self, n: i32) -> &mut Self {
        self.value += n;
        self
    }

    pub fn get_result(&self) -> i32 {
        self.value
    }
}
"#;
        let chunks = chunk_file(source, "calc.rs", SupportedLanguage::Rust);
        assert!(
            chunks.len() >= 2,
            "Should produce at least 2 chunks (struct + impl), got {}",
            chunks.len()
        );
        let kinds: Vec<&str> = chunks.iter().map(|c| c.kind.as_str()).collect();
        assert!(kinds.contains(&"struct_item"), "Should have struct_item");
        assert!(kinds.contains(&"impl_item"), "Should have impl_item");
    }

    #[test]
    fn line_based_fallback_for_unknown_language() {
        let source = "line 1\nline 2\nline 3\nline 4\nline 5\n";
        let chunks = chunk_file(source, "test.txt", SupportedLanguage::Unknown);
        assert!(
            !chunks.is_empty(),
            "Should produce chunks via line-based fallback"
        );
        assert_eq!(chunks[0].kind, "line_block");
        assert_eq!(chunks[0].language, "unknown");
    }

    #[test]
    fn chunk_id_is_deterministic() {
        let source = r#"
export function test(): void {
    console.log("test");
}
"#;
        let chunks1 = chunk_file(source, "test.ts", SupportedLanguage::TypeScript);
        let chunks2 = chunk_file(source, "test.ts", SupportedLanguage::TypeScript);
        assert_eq!(chunks1.len(), chunks2.len());
        for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
            assert_eq!(c1.id, c2.id, "Chunk IDs should be deterministic");
        }
    }

    #[test]
    fn chunk_content_hash_changes_with_content() {
        let source1 = r#"
export function test(): void {
    console.log("test1");
}
"#;
        let source2 = r#"
export function test(): void {
    console.log("test2");
}
"#;
        let chunks1 = chunk_file(source1, "test.ts", SupportedLanguage::TypeScript);
        let chunks2 = chunk_file(source2, "test.ts", SupportedLanguage::TypeScript);
        assert!(!chunks1.is_empty());
        assert!(!chunks2.is_empty());
        assert_ne!(chunks1[0].content_hash, chunks2[0].content_hash);
    }

    #[test]
    fn chunk_metadata_extraction() {
        let source = r#"
export function calculateSum(a: number, b: number): number {
    return a + b;
}
"#;
        let chunks = chunk_file(source, "test.ts", SupportedLanguage::TypeScript);
        assert!(!chunks.is_empty());
        let chunk = &chunks[0];
        assert_eq!(chunk.file_path, "test.ts");
        assert_eq!(chunk.language, "typescript");
        assert!(chunk.start_line > 0);
        assert!(chunk.end_line >= chunk.start_line);
        assert!(chunk.byte_start < chunk.byte_end);
    }

    #[test]
    fn skip_small_chunks() {
        let source = "function tiny() {}";
        let chunks = chunk_file(source, "test.ts", SupportedLanguage::TypeScript);
        // This function is too small (< 100 bytes), should fall back to line-based
        assert!(
            !chunks.is_empty(),
            "Should fall back to line-based for small code"
        );
    }

    #[test]
    fn multiple_functions_in_file() {
        let source = r#"
export function add(a: number, b: number): number {
    const result = a + b;
    console.log("Adding:", a, b, "Result:", result);
    return result;
}

export function subtract(a: number, b: number): number {
    const result = a - b;
    console.log("Subtracting:", a, b, "Result:", result);
    return result;
}

export function multiply(a: number, b: number): number {
    const result = a * b;
    console.log("Multiplying:", a, b, "Result:", result);
    return result;
}
"#;
        let chunks = chunk_file(source, "math.ts", SupportedLanguage::TypeScript);
        assert!(
            chunks.len() >= 3,
            "Should produce at least 3 chunks for 3 functions, got {}",
            chunks.len()
        );
    }

    // --- Phase 8: Multi-language chunker tests (RED) ---

    #[test]
    fn chunk_csharp_class_and_methods() {
        let source = r#"
using System;

namespace Calculator.App
{
    public class Calculator
    {
        private int _value;

        public Calculator(int initial)
        {
            _value = initial;
        }

        public int Add(int n)
        {
            _value += n;
            return _value;
        }

        public int GetValue()
        {
            return _value;
        }
    }
}
"#;
        let chunks = chunk_file(source, "Calculator.cs", SupportedLanguage::CSharp);
        assert!(!chunks.is_empty(), "Should produce chunks from C# source");
        assert_eq!(chunks[0].language, "csharp");
        assert!(
            chunks.iter().any(|c| c.content.contains("Calculator")),
            "Should contain the Calculator class"
        );
    }

    #[test]
    fn chunk_c_function_and_struct() {
        let source = r#"
#include <stdio.h>

struct Point {
    int x;
    int y;
    double distance_from_origin;
};

int calculate_distance(struct Point* a, struct Point* b) {
    int dx = a->x - b->x;
    int dy = a->y - b->y;
    return dx * dx + dy * dy;
}

void print_point(struct Point* p) {
    printf("Point(%d, %d)\n", p->x, p->y);
}
"#;
        let chunks = chunk_file(source, "geometry.c", SupportedLanguage::C);
        assert!(!chunks.is_empty(), "Should produce chunks from C source");
        assert_eq!(chunks[0].language, "c");
        assert!(
            chunks
                .iter()
                .any(|c| c.kind == "function_definition" || c.kind == "struct_specifier"),
            "Should have function_definition or struct_specifier"
        );
    }

    #[test]
    fn chunk_cpp_class_and_functions() {
        let source = r#"
#include <string>
#include <vector>

namespace data {

class DataProcessor {
public:
    DataProcessor(const std::string& name) : name_(name) {}

    void process(const std::vector<int>& data) {
        for (int item : data) {
            results_.push_back(item * 2);
        }
    }

    const std::vector<int>& results() const { return results_; }

private:
    std::string name_;
    std::vector<int> results_;
};

} // namespace data
"#;
        let chunks = chunk_file(source, "processor.cpp", SupportedLanguage::Cpp);
        assert!(!chunks.is_empty(), "Should produce chunks from C++ source");
        assert_eq!(chunks[0].language, "cpp");
    }

    #[test]
    fn chunk_ruby_class_and_methods() {
        let source = r#"
module Calculator
  class Engine
    attr_reader :value

    def initialize(initial = 0)
      @value = initial
    end

    def add(n)
      @value += n
      self
    end

    def result
      @value
    end
  end

  class AdvancedEngine < Engine
    def multiply(n)
      @value *= n
      self
    end
  end
end
"#;
        let chunks = chunk_file(source, "calculator.rb", SupportedLanguage::Ruby);
        assert!(!chunks.is_empty(), "Should produce chunks from Ruby source");
        assert_eq!(chunks[0].language, "ruby");
        assert!(
            chunks
                .iter()
                .any(|c| c.content.contains("Calculator") || c.content.contains("Engine")),
            "Should contain module or class"
        );
    }

    #[test]
    fn chunk_swift_class_and_functions() {
        let source = r#"
import Foundation

protocol Calculatable {
    func add(_ n: Int) -> Int
    func result() -> Int
}

class Calculator: Calculatable {
    private var value: Int

    init(initial: Int = 0) {
        self.value = initial
    }

    func add(_ n: Int) -> Int {
        value += n
        return value
    }

    func result() -> Int {
        return value
    }
}

enum Operation: String {
    case add = "Addition"
    case subtract = "Subtraction"
    case multiply = "Multiplication"
}
"#;
        let chunks = chunk_file(source, "Calculator.swift", SupportedLanguage::Swift);
        assert!(
            !chunks.is_empty(),
            "Should produce chunks from Swift source"
        );
        assert_eq!(chunks[0].language, "swift");
    }

    #[test]
    fn chunk_kotlin_class_and_functions() {
        let source = r#"
package com.example.calculator

interface Calculatable {
    fun add(n: Int): Int
    fun result(): Int
}

class Calculator(initial: Int = 0) : Calculatable {
    private var value: Int = initial

    override fun add(n: Int): Int {
        value += n
        return value
    }

    override fun result(): Int {
        return value
    }
}

object CalculatorFactory {
    fun create(): Calculator = Calculator(0)
    fun createWithInitial(value: Int): Calculator = Calculator(value)
}
"#;
        let chunks = chunk_file(source, "Calculator.kt", SupportedLanguage::Kotlin);
        assert!(
            !chunks.is_empty(),
            "Should produce chunks from Kotlin source"
        );
        assert_eq!(chunks[0].language, "kotlin");
    }

    #[test]
    fn test_line_based_chunks_offsets_and_unique_ids() {
        let source_lf = "line1\nline2\nline3\nline4\nline5";
        let chunks_lf = line_based_chunks(source_lf, "test_lf.txt", SupportedLanguage::Unknown);
        assert!(!chunks_lf.is_empty());
        assert_eq!(chunks_lf[0].byte_start, 0);
        assert_eq!(chunks_lf[0].byte_end, source_lf.len() as u32);

        // Let's construct a large file to force multiple chunks.
        let mut large_source = String::new();
        for i in 0..100 {
            large_source.push_str(&format!("line_{}\n", i));
        }
        let chunks_large = line_based_chunks(&large_source, "large.txt", SupportedLanguage::Unknown);
        assert!(chunks_large.len() > 1);
        
        // Verify that all chunk IDs are unique
        let mut ids = std::collections::HashSet::new();
        for chunk in &chunks_large {
            assert!(ids.insert(chunk.id.clone()), "Duplicate chunk ID found!");
            // Also check that the byte slice matches the chunk content
            let byte_slice = &large_source[chunk.byte_start as usize..chunk.byte_end as usize];
            assert!(byte_slice.contains(&chunk.content[..chunk.content.len().min(10)]));
        }

        // Test carriage returns
        let source_crlf = "line1\r\nline2\r\nline3\r\nline4\r\nline5";
        let chunks_crlf = line_based_chunks(source_crlf, "test_crlf.txt", SupportedLanguage::Unknown);
        assert!(!chunks_crlf.is_empty());
        assert_eq!(chunks_crlf[0].byte_start, 0);
        assert_eq!(chunks_crlf[0].byte_end, source_crlf.len() as u32);
    }
}
