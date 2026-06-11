//! MCP schema types — JSON-RPC 2.0 messages and tool definitions (spec §11).
//!
//! All types use serde for JSON serialization. The MCP protocol communicates
//! over stdio using JSON-RPC 2.0 messages, one per line.

use serde::{Deserialize, Serialize};

// ─── JSON-RPC 2.0 base types ─────────────────────────────────────────

/// JSON-RPC 2.0 request received from the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Must be "2.0".
    pub jsonrpc: String,
    /// Request identifier (string or number).
    /// `None` means notification — no response per JSON-RPC 2.0 §4.1.
    /// `Some(Value::Null)` means explicit `"id": null` which MUST receive a response.
    pub id: Option<serde_json::Value>,
    /// Method name (e.g., "initialize", "tools/list", "tools/call").
    pub method: String,
    /// Optional parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 successful response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Must be "2.0".
    pub jsonrpc: String,
    /// Same id as the request.
    pub id: serde_json::Value,
    /// Result payload.
    pub result: serde_json::Value,
}

/// JSON-RPC 2.0 error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Must be "2.0".
    pub jsonrpc: String,
    /// Same id as the request.
    pub id: serde_json::Value,
    /// Error object.
    pub error: JsonRpcErrorBody,
}

/// Error body within a JSON-RPC error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorBody {
    /// Numeric error code.
    pub code: i32,
    /// Short error message.
    pub message: String,
    /// Optional additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ─── MCP Initialize ──────────────────────────────────────────────────

/// Server information returned in the initialize response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Server capabilities advertised during initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Tools capability — empty object means tools are supported.
    pub tools: serde_json::Value,
}

/// Result of the "initialize" method (spec §11.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

// ─── MCP Tool definitions ────────────────────────────────────────────

/// A tool definition with name, description, and JSON Schema for inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Result of the "tools/list" method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<ToolDefinition>,
}

/// Content item in a tool result (spec §11.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultContent {
    pub r#type: String,
    pub text: String,
}

/// Result of a "tools/call" method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    pub content: Vec<ToolResultContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

// ─── Tool input parameter types ──────────────────────────────────────

/// Parameters for the `vec_search` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct VecSearchParams {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    pub language: Option<String>,
    pub path: Option<String>,
}

/// Parameters for the `vec_status` tool.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VecStatusParams {
    pub project_path: Option<String>,
}

/// Parameters for the `vec_reindex` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct VecReindexParams {
    pub path: Option<String>,
    #[serde(default)]
    pub full: bool,
}

fn default_limit() -> usize {
    10
}

fn default_threshold() -> f32 {
    0.3
}

// ─── Tool definition builders ────────────────────────────────────────

/// Build the `vec_search` tool definition per spec §11.3.
pub fn vec_search_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "vec_search".to_string(),
        description: "Semantic code search — find code by meaning, not just by name. Use when you need to find code related to a concept (e.g., 'payment retry logic', 'user authentication', 'error handling for database connections') and you don't know the exact symbol names or file locations. Returns ranked code chunks with file paths, line numbers, and similarity scores. Complements grep (exact match) and codegraph (structural). Use grep when you know the exact string; use codegraph when you know the symbol name; use vec_search when you know the concept but not the code.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language description of the code you're looking for. Be specific about the domain and behavior (e.g., 'retry logic with exponential backoff' is better than 'retry')."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return.",
                    "default": 10
                },
                "threshold": {
                    "type": "number",
                    "description": "Minimum similarity score (0.0–1.0). Lower values return more results with less relevance.",
                    "default": 0.3
                },
                "language": {
                    "type": "string",
                    "description": "Filter results by programming language (e.g., 'typescript', 'python', 'rust')."
                },
                "path": {
                    "type": "string",
                    "description": "Filter results by file path prefix (e.g., 'src/auth/' to search only in the auth module)."
                }
            },
            "required": ["query"]
        }),
    }
}

/// Build the `vec_status` tool definition per spec §11.3.
pub fn vec_status_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "vec_status".to_string(),
        description: "Check the status of the VectorCode index — provider, model, dimensions, number of indexed files and chunks, last sync time, and any pending file changes.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "projectPath": {
                    "type": "string",
                    "description": "Path to a project with .vectorcode/ initialized. Defaults to current directory."
                }
            }
        }),
    }
}

/// Build the `vec_reindex` tool definition per spec §11.3.
pub fn vec_reindex_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "vec_reindex".to_string(),
        description: "Force re-indexing of the codebase or specific files. Use after changing the embedding provider, or when the index seems stale or corrupted.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Specific file or directory to reindex. If omitted, reindexes the entire project."
                },
                "full": {
                    "type": "boolean",
                    "description": "If true, drops all existing data and rebuilds from scratch. If false, only reindexes changed files.",
                    "default": false
                }
            }
        }),
    }
}

/// Build the full tools list result.
pub fn all_tools_list() -> ToolsListResult {
    ToolsListResult {
        tools: vec![
            vec_search_tool_definition(),
            vec_status_tool_definition(),
            vec_reindex_tool_definition(),
        ],
    }
}

// ─── Response builder helpers ────────────────────────────────────────

/// Build a JSON-RPC 2.0 success response.
pub fn make_response(id: serde_json::Value, result: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result,
    }
}

/// Build a JSON-RPC 2.0 error response.
pub fn make_error(id: serde_json::Value, code: i32, message: String) -> JsonRpcError {
    JsonRpcError {
        jsonrpc: "2.0".to_string(),
        id,
        error: JsonRpcErrorBody {
            code,
            message,
            data: None,
        },
    }
}

/// Build a tool call result with a single text content item.
pub fn make_text_result(text: String) -> ToolCallResult {
    ToolCallResult {
        content: vec![ToolResultContent {
            r#type: "text".to_string(),
            text,
        }],
        is_error: None,
    }
}

/// Build a tool call error result.
pub fn make_error_result(message: String) -> ToolCallResult {
    ToolCallResult {
        content: vec![ToolResultContent {
            r#type: "text".to_string(),
            text: message,
        }],
        is_error: Some(true),
    }
}

/// Format search results as text per spec §11.3 response format.
pub fn format_search_results_text(
    query: &str,
    threshold: f32,
    results: &[crate::types::SearchResult],
) -> String {
    if results.is_empty() {
        return format!("No results found for \"{query}\" (threshold: {threshold:.2})");
    }

    let mut output = format!(
        "Found {} result{} for \"{query}\" (threshold: {threshold:.2})\n",
        results.len(),
        if results.len() == 1 { "" } else { "s" }
    );

    for (i, r) in results.iter().enumerate() {
        output.push_str(&format!(
            "\n[{}] {}:{}-{} (score: {:.2})\n",
            i + 1,
            r.file_path,
            r.start_line,
            r.end_line,
            r.score
        ));
        if let Some(sym) = &r.symbol {
            output.push_str(&format!("    Symbol: {sym}\n"));
        }
        output.push_str(&format!("    Kind: {}\n", r.kind));
        output.push('\n');
        // Show first few lines of content
        let preview: String = r.content.chars().take(300).collect();
        output.push_str(&format!("    {preview}\n"));
        if r.content.len() > 300 {
            output.push_str("    ...\n");
        }
    }

    output
}

/// Format index status as text per spec §11.3 response format.
pub fn format_status_text(meta: &crate::types::IndexMeta) -> String {
    let last_sync = meta.last_sync_at.as_deref().unwrap_or("never");

    format!(
        "VectorCode Index Status\n\
         ═══════════════════════\n\
         Provider:    {}\n\
         Model:       {}\n\
         Dimensions:  {}\n\
         Version:     {}\n\
         \n\
         Files:       {} indexed\n\
         Chunks:      {} stored\n\
         Last sync:   {}\n",
        meta.provider,
        meta.model,
        meta.dimensions,
        meta.vectorcode_version,
        meta.files_indexed,
        meta.chunks_stored,
        last_sync,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── JSON-RPC serialization ──────────────────────────────────────

    #[test]
    fn json_rpc_request_deserialize_initialize() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(serde_json::json!(1)));
        assert_eq!(req.method, "initialize");
    }

    #[test]
    fn json_rpc_request_deserialize_tools_list() {
        let json = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(serde_json::json!(2)));
    }

    #[test]
    fn json_rpc_request_deserialize_tools_call_with_params() {
        let json = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"vec_search","arguments":{"query":"auth","limit":5}}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.params["name"], "vec_search");
        assert_eq!(req.params["arguments"]["query"], "auth");
        assert_eq!(req.params["arguments"]["limit"], 5);
    }

    #[test]
    fn json_rpc_request_deserialize_string_id() {
        let json = r#"{"jsonrpc":"2.0","id":"abc-123","method":"initialize","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(serde_json::json!("abc-123")));
    }

    #[test]
    fn json_rpc_response_serialize() {
        let resp = make_response(serde_json::json!(1), serde_json::json!({"ok": true}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""jsonrpc":"2.0"#));
        assert!(json.contains(r#""id":1"#));
        assert!(json.contains(r#""result":{"ok":true}"#));
    }

    #[test]
    fn json_rpc_error_serialize() {
        let err = make_error(serde_json::json!(1), -32600, "Invalid request".to_string());
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains(r#""code":-32600"#));
        assert!(json.contains(r#""message":"Invalid request"#));
    }

    #[test]
    fn json_rpc_error_without_data_omits_field() {
        let err = make_error(serde_json::json!(1), -32600, "test".to_string());
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains(r#""data""#), "data field should be omitted");
    }

    // ─── Initialize result ───────────────────────────────────────────

    #[test]
    fn initialize_result_serializes_correctly() {
        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ServerCapabilities {
                tools: serde_json::json!({}),
            },
            server_info: ServerInfo {
                name: "vectorcode".to_string(),
                version: "0.1.0".to_string(),
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["serverInfo"]["name"], "vectorcode");
        assert_eq!(json["serverInfo"]["version"], "0.1.0");
        assert_eq!(json["protocolVersion"], "2024-11-05");
        assert!(json["capabilities"]["tools"].is_object());
    }

    // ─── Tool definitions ────────────────────────────────────────────

    #[test]
    fn vec_search_tool_has_required_fields() {
        let tool = vec_search_tool_definition();
        assert_eq!(tool.name, "vec_search");
        assert!(tool.description.contains("Semantic code search"));
        assert!(tool.input_schema["properties"]["query"].is_object());
        assert!(tool.input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("query")));
    }

    #[test]
    fn vec_status_tool_has_project_path_param() {
        let tool = vec_status_tool_definition();
        assert_eq!(tool.name, "vec_status");
        assert!(tool.input_schema["properties"]["projectPath"].is_object());
    }

    #[test]
    fn vec_reindex_tool_has_path_and_full_params() {
        let tool = vec_reindex_tool_definition();
        assert_eq!(tool.name, "vec_reindex");
        assert!(tool.input_schema["properties"]["path"].is_object());
        assert!(tool.input_schema["properties"]["full"].is_object());
    }

    #[test]
    fn all_tools_list_has_three_tools() {
        let list = all_tools_list();
        assert_eq!(list.tools.len(), 3);
        let names: Vec<&str> = list.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"vec_search"));
        assert!(names.contains(&"vec_status"));
        assert!(names.contains(&"vec_reindex"));
    }

    // ─── Tool parameter deserialization ──────────────────────────────

    #[test]
    fn vec_search_params_deserialize_minimal() {
        let json = r#"{"query":"auth logic"}"#;
        let params: VecSearchParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.query, "auth logic");
        assert_eq!(params.limit, 10);
        assert!((params.threshold - 0.3).abs() < f32::EPSILON);
        assert!(params.language.is_none());
        assert!(params.path.is_none());
    }

    #[test]
    fn vec_search_params_deserialize_full() {
        let json = r#"{"query":"retry","limit":5,"threshold":0.5,"language":"rust","path":"src/"}"#;
        let params: VecSearchParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.query, "retry");
        assert_eq!(params.limit, 5);
        assert!((params.threshold - 0.5).abs() < f32::EPSILON);
        assert_eq!(params.language.as_deref(), Some("rust"));
        assert_eq!(params.path.as_deref(), Some("src/"));
    }

    #[test]
    fn vec_status_params_deserialize_empty() {
        let json = r#"{}"#;
        let params: VecStatusParams = serde_json::from_str(json).unwrap();
        assert!(params.project_path.is_none());
    }

    #[test]
    fn vec_status_params_deserialize_with_path() {
        let json = r#"{"projectPath":"/tmp/myproject"}"#;
        let params: VecStatusParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.project_path.as_deref(), Some("/tmp/myproject"));
    }

    #[test]
    fn vec_reindex_params_deserialize_defaults() {
        let json = r#"{}"#;
        let params: VecReindexParams = serde_json::from_str(json).unwrap();
        assert!(params.path.is_none());
        assert!(!params.full);
    }

    #[test]
    fn vec_reindex_params_deserialize_full() {
        let json = r#"{"path":"src/auth/","full":true}"#;
        let params: VecReindexParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.path.as_deref(), Some("src/auth/"));
        assert!(params.full);
    }

    // ─── Result formatting ───────────────────────────────────────────

    #[test]
    fn make_text_result_creates_single_text_content() {
        let result = make_text_result("hello world".to_string());
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].r#type, "text");
        assert_eq!(result.content[0].text, "hello world");
        assert!(result.is_error.is_none());
    }

    #[test]
    fn make_error_result_sets_is_error_true() {
        let result = make_error_result("something broke".to_string());
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].text, "something broke");
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn format_search_results_text_empty() {
        let text = format_search_results_text("auth", 0.3, &[]);
        assert!(text.contains("No results found"));
        assert!(text.contains("auth"));
    }

    #[test]
    fn format_search_results_text_with_results() {
        let results = vec![crate::types::SearchResult {
            file_path: "src/auth.ts".to_string(),
            start_line: 10,
            end_line: 20,
            symbol: Some("authenticate".to_string()),
            kind: "function_declaration".to_string(),
            language: "typescript".to_string(),
            parent_context: None,
            content: "function authenticate() { /* ... */ }".to_string(),
            score: 0.87,
        }];
        let text = format_search_results_text("auth logic", 0.3, &results);
        assert!(text.contains("Found 1 result"));
        assert!(text.contains("src/auth.ts:10-20"));
        assert!(text.contains("score: 0.87"));
        assert!(text.contains("Symbol: authenticate"));
        assert!(text.contains("Kind: function_declaration"));
    }

    #[test]
    fn format_search_results_text_multiple_results_plural() {
        let results = vec![
            crate::types::SearchResult {
                file_path: "a.ts".to_string(),
                start_line: 1,
                end_line: 5,
                symbol: None,
                kind: "function_declaration".to_string(),
                language: "typescript".to_string(),
                parent_context: None,
                content: "fn a()".to_string(),
                score: 0.9,
            },
            crate::types::SearchResult {
                file_path: "b.ts".to_string(),
                start_line: 1,
                end_line: 5,
                symbol: None,
                kind: "function_declaration".to_string(),
                language: "typescript".to_string(),
                parent_context: None,
                content: "fn b()".to_string(),
                score: 0.7,
            },
        ];
        let text = format_search_results_text("test", 0.3, &results);
        assert!(text.contains("Found 2 results"));
    }

    #[test]
    fn format_status_text_contains_all_fields() {
        let meta = crate::types::IndexMeta {
            provider: "gemini".to_string(),
            model: "gemini-embedding-001".to_string(),
            dimensions: 768,
            created_at: "2026-06-10T20:00:00Z".to_string(),
            last_sync_at: Some("2026-06-10T20:05:00Z".to_string()),
            files_indexed: 2515,
            chunks_stored: 8432,
            vectorcode_version: "0.1.0".to_string(),
        };
        let text = format_status_text(&meta);
        assert!(text.contains("Provider:    gemini"));
        assert!(text.contains("Model:       gemini-embedding-001"));
        assert!(text.contains("Dimensions:  768"));
        assert!(text.contains("Version:     0.1.0"));
        assert!(text.contains("Files:       2515 indexed"));
        assert!(text.contains("Chunks:      8432 stored"));
        assert!(text.contains("Last sync:   2026-06-10T20:05:00Z"));
    }

    #[test]
    fn format_status_text_no_last_sync_shows_never() {
        let meta = crate::types::IndexMeta {
            provider: "onnx".to_string(),
            model: "all-MiniLM-L6-v2".to_string(),
            dimensions: 384,
            created_at: "2026-06-10T20:00:00Z".to_string(),
            last_sync_at: None,
            files_indexed: 0,
            chunks_stored: 0,
            vectorcode_version: "0.1.0".to_string(),
        };
        let text = format_status_text(&meta);
        assert!(text.contains("Last sync:   never"));
    }

    // ─── Tool call result serialization ──────────────────────────────

    #[test]
    fn tool_call_result_serializes_content_array() {
        let result = make_text_result("hello".to_string());
        let json = serde_json::to_value(&result).unwrap();
        assert!(json["content"].is_array());
        assert_eq!(json["content"][0]["type"], "text");
        assert_eq!(json["content"][0]["text"], "hello");
    }

    #[test]
    fn tool_call_error_result_serializes_is_error() {
        let result = make_error_result("fail".to_string());
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["isError"], true);
    }

    #[test]
    fn tool_call_success_result_omits_is_error() {
        let result = make_text_result("ok".to_string());
        let json = serde_json::to_value(&result).unwrap();
        assert!(
            json.get("isError").is_none(),
            "isError should be omitted for success"
        );
    }
}
