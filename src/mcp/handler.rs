use rmcp::ServerHandler;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool_router, tool_handler, tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use tracing::error;

use crate::mcp::AppState;
use crate::engine::searcher::{SearchOptions, Searcher};
use crate::engine::indexer::Indexer;
use crate::store::meta;
use crate::watcher::PendingFile;

#[derive(Clone)]
pub struct McpHandler {
    state: AppState,
    tool_router: ToolRouter<Self>,
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

#[tool_router]
impl McpHandler {
    #[tool(
        name = "vec_search",
        description = "Perform a semantic search over the codebase. \
                       Use this to find code conceptually related to the query, \
                       even if exact keywords don't match. Results are ordered by relevance.",
        annotations(read_only_hint = true)
    )]
    async fn vec_search(
        &self,
        params: Parameters<VecSearchParams>
    ) -> Result<String, String> {
        let p = params.0;
        if p.query.is_empty() {
            return Err("Query cannot be empty".to_string());
        }

        let searcher = Searcher::new(
            self.state.db.clone(),
            self.state.embedder.clone(),
            self.state.config.search.clone(),
        );

        let options = SearchOptions {
            limit: p.limit.unwrap_or(10),
            threshold: p.threshold.unwrap_or(0.0),
            language: p.language,
            path: p.path,
        };

        match searcher.search(&p.query, options).await {
            Ok(results) => {
                let staleness_banner = match &self.state.watcher {
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
    async fn vec_status(
        &self,
        _params: Parameters<VecStatusParams>
    ) -> Result<String, String> {
        let db = self.state.db.lock().await;
        match meta::read_index_meta(db.conn()) {
            Ok(Some(index_meta)) => {
                Ok(format_status_text(&index_meta))
            }
            Ok(None) => Err("Index metadata not found. Run `vectorcode init` to initialize.".to_string()),
            Err(e) => Err(format!("Failed to read index metadata: {e}")),
        }
    }

    #[tool(
        name = "vec_reindex",
        description = "Trigger a background re-index of the project. \
                       Use full=true to drop the existing index and start fresh.",
        annotations(destructive_hint = true)
    )]
    async fn vec_reindex(
        &self,
        params: Parameters<VecReindexParams>
    ) -> Result<String, String> {
        let p = params.0;
        
        if p.full {
            let db = self.state.db.lock().await;
            if let Err(e) = db.init_schema(self.state.embedder.dimensions()) {
                return Err(format!("Failed to reinitialize schema: {e}"));
            }
        }

        let indexer = Indexer::new(
            self.state.db.clone(),
            self.state.embedder.clone(),
            self.state.config.indexing.clone(),
        );

        match indexer.index_project(&self.state.project_path).await {
            Ok(report) => {
                Ok(format!(
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
                ))
            }
            Err(e) => {
                error!("vec_reindex failed: {e}");
                Err(format!("Re-indexing failed: {e}"))
            }
        }
    }

    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

use rmcp::model::{InitializeResult, ServerCapabilities, Implementation, ProtocolVersion};

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
            instructions: None,
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

        let mut lines: Vec<&str> = res.content.lines().collect();
        if lines.len() > 15 {
            lines.truncate(15);
            lines.push("...");
        }

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
