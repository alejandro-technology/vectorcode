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

/// State that is initialized once the project path is discovered.
#[derive(Clone)]
pub struct AppInnerState {
    pub db: Arc<tokio::sync::Mutex<Database>>,
    pub embedder: Arc<dyn Embedder>,
    pub config: Config,
    pub project_path: PathBuf,
    /// Optional file watcher for auto-sync and staleness detection
    pub watcher: Option<Arc<tokio::sync::RwLock<FileWatcher>>>,
}

/// Shared application state accessible by MCP tool handlers.
/// Starts uninitialized, and gets initialized once the MCP client provides
/// the workspace roots.
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<tokio::sync::RwLock<Option<AppInnerState>>>,
    // The dynamically discovered root, which we can fallback to for lazy init
    pub known_root: Arc<tokio::sync::RwLock<Option<PathBuf>>>,
    // CLI flags we need to hold onto for initialization
    pub watch: bool,
    pub debounce: u64,
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
