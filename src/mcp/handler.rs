//! MCP tool handlers — dispatch logic for vec_search, vec_status, vec_reindex (spec §11.3).
//!
//! Each handler takes `&AppState` and returns MCP-compatible result types.
//! No global state — everything comes through the shared AppState.

use tracing::{error, info};

use crate::engine::indexer::Indexer;
use crate::engine::searcher::{SearchOptions, Searcher};
use crate::store::meta;
use crate::watcher::PendingFile;

use super::schema::*;
use super::AppState;

/// Handle the "initialize" method (spec §11.2).
///
/// Returns server info, protocol version, and capabilities.
pub fn handle_initialize() -> InitializeResult {
    InitializeResult {
        protocol_version: "2024-11-05".to_string(),
        capabilities: ServerCapabilities {
            tools: serde_json::json!({"listChanged": true}),
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

/// Handle the "ping" method (MCP spec § utilities/ping).
///
/// Returns an empty result object `{}` to indicate server liveness.
pub fn handle_ping() -> serde_json::Value {
    serde_json::json!({})
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
        "vec_status" => handle_vec_status(state, arguments).await,
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
        state.db.clone(),
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
            // Check for staleness — spec §14.2
            let staleness_banner = match &state.watcher {
                Some(watcher) => {
                    let pending = watcher.read().await.pending_files().await;
                    build_staleness_banner(&results, &pending)
                }
                None => None,
            };

            let text = format_search_results_text(&params.query, params.threshold, &results);
            let final_text = match staleness_banner {
                Some(banner) => format!("{banner}\n{text}"),
                None => text,
            };
            make_text_result(final_text)
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
async fn handle_vec_status(state: &AppState, arguments: &serde_json::Value) -> ToolCallResult {
    // Parse optional project_path with explicit error handling
    let _params: VecStatusParams = match serde_json::from_value(arguments.clone()) {
        Ok(p) => p,
        Err(e) => {
            return make_error_result(format!("Invalid vec_status parameters: {e}"));
        }
    };

    let db = state.db.lock().await;
    match meta::read_index_meta(db.conn()) {
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
    let params: VecReindexParams = match serde_json::from_value(arguments.clone()) {
        Ok(p) => p,
        Err(e) => {
            return make_error_result(format!("Invalid vec_reindex parameters: {e}"));
        }
    };

    // If full reindex requested, reinitialize the schema
    if params.full {
        let db = state.db.lock().await;
        if let Err(e) = db.init_schema(state.embedder.dimensions()) {
            return make_error_result(format!("Failed to reinitialize schema: {e}"));
        }
    }

    let indexer = Indexer::new(
        state.db.clone(),
        state.embedder.clone(),
        state.config.indexing.clone(),
    );

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

/// Build a staleness banner if any search result files have pending changes (spec §14.2).
///
/// Returns `Some(banner)` if there are matching pending files, `None` otherwise.
fn build_staleness_banner(
    results: &[crate::types::SearchResult],
    pending: &[PendingFile],
) -> Option<String> {
    if pending.is_empty() || results.is_empty() {
        return None;
    }

    let now = std::time::SystemTime::now();
    let mut stale_entries: Vec<String> = Vec::new();

    for result in results {
        for pf in pending {
            // Compare by checking if the result file_path is a suffix of the pending path
            let pending_str = pf.path.to_string_lossy();
            if pending_str.ends_with(&result.file_path) || result.file_path == pending_str.as_ref()
            {
                let ago = now
                    .duration_since(pf.modified_at)
                    .unwrap_or_default()
                    .as_secs();
                let time_str = if ago < 60 {
                    format!("{ago}s ago")
                } else {
                    format!("{}m ago", ago / 60)
                };
                stale_entries.push(format!("  - {} (modified {})", result.file_path, time_str));
                break;
            }
        }
    }

    if stale_entries.is_empty() {
        return None;
    }

    let entries = stale_entries.join("\n");
    Some(format!(
        "⚠️ Some files referenced below were modified since the last index sync\n\
         and may not reflect the latest content:\n\
         {entries}\n\
         Use grep or read these files directly for accurate content.\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::Config;
    use crate::embedder::mock::MockEmbedder;
    use crate::store::db::Database;
    use crate::types::IndexMeta;
    use std::path::PathBuf;
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
            db: Arc::new(tokio::sync::Mutex::new(db)),
            embedder: Arc::new(MockEmbedder::new(64)),
            config: Config::default(),
            project_path: std::path::PathBuf::from("/tmp/test-project"),
            watcher: None,
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
        assert_eq!(
            result.capabilities.tools["listChanged"],
            serde_json::json!(true),
            "tools capability should include listChanged: true"
        );
    }

    // ─── handle_tools_list tests ─────────────────────────────────────

    #[test]
    fn handle_tools_list_returns_three_tools() {
        let result = handle_tools_list();
        assert_eq!(result.tools.len(), 3);
    }

    // ─── handle_ping tests ───────────────────────────────────────────

    #[test]
    fn handle_ping_returns_empty_object() {
        let result = handle_ping();
        assert!(result.is_object(), "Ping should return a JSON object");
        assert!(
            result.as_object().unwrap().is_empty(),
            "Ping should return an empty object {{}}"
        );
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

    #[tokio::test]
    async fn handle_vec_status_invalid_params_returns_descriptive_error() {
        let state = setup_test_state();
        // Send a truly invalid structure (string instead of object)
        let result = handle_vec_status(&state, &serde_json::json!("not an object")).await;
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .text
                .contains("Invalid vec_status parameters"),
            "Got: {}",
            result.content[0].text
        );
    }

    #[tokio::test]
    async fn handle_vec_reindex_invalid_params_returns_descriptive_error() {
        let state = setup_test_state();
        let result = handle_vec_reindex(&state, &serde_json::json!("not an object")).await;
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .text
                .contains("Invalid vec_reindex parameters"),
            "Got: {}",
            result.content[0].text
        );
    }

    // ─── handle_vec_status tests ─────────────────────────────────────

    #[tokio::test]
    async fn handle_vec_status_with_meta_returns_formatted_text() {
        let state = setup_test_state();
        let result = handle_vec_status(&state, &serde_json::json!({})).await;
        assert!(result.is_error.is_none());
        let text = &result.content[0].text;
        assert!(text.contains("Provider:    mock"));
        assert!(text.contains("Model:       mock-embedder"));
        assert!(text.contains("Dimensions:  64"));
        assert!(text.contains("Files:       42 indexed"));
        assert!(text.contains("Chunks:      200 stored"));
    }

    #[tokio::test]
    async fn handle_vec_status_without_meta_returns_error() {
        let db = Database::open_in_memory().unwrap();
        db.init_schema(64).unwrap();
        // Don't write meta — simulate uninitialized index
        let state = AppState {
            db: Arc::new(tokio::sync::Mutex::new(db)),
            embedder: Arc::new(MockEmbedder::new(64)),
            config: Config::default(),
            project_path: std::path::PathBuf::from("/tmp"),
            watcher: None,
        };
        let result = handle_vec_status(&state, &serde_json::json!({})).await;
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

    // ─── staleness banner tests ──────────────────────────────────────

    #[test]
    fn build_staleness_banner_none_when_no_pending() {
        let results = vec![crate::types::SearchResult {
            file_path: "src/main.rs".to_string(),
            start_line: 1,
            end_line: 5,
            symbol: None,
            kind: "function_item".to_string(),
            language: "rust".to_string(),
            parent_context: None,
            content: "fn main() {}".to_string(),
            score: 0.9,
        }];
        let banner = build_staleness_banner(&results, &[]);
        assert!(banner.is_none(), "No pending files → no banner");
    }

    #[test]
    fn build_staleness_banner_none_when_no_results() {
        let pending = vec![PendingFile {
            path: PathBuf::from("/project/src/main.rs"),
            modified_at: std::time::SystemTime::now(),
        }];
        let banner = build_staleness_banner(&[], &pending);
        assert!(banner.is_none(), "No results → no banner");
    }

    #[test]
    fn build_staleness_banner_matches_pending_to_results() {
        let results = vec![crate::types::SearchResult {
            file_path: "src/payment/retry.ts".to_string(),
            start_line: 10,
            end_line: 20,
            symbol: Some("retryPayment".to_string()),
            kind: "function_declaration".to_string(),
            language: "typescript".to_string(),
            parent_context: None,
            content: "function retryPayment() {}".to_string(),
            score: 0.85,
        }];
        let pending = vec![PendingFile {
            path: PathBuf::from("/project/src/payment/retry.ts"),
            modified_at: std::time::SystemTime::now(),
        }];
        let banner = build_staleness_banner(&results, &pending);
        assert!(
            banner.is_some(),
            "Should produce banner when result matches pending"
        );
        let banner = banner.unwrap();
        assert!(banner.contains("⚠️"), "Banner should contain warning emoji");
        assert!(
            banner.contains("src/payment/retry.ts"),
            "Banner should mention the stale file"
        );
        assert!(
            banner.contains("modified"),
            "Banner should mention modification time"
        );
    }

    #[test]
    fn build_staleness_banner_none_when_no_overlap() {
        let results = vec![crate::types::SearchResult {
            file_path: "src/auth.ts".to_string(),
            start_line: 1,
            end_line: 5,
            symbol: None,
            kind: "function_declaration".to_string(),
            language: "typescript".to_string(),
            parent_context: None,
            content: "function auth() {}".to_string(),
            score: 0.7,
        }];
        let pending = vec![PendingFile {
            path: PathBuf::from("/project/src/payment/retry.ts"),
            modified_at: std::time::SystemTime::now(),
        }];
        let banner = build_staleness_banner(&results, &pending);
        assert!(
            banner.is_none(),
            "No overlap between results and pending → no banner"
        );
    }
}
