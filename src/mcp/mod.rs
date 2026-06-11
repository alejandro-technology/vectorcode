//! MCP server module — JSON-RPC 2.0 over stdio (spec §11).
//!
//! Implements the Model Context Protocol server that communicates
//! with AI agents via stdin/stdout using JSON-RPC 2.0 messages.

pub mod handler;
pub mod schema;
pub mod transport;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::config::schema::Config;
use crate::embedder::Embedder;
use crate::store::db::Database;
use crate::watcher::FileWatcher;

use self::schema::*;
use self::transport::McpTransport;

/// Shared application state accessible by MCP tool handlers.
pub struct AppState {
    pub db: Database,
    pub embedder: Arc<dyn Embedder>,
    pub config: Config,
    pub project_path: PathBuf,
    /// Optional file watcher for auto-sync and staleness detection (spec §14).
    pub watcher: Option<Arc<tokio::sync::RwLock<FileWatcher>>>,
}

/// MCP server that processes JSON-RPC messages over stdio.
pub struct McpServer {
    transport: McpTransport,
    state: Arc<Mutex<AppState>>,
}

impl McpServer {
    /// Create a new MCP server with the given application state.
    pub fn new(state: AppState) -> Self {
        Self {
            transport: McpTransport::new(),
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Run the main message loop — reads from stdin, dispatches, writes to stdout.
    ///
    /// Continues until stdin closes (EOF) or an unrecoverable error occurs.
    pub async fn run(&mut self) -> Result<()> {
        info!("MCP server starting on stdio");

        loop {
            // Read a message from stdin
            let line = match self.transport.read_line().await? {
                Some(line) => line,
                None => {
                    info!("stdin closed (EOF), shutting down MCP server");
                    break;
                }
            };

            // Parse as JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(e) => {
                    warn!("Failed to parse JSON-RPC request: {e}");
                    let error_resp =
                        make_error(serde_json::Value::Null, -32700, format!("Parse error: {e}"));
                    self.transport.write_message(&error_resp).await?;
                    continue;
                }
            };

            debug!("Received method: {}", request.method);

            // Dispatch to handler (returns None for notifications)
            if let Some(response) = self.dispatch(request).await {
                // Write response to stdout
                self.transport.write_message(&response).await?;
            }
        }

        info!("MCP server shut down cleanly");
        Ok(())
    }

    /// Dispatch a JSON-RPC request to the appropriate handler.
    ///
    /// Returns `None` for notifications (no `id` field per JSON-RPC 2.0 §4.1).
    /// Returns `Some(response)` for requests that expect a response.
    async fn dispatch(&self, request: JsonRpcRequest) -> Option<serde_json::Value> {
        // JSON-RPC 2.0 §4.1: notifications have no "id" and MUST NOT receive a response
        if request.id.is_null() {
            debug!("Received notification: {}", request.method);
            return None;
        }

        let id = request.id.clone();

        let response = match request.method.as_str() {
            "initialize" => {
                let result = handler::handle_initialize();
                serde_json::to_value(make_response(id, serde_json::to_value(result).unwrap()))
                    .unwrap()
            }
            "tools/list" => {
                let result = handler::handle_tools_list();
                serde_json::to_value(make_response(id, serde_json::to_value(result).unwrap()))
                    .unwrap()
            }
            "tools/call" => {
                // Extract tool name and arguments from params
                let tool_name = request.params["name"].as_str().unwrap_or("");
                let arguments = &request.params["arguments"];

                let state = self.state.lock().await;
                let result = handler::handle_tool_call(tool_name, arguments, &state).await;

                serde_json::to_value(make_response(id, serde_json::to_value(result).unwrap()))
                    .unwrap()
            }
            "notifications/initialized" => {
                // JSON-RPC notification — clients send this after initialize handshake.
                // Per JSON-RPC 2.0 §4.1 we do NOT send a response.
                // The early null-id check above already handles this for well-behaved clients.
                debug!("Received notifications/initialized (unexpected id present)");
                serde_json::to_value(make_response(id, serde_json::json!({}))).unwrap()
            }
            other => {
                warn!("Unknown method: {other}");
                serde_json::to_value(make_error(id, -32601, format!("Method not found: {other}")))
                    .unwrap()
            }
        };

        Some(response)
    }
}
