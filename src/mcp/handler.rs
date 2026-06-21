use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::schemars::JsonSchema;
use rmcp::ServerHandler;
use rmcp::{tool, tool_handler, tool_router};
use serde::Deserialize;
use tracing::error;

use crate::engine::indexer::Indexer;
use crate::engine::languages::SupportedLanguage;
use crate::engine::outliner;
use crate::engine::searcher::{SearchMode, SearchOptions};
use crate::mcp::{AppInnerState, AppState};
use crate::store::meta;
use crate::watcher::PendingFile;

#[derive(Clone)]
pub struct McpHandler {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

impl McpHandler {
    async fn get_all_inner_states(&self) -> Result<Vec<AppInnerState>, String> {
        {
            let workspaces = self.state.workspaces.read().await;
            if !workspaces.is_empty() {
                return Ok(workspaces.values().cloned().collect());
            }
        }

        let known_roots = self.state.known_roots.read().await.clone();
        let mut initialized = Vec::new();

        for root in known_roots {
            if root.join(".vectorcode").exists() {
                match crate::cli::serve::try_init_workspace(
                    &root,
                    self.state.watch,
                    self.state.debounce,
                )
                .await
                {
                    Ok(inner_state) => {
                        self.state
                            .workspaces
                            .write()
                            .await
                            .insert(root.clone(), inner_state.clone());
                        initialized.push(inner_state);
                    }
                    Err(e) => {
                        return Err(format!(
                            "Failed to lazily initialize workspace at {}: {}",
                            root.display(),
                            e
                        ))
                    }
                }
            }
        }

        if !initialized.is_empty() {
            Ok(initialized)
        } else {
            Err("VectorCode is not initialized in any workspace. Run `vectorcode init` in your project directory to set it up.".to_string())
        }
    }

    /// Validate a client-supplied `file_path` for graph tools.
    ///
    /// Returns `true` when the path resolves inside an initialized
    /// workspace, `false` otherwise. Graph tools use this to silently
    /// drop invalid disambiguation paths (preserving the non-error
    /// contract for callers that pass optional hints).
    async fn validate_graph_file_path(&self, file_path: Option<&str>) -> bool {
        let Some(fp) = file_path else {
            return true;
        };
        // Trigger lazy init so the map is populated before we resolve.
        if self.get_all_inner_states().await.is_err() {
            return false;
        }
        let workspaces = self.state.workspaces.read().await;
        crate::mcp::security::resolve_within_workspace(fp, &workspaces).is_ok()
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
    /// Search mode: "dense" (default), "sparse", "hybrid", "hybrid-rerank", or "graph"
    pub mode: Option<String>,
    /// Routing strategy: "auto" (heuristic), "graph" (force graph), "hybrid" (force hybrid), or None (default)
    pub routing: Option<String>,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VecOutlineParams {
    /// The file path to outline (relative to project root)
    pub file_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VecFindCallersParams {
    /// Symbol name to find callers of.
    pub symbol: String,
    /// Optional file path to disambiguate overloaded symbols (which file defines the target).
    pub file_path: Option<String>,
    /// Max results (default 10, max 100).
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VecFindDependentsParams {
    /// Symbol name to find dependents of (importers, inheritors, referencers).
    pub symbol: String,
    /// Optional file path to disambiguate overloaded symbols.
    pub file_path: Option<String>,
    /// Max results (default 10, max 100).
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VecTraceImportsParams {
    /// Symbol name to trace imports for (what does this symbol import?).
    pub symbol: String,
    /// Optional file path to disambiguate overloaded symbols.
    pub file_path: Option<String>,
    /// Max results (default 10, max 100).
    pub limit: Option<usize>,
}

#[tool_router]
impl McpHandler {
    #[tool(
        name = "vec_search",
        description = "Semantic code search with configurable retrieval mode. \
                       Use 'dense' for pure semantic search (default), \
                       'sparse' for keyword/lexical search, \
                       'hybrid' for combined dense+sparse with RRF fusion, \
                       'hybrid-rerank' for hybrid with cross-encoder reranking (highest quality, slower), \
                       or 'graph' for structural graph queries. \
                       Use 'routing' param to control routing: 'auto' (heuristic), 'graph' (force), 'hybrid' (force). \
                       Results are ordered by relevance.",
        annotations(read_only_hint = true)
    )]
    async fn vec_search(&self, params: Parameters<VecSearchParams>) -> Result<String, String> {
        let p = params.0;
        if p.query.is_empty() {
            return Err("Query cannot be empty".to_string());
        }

        let inner_states = self.get_all_inner_states().await?;

        // Determine effective mode based on routing param
        let mode: SearchMode = if let Some(ref routing) = p.routing {
            match routing.as_str() {
                "graph" => SearchMode::Graph,
                "hybrid" => SearchMode::Hybrid,
                "auto" => {
                    // Use heuristic classifier
                    match crate::engine::classify_query(&p.query) {
                        crate::engine::RoutingDecision::Graph(_) => SearchMode::Graph,
                        crate::engine::RoutingDecision::Hybrid => {
                            // Fall back to mode param or default
                            p.mode
                                .as_deref()
                                .unwrap_or("dense")
                                .parse()
                                .map_err(|e: String| {
                                    format!("Invalid search mode: {e}. Valid: dense, sparse, hybrid, hybrid-rerank, graph")
                                })?
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "Invalid routing: '{other}'. Valid: auto, graph, hybrid"
                    ));
                }
            }
        } else {
            // No routing param — use mode param (default: dense)
            p.mode
                .as_deref()
                .unwrap_or("dense")
                .parse()
                .map_err(|e: String| {
                    format!("Invalid search mode: {e}. Valid: dense, sparse, hybrid, hybrid-rerank, graph")
                })?
        };

        let options = SearchOptions {
            limit: p.limit.unwrap_or(10).min(100),
            threshold: p.threshold.unwrap_or(0.0),
            language: p.language,
            path: p.path,
            ..Default::default()
        };

        let mut all_results = Vec::new();
        let mut pending_files = Vec::new();

        let mut search_config = inner_states
            .first()
            .map(|s| s.config.search.clone())
            .unwrap_or_default();
        if mode == SearchMode::HybridRerank {
            search_config.rerank.enabled = true;
        }

        match mode {
            SearchMode::Hybrid | SearchMode::HybridRerank => {
                let mut global_dense = Vec::new();
                let mut global_sparse = Vec::new();

                for inner in &inner_states {
                    let repo_name = inner
                        .project_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if let Some(w) = &inner.watcher {
                        pending_files.extend(w.read().await.pending_files().await);
                    }

                    let dense_searcher = crate::engine::searcher::build_strategy(
                        SearchMode::Dense,
                        inner.db.clone(),
                        inner.embedder.clone(),
                        inner.config.search.clone(),
                    )
                    .await;

                    let sparse_searcher = crate::engine::searcher::build_strategy(
                        SearchMode::Sparse,
                        inner.db.clone(),
                        inner.embedder.clone(),
                        inner.config.search.clone(),
                    )
                    .await;

                    let (dense_result, sparse_result) = tokio::join!(
                        dense_searcher.search(&p.query, options.clone()),
                        sparse_searcher.search(&p.query, options.clone())
                    );

                    if let Ok(mut res) = dense_result {
                        for r in &mut res {
                            r.repo_name = Some(repo_name.clone());
                        }
                        global_dense.extend(res);
                    }
                    if let Ok(mut res) = sparse_result {
                        for r in &mut res {
                            r.repo_name = Some(repo_name.clone());
                        }
                        global_sparse.extend(res);
                    }
                }

                global_dense.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                global_sparse.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                all_results = crate::engine::fusion::rrf_fuse(
                    &[global_dense, global_sparse],
                    search_config.rrf_k,
                    options.limit,
                );
            }
            _ => {
                for inner in &inner_states {
                    let repo_name = inner
                        .project_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if let Some(w) = &inner.watcher {
                        pending_files.extend(w.read().await.pending_files().await);
                    }

                    let mut inner_config = inner.config.search.clone();
                    if mode == SearchMode::HybridRerank {
                        inner_config.rerank.enabled = true;
                    }

                    let searcher = crate::engine::searcher::build_strategy(
                        mode,
                        inner.db.clone(),
                        inner.embedder.clone(),
                        inner_config,
                    )
                    .await;

                    if let Ok(mut res) = searcher.search(&p.query, options.clone()).await {
                        for r in &mut res {
                            r.repo_name = Some(repo_name.clone());
                        }
                        all_results.extend(res);
                    }
                }
                all_results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                all_results.truncate(options.limit);
            }
        }

        let staleness_banner = build_staleness_banner(&all_results, &pending_files);
        let text = format_search_results_text(&p.query, p.threshold, &all_results);

        match staleness_banner {
            Some(banner) => Ok(format!("{banner}\n{text}")),
            None => Ok(text),
        }
    }

    #[tool(
        name = "vec_status",
        description = "Get the current status of the vector index across all initialized workspaces.",
        annotations(read_only_hint = true)
    )]
    async fn vec_status(&self, _params: Parameters<VecStatusParams>) -> Result<String, String> {
        let inner_states = self.get_all_inner_states().await?;
        let mut outputs = Vec::new();

        for inner in inner_states {
            let repo_name = inner
                .project_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let db = inner.db.lock().await;
            match meta::read_index_meta(db.conn()) {
                Ok(Some(index_meta)) => {
                    outputs.push(format!(
                        "Workspace: {}\n{}",
                        repo_name,
                        format_status_text(&index_meta)
                    ));
                }
                Ok(None) => {
                    outputs.push(format!(
                        "Workspace: {}\nIndex metadata not found. Run `vectorcode init`.",
                        repo_name
                    ));
                }
                Err(e) => {
                    outputs.push(format!(
                        "Workspace: {}\nFailed to read index metadata: {}",
                        repo_name, e
                    ));
                }
            }
        }
        Ok(outputs.join("\n\n------------------------\n\n"))
    }

    #[tool(
        name = "vec_reindex",
        description = "Trigger a background re-index of the project. \
                       Use full=true to drop the existing index and start fresh.",
        annotations(destructive_hint = true)
    )]
    async fn vec_reindex(&self, params: Parameters<VecReindexParams>) -> Result<String, String> {
        let p = params.0;
        let inner_states = self.get_all_inner_states().await?;
        let mut outputs = Vec::new();

        for inner_state in inner_states {
            let repo_name = inner_state
                .project_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            if p.full {
                let db = inner_state.db.lock().await;
                if let Err(e) = db.clear_database() {
                    outputs.push(format!(
                        "Workspace {}: Failed to clear database: {}",
                        repo_name, e
                    ));
                    continue;
                }
                if let Err(e) = db.init_schema(inner_state.embedder.dimensions()) {
                    outputs.push(format!(
                        "Workspace {}: Failed to reinitialize schema: {}",
                        repo_name, e
                    ));
                    continue;
                }
            }

            let indexer = Indexer::new(
                inner_state.db.clone(),
                inner_state.embedder.clone(),
                inner_state.config.indexing.clone(),
            );

            match indexer.index_project(&inner_state.project_path).await {
                Ok(report) => outputs.push(format!(
                    "Workspace {}: Re-indexing complete.\n\
                         Files scanned:  {}\n\
                         Files indexed:  {}\n\
                         Chunks total:   {}\n\
                         Chunks new:     {}\n\
                         Chunks skipped: {}\n\
                         Duration:       {:.2}s\n",
                    repo_name,
                    report.files_scanned,
                    report.files_indexed,
                    report.chunks_total,
                    report.chunks_new,
                    report.chunks_skipped,
                    report.duration.as_secs_f64(),
                )),
                Err(e) => {
                    error!("vec_reindex failed for {}: {}", repo_name, e);
                    outputs.push(format!(
                        "Workspace {}: Re-indexing failed: {}",
                        repo_name, e
                    ));
                }
            }
        }
        Ok(outputs.join("\n\n"))
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
        // Trigger lazy workspace init so the map is populated before we resolve.
        let _ = self.get_all_inner_states().await?;

        // Delegate path validation to the shared security helper.
        // BTreeMap iteration is deterministic, so the resolution is stable
        // across overlapping workspaces (R7).
        let workspaces = self.state.workspaces.read().await;
        let (canonical, _inner_state) =
            crate::mcp::security::resolve_within_workspace(&p.file_path, &workspaces)
                .map_err(|e| format!("Access denied: {e}"))?;
        drop(workspaces);

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

    #[tool(
        name = "vec_outline",
        description = "Get a structural outline of a source file — top-level functions, \
                       classes, structs, interfaces, and traits with their signatures. \
                       Useful for understanding file structure without reading the entire file.",
        annotations(read_only_hint = true)
    )]
    async fn vec_outline(&self, params: Parameters<VecOutlineParams>) -> Result<String, String> {
        let p = params.0;
        // Trigger lazy workspace init so the map is populated before we resolve.
        let _ = self.get_all_inner_states().await?;

        // Delegate path validation to the shared security helper.
        let workspaces = self.state.workspaces.read().await;
        let (canonical, _inner_state) =
            crate::mcp::security::resolve_within_workspace(&p.file_path, &workspaces)
                .map_err(|e| format!("Access denied: {e}"))?;
        drop(workspaces);

        let metadata = tokio::fs::metadata(&canonical)
            .await
            .map_err(|e| format!("Failed to get file metadata: {e}"))?;
        if metadata.len() > 2 * 1024 * 1024 {
            return Err("Access denied: File size exceeds the maximum limit of 2MB.".to_string());
        }

        let source = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|e| format!("Failed to read file: {e}"))?;

        let ext = canonical.extension().and_then(|e| e.to_str()).unwrap_or("");
        let language = SupportedLanguage::from_extension(ext);

        let items = outliner::outline_file(&source, &p.file_path, language);

        if items.is_empty() {
            return Ok(format!(
                "No outline items found for {} (language: {}). \
                 The file may be empty, unsupported, or contain no top-level symbols.",
                p.file_path,
                language.as_str()
            ));
        }

        let mut output = format!("Outline of {} ({} items):\n\n", p.file_path, items.len());
        for item in &items {
            let vis = item
                .visibility
                .as_deref()
                .map(|v| format!("{v} "))
                .unwrap_or_default();
            output.push_str(&format!(
                "  L{:<5} {}{} {}\n",
                item.start_line, vis, item.kind, item.signature
            ));
        }

        Ok(output)
    }

    #[tool(
        name = "vec_find_callers",
        description = "Find all functions/methods that call a given symbol. \
                       Returns a human-readable list of callers with file paths. \
                       Use file_path to disambiguate when the same symbol name exists in multiple files.",
        annotations(read_only_hint = true)
    )]
    async fn vec_find_callers(
        &self,
        params: Parameters<VecFindCallersParams>,
    ) -> Result<String, String> {
        let p = params.0;
        let inner_states = self.get_all_inner_states().await?;

        let mut all_nodes = Vec::new();

        for inner in inner_states {
            let db = inner.db.lock().await;

            let nodes = if let Some(fp) = p.file_path.as_deref() {
                if self.validate_graph_file_path(Some(fp)).await {
                    crate::store::graph::get_callers_filtered(db.conn(), &p.symbol, Some(fp))
                } else {
                    // Invalid disambiguation path → empty result for this workspace.
                    Ok(Vec::new())
                }
            } else {
                use crate::store::graph::GraphStore;
                db.get_callers(&p.symbol)
            };

            if let Ok(nodes) = nodes {
                all_nodes.extend(nodes);
            } else if let Err(e) = nodes {
                error!("vec_find_callers failed for workspace: {e}");
            }
        }

        let limit = p.limit.unwrap_or(10).min(100);
        let truncated = &all_nodes[..all_nodes.len().min(limit)];
        Ok(format_graph_results_text("callers", &p.symbol, truncated))
    }

    #[tool(
        name = "vec_find_dependents",
        description = "Find all symbols that depend on a given symbol (importers, inheritors, referencers). \
                       Returns a human-readable list of dependents with file paths. \
                       Use file_path to disambiguate when the same symbol name exists in multiple files.",
        annotations(read_only_hint = true)
    )]
    async fn vec_find_dependents(
        &self,
        params: Parameters<VecFindDependentsParams>,
    ) -> Result<String, String> {
        let p = params.0;
        let inner_states = self.get_all_inner_states().await?;

        let mut all_nodes = Vec::new();

        for inner in inner_states {
            let db = inner.db.lock().await;

            use crate::store::graph::GraphStore;
            let nodes = if let Some(fp) = p.file_path.as_deref() {
                if self.validate_graph_file_path(Some(fp)).await {
                    db.get_dependents(&p.symbol, Some(fp))
                } else {
                    // Invalid disambiguation path → empty result for this workspace.
                    Ok(Vec::new())
                }
            } else {
                db.get_dependents(&p.symbol, None)
            };

            if let Ok(nodes) = nodes {
                all_nodes.extend(nodes);
            } else if let Err(e) = nodes {
                error!("vec_find_dependents failed for workspace: {e}");
            }
        }

        let limit = p.limit.unwrap_or(10).min(100);
        let truncated = &all_nodes[..all_nodes.len().min(limit)];
        Ok(format_graph_results_text(
            "dependents",
            &p.symbol,
            truncated,
        ))
    }

    #[tool(
        name = "vec_trace_imports",
        description = "Trace what a symbol imports (outgoing Import edges). \
                       Returns a human-readable list of imported symbols with file paths. \
                       Use file_path to disambiguate when the same symbol name exists in multiple files.",
        annotations(read_only_hint = true)
    )]
    async fn vec_trace_imports(
        &self,
        params: Parameters<VecTraceImportsParams>,
    ) -> Result<String, String> {
        let p = params.0;
        let inner_states = self.get_all_inner_states().await?;

        let mut all_nodes = Vec::new();

        for inner in inner_states {
            let db = inner.db.lock().await;

            use crate::store::graph::GraphStore;
            let nodes = if let Some(fp) = p.file_path.as_deref() {
                if self.validate_graph_file_path(Some(fp)).await {
                    db.get_imports(&p.symbol, Some(fp))
                } else {
                    // Invalid disambiguation path → empty result for this workspace.
                    Ok(Vec::new())
                }
            } else {
                db.get_imports(&p.symbol, None)
            };

            if let Ok(nodes) = nodes {
                all_nodes.extend(nodes);
            } else if let Err(e) = nodes {
                error!("vec_trace_imports failed for workspace: {e}");
            }
        }

        let limit = p.limit.unwrap_or(10).min(100);
        let truncated = &all_nodes[..all_nodes.len().min(limit)];
        Ok(format_graph_results_text("imports", &p.symbol, truncated))
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
                                to fetch only the specific lines you need. \
                                Don't call `vec_read_lines` sequentially (e.g., 1-100, 100-200) to \
                                reconstruct an entire file. Use `vec_search` to find relevant code, then \
                                read only the specific lines you need.".to_string()),
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
                    let mut found_roots = Vec::new();
                    let mut workspaces_to_add = Vec::new();

                    for root in &roots_result.roots {
                        if let Ok(url) = url::Url::parse(&root.uri) {
                            if url.scheme() == "file" {
                                if let Ok(path) = url.to_file_path() {
                                    found_roots.push(path.clone());
                                    if path.join(".vectorcode").exists() {
                                        tracing::info!(
                                            "Found initialized vectorcode workspace at {}",
                                            path.display()
                                        );
                                        match crate::cli::serve::try_init_workspace(
                                            &path,
                                            state.watch,
                                            state.debounce,
                                        )
                                        .await
                                        {
                                            Ok(inner) => {
                                                workspaces_to_add.push((path.clone(), inner));
                                            }
                                            Err(e) => {
                                                tracing::error!(
                                                    "Failed to initialize workspace from root {}: {}",
                                                    path.display(),
                                                    e
                                                );
                                            }
                                        }
                                    } else {
                                        tracing::info!("Found root {} but it is not initialized. Awaiting user to run `vectorcode init`.", path.display());
                                    }
                                }
                            }
                        }
                    }

                    if found_roots.is_empty() {
                        tracing::info!("No roots provided by client.");
                    } else {
                        *state.known_roots.write().await = found_roots;
                    }

                    if !workspaces_to_add.is_empty() {
                        let mut workspaces = state.workspaces.write().await;
                        for (path, inner) in workspaces_to_add {
                            workspaces.insert(path, inner);
                        }
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

        let repo_prefix = res
            .repo_name
            .as_ref()
            .map(|r| format!("[Repo: {}] ", r))
            .unwrap_or_default();

        out.push_str(&format!(
            "{}. {}{}:L{}-{}{} [{}] (score: {:.2})\n",
            i + 1,
            repo_prefix,
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

fn format_graph_results_text(
    kind: &str,
    symbol: &str,
    nodes: &[crate::types::GraphNode],
) -> String {
    if nodes.is_empty() {
        return format!("No graph data for '{symbol}'. Run 'vectorcode reindex' to populate.");
    }

    let mut out = format!("Found {} {} for '{}':\n\n", nodes.len(), kind, symbol);
    for (i, node) in nodes.iter().enumerate() {
        out.push_str(&format!(
            "{}. {}::{} ({})\n",
            i + 1,
            node.file_path,
            node.symbol,
            node.kind
        ));
    }
    out.push_str("\nUse vec_read_lines to inspect.");
    out
}
