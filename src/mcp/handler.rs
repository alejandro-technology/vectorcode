//! MCP tool handlers — dispatch logic for vec_search, vec_status, vec_reindex (spec §11.3).
//!
//! Each handler takes `&AppState` and returns MCP-compatible result types.
//! No global state — everything comes through the shared AppState.

use tracing::{error, info};

use crate::engine::indexer::Indexer;
use crate::engine::searcher::{SearchOptions, Searcher};
use crate::store::meta;

use super::schema::*;
use super::AppState;

/// Handle the "initialize" method (spec §11.2).
///
/// Returns server info, protocol version, and capabilities.
pub fn handle_initialize() -> InitializeResult {
    InitializeResult {
        protocol_version: "2024-11-05".to_string(),
        capabilities: ServerCapabilities {
            tools: serde_json::json!({}),
        },
        server_info: ServerInfo {
            name: "vectorcode".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    }
}

/// Handle the "tools/list" method.
///
/// Returns all available tool definitions.
pub fn handle_tools_list() -> ToolsListResult {
    all_tools_list()
}

/// Handle a "tools/call" method by dispatching to the appropriate tool handler.
///
/// Extracts the tool name and arguments from the JSON-RPC params, then
/// delegates to the specific handler function.
pub async fn handle_tool_call(
    name: &str,
    arguments: &serde_json::Value,
    state: &AppState,
) -> ToolCallResult {
    info!("Tool call: {name}");

    match name {
        "vec_search" => handle_vec_search(state, arguments).await,
        "vec_status" => handle_vec_status(state, arguments),
        "vec_reindex" => handle_vec_reindex(state, arguments).await,
        other => {
            error!("Unknown tool: {other}");
            make_error_result(format!("Unknown tool: {other}"))
        }
    }
}

/// Handle the `vec_search` tool call (spec §11.3).
///
/// Performs semantic search over the indexed codebase using the Searcher engine.
async fn handle_vec_search(state: &AppState, arguments: &serde_json::Value) -> ToolCallResult {
    // Parse parameters
    let params: VecSearchParams = match serde_json::from_value(arguments.clone()) {
        Ok(p) => p,
        Err(e) => {
            return make_error_result(format!("Invalid vec_search parameters: {e}"));
        }
    };

    if params.query.is_empty() {
        return make_error_result("Query cannot be empty".to_string());
    }

    // Create searcher and execute search
    let searcher = Searcher::new(
        // We need a new Database handle — SQLite doesn't support shared connections
        // across threads easily. For the MCP server, we open a new connection.
        match open_db_for_state(state) {
            Ok(db) => db,
            Err(e) => return make_error_result(format!("Database error: {e}")),
        },
        state.embedder.clone(),
        state.config.search.clone(),
    );

    let options = SearchOptions {
        limit: params.limit,
        threshold: params.threshold,
        language: params.language,
        path: params.path,
    };

    match searcher.search(&params.query, options).await {
        Ok(results) => {
            let text = format_search_results_text(&params.query, params.threshold, &results);
            make_text_result(text)
        }
        Err(e) => {
            error!("vec_search failed: {e}");
            make_error_result(format!("Search failed: {e}"))
        }
    }
}

/// Handle the `vec_status` tool call (spec §11.3).
///
/// Reads index metadata and returns formatted status text.
fn handle_vec_status(state: &AppState, arguments: &serde_json::Value) -> ToolCallResult {
    // Parse optional project_path (for now, we use the state's project_path)
    let _params: VecStatusParams =
        serde_json::from_value(arguments.clone()).unwrap_or(VecStatusParams { project_path: None });

    match meta::read_index_meta(state.db.conn()) {
        Ok(Some(index_meta)) => {
            let text = format_status_text(&index_meta);
            make_text_result(text)
        }
        Ok(None) => make_error_result(
            "Index metadata not found. Run `vectorcode init` to initialize.".to_string(),
        ),
        Err(e) => make_error_result(format!("Failed to read index metadata: {e}")),
    }
}

/// Handle the `vec_reindex` tool call (spec §11.3).
///
/// Triggers a full or incremental re-index of the project.
async fn handle_vec_reindex(state: &AppState, arguments: &serde_json::Value) -> ToolCallResult {
    let params: VecReindexParams =
        serde_json::from_value(arguments.clone()).unwrap_or(VecReindexParams {
            path: None,
            full: false,
        });

    let db = match open_db_for_state(state) {
        Ok(db) => db,
        Err(e) => return make_error_result(format!("Database error: {e}")),
    };

    // If full reindex requested, reinitialize the schema
    if params.full {
        if let Err(e) = db.init_schema(state.embedder.dimensions()) {
            return make_error_result(format!("Failed to reinitialize schema: {e}"));
        }
    }

    let indexer = Indexer::new(db, state.embedder.clone(), state.config.indexing.clone());

    match indexer.index_project(&state.project_path).await {
        Ok(report) => {
            let text = format!(
                "Re-indexing complete.\n\
                 Files scanned:  {}\n\
                 Files indexed:  {}\n\
                 Chunks total:   {}\n\
                 Chunks new:     {}\n\
                 Chunks skipped: {}\n\
                 Duration:       {:.2}s\n",
                report.files_scanned,
                report.files_indexed,
                report.chunks_total,
                report.chunks_new,
                report.chunks_skipped,
                report.duration.as_secs_f64(),
            );
            make_text_result(text)
        }
        Err(e) => {
            error!("vec_reindex failed: {e}");
            make_error_result(format!("Re-indexing failed: {e}"))
        }
    }
}

/// Open a new database connection for the given state.
///
/// SQLite connections are not easily shared across async tasks, so we open
/// a new connection for each tool call. This is safe because SQLite WAL mode
/// supports concurrent readers.
fn open_db_for_state(state: &AppState) -> anyhow::Result<crate::store::db::Database> {
    let db_path = state.project_path.join(".vectorcode").join("index.db");
    crate::store::db::Database::open(&db_path).map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::Config;
    use crate::embedder::mock::MockEmbedder;
    use crate::store::db::Database;
    use crate::types::IndexMeta;
    use std::sync::Arc;

    fn setup_test_state() -> AppState {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(64).unwrap();

        // Write some meta
        let meta = IndexMeta {
            provider: "mock".to_string(),
            model: "mock-embedder".to_string(),
            dimensions: 64,
            created_at: "2026-06-10T20:00:00Z".to_string(),
            last_sync_at: Some("2026-06-10T20:05:00Z".to_string()),
            files_indexed: 42,
            chunks_stored: 200,
            vectorcode_version: "0.1.0".to_string(),
        };
        crate::store::meta::write_index_meta(db.conn(), &meta).unwrap();

        AppState {
            db,
            embedder: Arc::new(MockEmbedder::new(64)),
            config: Config::default(),
            project_path: std::path::PathBuf::from("/tmp/test-project"),
        }
    }

    // ─── handle_initialize tests ─────────────────────────────────────

    #[test]
    fn handle_initialize_returns_vectorcode_name() {
        let result = handle_initialize();
        assert_eq!(result.server_info.name, "vectorcode");
    }

    #[test]
    fn handle_initialize_returns_version() {
        let result = handle_initialize();
        assert_eq!(result.server_info.version, "0.1.0");
    }

    #[test]
    fn handle_initialize_returns_protocol_version() {
        let result = handle_initialize();
        assert_eq!(result.protocol_version, "2024-11-05");
    }

    #[test]
    fn handle_initialize_has_tools_capability() {
        let result = handle_initialize();
        assert!(result.capabilities.tools.is_object());
    }

    // ─── handle_tools_list tests ─────────────────────────────────────

    #[test]
    fn handle_tools_list_returns_three_tools() {
        let result = handle_tools_list();
        assert_eq!(result.tools.len(), 3);
    }

    #[test]
    fn handle_tools_list_contains_vec_search() {
        let result = handle_tools_list();
        assert!(result.tools.iter().any(|t| t.name == "vec_search"));
    }

    #[test]
    fn handle_tools_list_contains_vec_status() {
        let result = handle_tools_list();
        assert!(result.tools.iter().any(|t| t.name == "vec_status"));
    }

    #[test]
    fn handle_tools_list_contains_vec_reindex() {
        let result = handle_tools_list();
        assert!(result.tools.iter().any(|t| t.name == "vec_reindex"));
    }

    // ─── handle_tool_call dispatch tests ─────────────────────────────

    #[tokio::test]
    async fn handle_tool_call_unknown_tool_returns_error() {
        let state = setup_test_state();
        let result = handle_tool_call("nonexistent_tool", &serde_json::json!({}), &state).await;
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0].text.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn handle_tool_call_vec_status_returns_status() {
        let state = setup_test_state();
        let result = handle_tool_call("vec_status", &serde_json::json!({}), &state).await;
        assert!(result.is_error.is_none());
        assert!(result.content[0].text.contains("VectorCode Index Status"));
        assert!(result.content[0].text.contains("mock"));
    }

    #[tokio::test]
    async fn handle_tool_call_vec_search_empty_query_returns_error() {
        let state = setup_test_state();
        let result =
            handle_tool_call("vec_search", &serde_json::json!({"query": ""}), &state).await;
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0].text.contains("empty"));
    }

    #[tokio::test]
    async fn handle_tool_call_vec_search_invalid_params_returns_error() {
        let state = setup_test_state();
        // Missing required "query" field
        let result = handle_tool_call("vec_search", &serde_json::json!({"limit": 5}), &state).await;
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0].text.contains("Invalid"));
    }

    // ─── handle_vec_status tests ─────────────────────────────────────

    #[test]
    fn handle_vec_status_with_meta_returns_formatted_text() {
        let state = setup_test_state();
        let result = handle_vec_status(&state, &serde_json::json!({}));
        assert!(result.is_error.is_none());
        let text = &result.content[0].text;
        assert!(text.contains("Provider:    mock"));
        assert!(text.contains("Model:       mock-embedder"));
        assert!(text.contains("Dimensions:  64"));
        assert!(text.contains("Files:       42 indexed"));
        assert!(text.contains("Chunks:      200 stored"));
    }

    #[test]
    fn handle_vec_status_without_meta_returns_error() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(64).unwrap();
        // Don't write meta — simulate uninitialized index
        let state = AppState {
            db,
            embedder: Arc::new(MockEmbedder::new(64)),
            config: Config::default(),
            project_path: std::path::PathBuf::from("/tmp"),
        };
        let result = handle_vec_status(&state, &serde_json::json!({}));
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0].text.contains("not found"));
    }

    // ─── format helper integration ───────────────────────────────────

    #[test]
    fn format_search_results_text_shows_content_preview() {
        let results = vec![crate::types::SearchResult {
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 5,
            symbol: Some("my_fn".to_string()),
            kind: "function_item".to_string(),
            language: "rust".to_string(),
            parent_context: None,
            content: "fn my_fn() { println!(\"hello\"); }".to_string(),
            score: 0.95,
        }];
        let text = format_search_results_text("test query", 0.3, &results);
        assert!(text.contains("fn my_fn()"));
        assert!(text.contains("score: 0.95"));
    }

    #[test]
    fn format_search_results_text_truncates_long_content() {
        let long_content = "x".repeat(500);
        let results = vec![crate::types::SearchResult {
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 5,
            symbol: None,
            kind: "function_item".to_string(),
            language: "rust".to_string(),
            parent_context: None,
            content: long_content,
            score: 0.5,
        }];
        let text = format_search_results_text("test", 0.3, &results);
        assert!(
            text.contains("..."),
            "Long content should be truncated with ..."
        );
    }
}
