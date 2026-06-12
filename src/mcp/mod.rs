//! MCP server module using rmcp sdk.

pub mod handler;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rmcp::service::ServiceExt;
use rmcp::transport::io::stdio;

use crate::config::schema::Config;
use crate::embedder::Embedder;
use crate::store::db::Database;
use crate::watcher::FileWatcher;

use self::handler::McpHandler;

/// Shared application state accessible by MCP tool handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<tokio::sync::Mutex<Database>>,
    pub embedder: Arc<dyn Embedder>,
    pub config: Config,
    pub project_path: PathBuf,
    /// Optional file watcher for auto-sync and staleness detection
    pub watcher: Option<Arc<tokio::sync::RwLock<FileWatcher>>>,
}

/// MCP server that processes JSON-RPC messages over stdio.
pub struct McpServer {
    state: AppState,
}

impl McpServer {
    /// Create a new MCP server with the given application state.
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// Run the main message loop using rmcp sdk.
    pub async fn run(&mut self) -> Result<()> {
        let handler = McpHandler::new(self.state.clone());
        let service = handler
            .serve(stdio())
            .await
            .map_err(|e| anyhow::anyhow!("Serve error: {}", e))?;
        service
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("Service error: {}", e))?;
        Ok(())
    }
}
