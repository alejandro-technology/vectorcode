use std::path::PathBuf;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::schemars::JsonSchema;
use rmcp::ServerHandler;
use rmcp::{tool, tool_handler, tool_router};
use serde::Deserialize;
use tracing::error;

use crate::engine::indexer::Indexer;
use crate::engine::searcher::{SearchOptions, Searcher};
use crate::mcp::{AppInnerState, AppState};
use crate::store::meta;
use crate::watcher::PendingFile;

#[derive(Clone)]
pub struct McpHandler {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

impl McpHandler {
    async fn get_inner_state(&self) -> Result<AppInnerState, String> {
        {
            let inner = self.state.inner.read().await;
            if let Some(state) = &*inner {
                return Ok(state.clone());
            }
        }

        let known_root = self.state.known_root.read().await.clone();
        if let Some(root) = known_root {
            if root.join(".vectorcode").exists() {
                match crate::cli::serve::try_init_workspace(&root, self.state.watch, self.state.debounce).await {
                    Ok(inner_state) => {
                        *self.state.inner.write().await = Some(inner_state.clone());
                        return Ok(inner_state);
                    }
                    Err(e) => return Err(format!("Failed to lazily initialize workspace: {e}")),
                }
            }
        }

        Err("VectorCode is not initialized in this workspace. Run `vectorcode init` in your project directory to set it up.".to_string())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VecSearchParams {
    /// Semantic search query
    pub query: String,
    /// Maximum number of results to return (default: 10, max: 100)
    pub limit: Option<usize>,
    /// Minimum similarity score threshold (0.0 to 1.0, default: 0.0)
    pub threshold: Option<f32>,
    /// Filter results by programming language (e.g., 'rust', 'typescript')
    pub language: Option<String>,
    /// Filter results by file path prefix
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VecStatusParams {
    /// Reserved for future use
    pub reserved: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VecReindexParams {
    /// Set to true to drop the index and start fresh
    pub full: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VecReadLinesParams {
    /// The file path to read
    pub file_path: String,
    /// The starting line number (1-indexed, inclusive)
    pub start_line: usize,
    /// The ending line number (1-indexed, inclusive)
    pub end_line: usize,
}

#[tool_router]
impl McpHandler {
    #[tool(
        name = "vec_search",
        description = "Perform a semantic search over the codebase. \
                       Use this to find code conceptually related to the query, \
                       even if exact keywords don't match. Results are ordered by relevance.",
        annotations(read_only_hint = true)
    )]
    async fn vec_search(&self, params: Parameters<VecSearchParams>) -> Result<String, String> {
        let p = params.0;
        if p.query.is_empty() {
            return Err("Query cannot be empty".to_string());
        }

        let inner_state = self.get_inner_state().await?;

        let searcher = Searcher::new(
            inner_state.db.clone(),
            inner_state.embedder.clone(),
            inner_state.config.search.clone(),
        );

        let options = SearchOptions {
            limit: p.limit.unwrap_or(10).min(100),
            threshold: p.threshold.unwrap_or(0.0),
            language: p.language,
            path: p.path,
        };

        match searcher.search(&p.query, options).await {
            Ok(results) => {
                let staleness_banner = match &inner_state.watcher {
                    Some(w) => {
                        let pending = w.read().await.pending_files().await;
                        build_staleness_banner(&results, &pending)
                    }
                    None => None,
                };

                let text = format_search_results_text(&p.query, p.threshold, &results);
                match staleness_banner {
                    Some(banner) => Ok(format!("{banner}\n{text}")),
                    None => Ok(text),
                }
            }
            Err(e) => {
                error!("vec_search failed: {e}");
                Err(format!("Search failed: {e}"))
            }
        }
    }

    #[tool(
        name = "vec_status",
        description = "Get the current status of the vector index, including provider, \
                       dimensions, number of files indexed, and last sync time.",
        annotations(read_only_hint = true)
    )]
    async fn vec_status(&self, _params: Parameters<VecStatusParams>) -> Result<String, String> {
        let inner_state = self.get_inner_state().await?;
        let db = inner_state.db.lock().await;
        match meta::read_index_meta(db.conn()) {
            Ok(Some(index_meta)) => Ok(format_status_text(&index_meta)),
            Ok(None) => {
                Err("Index metadata not found. Run `vectorcode init` to initialize.".to_string())
            }
            Err(e) => Err(format!("Failed to read index metadata: {e}")),
        }
    }

    #[tool(
        name = "vec_reindex",
        description = "Trigger a background re-index of the project. \
                       Use full=true to drop the existing index and start fresh.",
        annotations(destructive_hint = true)
    )]
    async fn vec_reindex(&self, params: Parameters<VecReindexParams>) -> Result<String, String> {
        let p = params.0;
        let inner_state = self.get_inner_state().await?;

        if p.full {
            let db = inner_state.db.lock().await;
            if let Err(e) = db.init_schema(inner_state.embedder.dimensions()) {
                return Err(format!("Failed to reinitialize schema: {e}"));
            }
        }

        let indexer = Indexer::new(
            inner_state.db.clone(),
            inner_state.embedder.clone(),
            inner_state.config.indexing.clone(),
        );

        match indexer.index_project(&inner_state.project_path).await {
            Ok(report) => Ok(format!(
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
            )),
            Err(e) => {
                error!("vec_reindex failed: {e}");
                Err(format!("Re-indexing failed: {e}"))
            }
        }
    }

    #[tool(
        name = "vec_read_lines",
        description = "Read a specific range of lines from a file. \
                       Use this instead of generic file reading when you only need \
                       to expand the context around a snippet found via vec_search.",
        annotations(read_only_hint = true)
    )]
    async fn vec_read_lines(
        &self,
        params: Parameters<VecReadLinesParams>,
    ) -> Result<String, String> {
        let p = params.0;
        let inner_state = self.get_inner_state().await?;
        let requested_path = inner_state.project_path.join(&p.file_path);

        // Canonicalize to resolve any ../ and follow symlinks
        let canonical =
            match tokio::task::spawn_blocking(move || std::fs::canonicalize(&requested_path)).await
            {
                Ok(Ok(c)) => c,
                _ => return Err(format!("File not found or invalid: {}", p.file_path)),
            };

        let canonical_project = std::fs::canonicalize(&inner_state.project_path)
            .unwrap_or_else(|_| inner_state.project_path.clone());

        if !canonical.starts_with(&canonical_project) {
            return Err("Access denied: Path is outside of project bounds.".to_string());
        }

        if p.start_line == 0 {
            return Err("Invalid start_line".to_string());
        }
        if p.end_line < p.start_line {
            return Err("start_line must be <= end_line".to_string());
        }
        if p.end_line - p.start_line + 1 > 500 {
            return Err("Requested line range exceeds the maximum limit of 500 lines.".to_string());
        }

        let metadata = tokio::fs::metadata(&canonical)
            .await
            .map_err(|e| format!("Failed to get file metadata: {e}"))?;
        if metadata.len() > 2 * 1024 * 1024 {
            return Err("Access denied: File size exceeds the maximum limit of 2MB.".to_string());
        }

        let content = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|e| format!("Failed to read file: {e}"))?;

        let lines: Vec<&str> = content.lines().collect();
        if p.start_line > lines.len() {
            return Err("Invalid start_line (exceeds file length)".to_string());
        }

        let start_idx = p.start_line - 1;
        let end_idx = std::cmp::min(p.end_line, lines.len());

        let extracted = lines[start_idx..end_idx].join("\n");
        Ok(format!(
            "Lines {}-{} of {}:\n{}",
            p.start_line, end_idx, p.file_path, extracted
        ))
    }

    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

use rmcp::model::{Implementation, InitializeResult, ProtocolVersion, ServerCapabilities};
use rmcp::service::NotificationContext;
use rmcp::service::RoleServer;
use std::future::Future;

#[tool_handler]
impl ServerHandler for McpHandler {
    fn get_info(&self) -> InitializeResult {
        InitializeResult {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "vectorcode".to_string(),
                title: None,
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
            instructions: Some("VectorCode provides semantic search. \
                                IMPORTANT: Do not use generic file reading tools to read entire files \
                                discovered via vec_search. Rely on the snippets returned. \
                                If you need more surrounding context, use the `vec_read_lines` tool \
                                to fetch only the specific lines you need.".to_string()),
        }
    }

    fn on_initialized(
        &self,
        context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        let state = self.state.clone();
        async move {
            tracing::info!("client initialized, fetching roots...");
            
            // We must spawn a background task to fetch roots. Otherwise, waiting for the
            // client's response blocks the MCP message loop and causes a deadlock.
            tokio::spawn(async move {
                // Ask the client for its active workspace roots
                if let Ok(roots_result) = context.peer.list_roots().await {
                    // Find the first root that has a .vectorcode directory
                    let mut chosen_root = None;
                    for root in &roots_result.roots {
                        if let Ok(url) = url::Url::parse(&root.uri) {
                            if url.scheme() == "file" {
                                if let Ok(path) = url.to_file_path() {
                                    if path.join(".vectorcode").exists() {
                                        chosen_root = Some(path);
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    // If none have .vectorcode, fallback to the first root
                    if chosen_root.is_none() {
                        if let Some(first_root) = roots_result.roots.first() {
                            if let Ok(url) = url::Url::parse(&first_root.uri) {
                                if url.scheme() == "file" {
                                    if let Ok(path) = url.to_file_path() {
                                        chosen_root = Some(path);
                                    }
                                }
                            }
                        }
                    }

                    if let Some(project_path) = chosen_root {
                        *state.known_root.write().await = Some(project_path.clone());
                        if project_path.join(".vectorcode").exists() {
                            tracing::info!("Found initialized vectorcode workspace at {}", project_path.display());
                            match crate::cli::serve::try_init_workspace(&project_path, state.watch, state.debounce).await {
                                Ok(inner) => {
                                    *state.inner.write().await = Some(inner);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to initialize workspace from root {}: {}", project_path.display(), e);
                                }
                            }
                        } else {
                            tracing::info!("Found root {} but it is not initialized. Awaiting user to run `vectorcode init`.", project_path.display());
                        }
                    } else {
                        tracing::info!("No roots provided by client.");
                    }
                } else {
                    tracing::warn!("Failed to fetch roots from client.");
                }
            });
        }
    }
}

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

fn format_search_results_text(
    query: &str,
    threshold: Option<f32>,
    results: &[crate::types::SearchResult],
) -> String {
    if results.is_empty() {
        return format!("No results found for query: '{query}'");
    }

    let threshold_str = if let Some(t) = threshold {
        format!(" (threshold: {t})")
    } else {
        "".to_string()
    };

    let mut out = format!(
        "Found {} results for '{}'{threshold_str}:\n\n",
        results.len(),
        query
    );

    for (i, res) in results.iter().enumerate() {
        let symbol_info = res
            .symbol
            .as_ref()
            .map(|s| format!(" `{s}`"))
            .unwrap_or_default();

        let ctx_info = res
            .parent_context
            .as_ref()
            .map(|c| format!(" (in {c})"))
            .unwrap_or_default();

        out.push_str(&format!(
            "{}. {}:L{}-{}{} [{}] (score: {:.2})\n",
            i + 1,
            res.file_path,
            res.start_line,
            res.end_line,
            symbol_info,
            res.language,
            res.score
        ));

        if !ctx_info.is_empty() {
            out.push_str(&format!("   Context:{ctx_info}\n"));
        }

        let lines: Vec<&str> = res.content.lines().collect();
        // Removed artificial truncation so LLMs see the entire AST chunk
        // without feeling forced to use read_file for the remaining lines.

        out.push_str("   ---\n");
        for line in lines {
            out.push_str(&format!("   | {}\n", line));
        }
        out.push_str("   ---\n\n");
    }

    out
}

fn format_status_text(meta: &crate::types::IndexMeta) -> String {
    format!(
        "VectorCode Index Status\n\
         =======================\n\
         Provider:    {}\n\
         Model:       {}\n\
         Dimensions:  {}\n\
         Files:       {} indexed\n\
         Chunks:      {} stored\n\
         Created:     {}\n\
         Last Sync:   {}\n\
         Version:     {}",
        meta.provider,
        meta.model,
        meta.dimensions,
        meta.files_indexed,
        meta.chunks_stored,
        meta.created_at,
        meta.last_sync_at.as_deref().unwrap_or("Never"),
        meta.vectorcode_version
    )
}
