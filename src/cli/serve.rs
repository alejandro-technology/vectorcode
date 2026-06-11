//! `vectorcode serve` — start the MCP server (spec §12.5).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use tracing::info;

use crate::cli::create_embedder_from_config;
use crate::config;
use crate::engine::indexer::Indexer;
use crate::mcp::{AppState, McpServer};
use crate::store::db::Database;
use crate::store::files;
use crate::watcher::FileWatcher;

/// Arguments for `vectorcode serve`.
#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Start as MCP server (stdio transport).
    #[arg(long)]
    pub mcp: bool,

    /// Disable file watcher for auto-sync.
    ///
    /// Note: File watching is ENABLED by default. Use `--no-watch` to disable.
    /// Clap doesn't support a clean `--watch`/`--no-watch` pair with default=true,
    /// so we use the inverted boolean pattern. This is a known clap limitation.
    #[arg(long)]
    pub no_watch: bool,

    /// File watcher debounce interval in milliseconds.
    #[arg(long, default_value = "2000")]
    pub debounce: u64,
}

/// Execute the `serve` command.
///
/// Creates the MCP server with shared application state and runs the
/// JSON-RPC message loop over stdio.
pub async fn execute(args: &ServeArgs, project_path: &std::path::Path) -> Result<()> {
    if !args.mcp {
        anyhow::bail!("Only --mcp mode is supported. Use: vectorcode serve --mcp");
    }

    let vc_dir = project_path.join(".vectorcode");
    if !vc_dir.exists() {
        anyhow::bail!(
            "VectorCode is not initialized in {}.\nRun `vectorcode init` first.",
            project_path.display()
        );
    }

    // Load config
    let mut cfg = config::load_config(project_path)?;
    cfg.apply_env_overrides();

    // Override debounce from CLI arg
    cfg.watcher.debounce_ms = args.debounce;

    // Create embedder first — we need its dimensions to ensure schema exists
    let embedder = create_embedder_from_config(&cfg)?;

    // Open database and ensure schema exists (idempotent via user_version check)
    let db_path = vc_dir.join("index.db");
    let db = Database::open(&db_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    db.init_schema(embedder.dimensions())?;

    // Validate provider dimensions match the existing index
    if let Some(existing_meta) =
        crate::store::meta::read_index_meta(db.conn()).map_err(|e| anyhow::anyhow!("{e}"))?
    {
        if existing_meta.dimensions != embedder.dimensions() {
            anyhow::bail!(
                "Provider dimension mismatch: index was built with {} dimensions, \
                 but current provider uses {} dimensions. Re-index with 'vectorcode index' \
                 or change the provider.",
                existing_meta.dimensions,
                embedder.dimensions()
            );
        }
    }

    let watch = !args.no_watch && !cfg.watcher.disabled;
    info!("MCP server starting on stdio");
    info!("  Project: {}", project_path.display());
    info!("  Watch:   {watch}");
    info!("  Debounce: {}ms", args.debounce);

    // Create file watcher if enabled
    let watcher = if watch {
        let watcher_config = &cfg.watcher;
        match FileWatcher::new(project_path, watcher_config) {
            Ok(mut w) => {
                if let Err(e) = w.start() {
                    tracing::warn!("Failed to start file watcher: {e}");
                    None
                } else {
                    Some(Arc::new(tokio::sync::RwLock::new(w)))
                }
            }
            Err(e) => {
                tracing::warn!("Failed to create file watcher: {e}");
                None
            }
        }
    } else {
        None
    };

    // Create AppState and McpServer
    let state = AppState {
        db: Arc::new(std::sync::Mutex::new(db)),
        embedder: embedder.clone(),
        config: cfg.clone(),
        project_path: project_path.to_path_buf(),
        watcher: watcher.clone(),
    };

    // Start background watcher task if watcher is available
    if let Some(watcher_arc) = &watcher {
        let watcher_for_task = watcher_arc.clone();
        let db_path_for_task = project_path.join(".vectorcode").join("index.db");
        let project_path_for_task = project_path.to_path_buf();
        let embedder_for_task = embedder.clone();
        let indexing_config = cfg.indexing.clone();

        tokio::spawn(async move {
            run_watcher_background(
                watcher_for_task,
                db_path_for_task,
                project_path_for_task,
                embedder_for_task,
                indexing_config,
            )
            .await;
        });
    }

    let mut server = McpServer::new(state);

    // Spawn connect-time catch-up on a blocking thread (spec §14.3).
    // Runs on the blocking pool so the MCP message loop enters immediately
    // and the client's initialize request is answered without timeout.
    // Uses spawn_blocking because Indexer (rusqlite internals) is !Send.
    if watch {
        let catchup_db_path = db_path.clone();
        let catchup_embedder = embedder.clone();
        let catchup_cfg = cfg.clone();
        let catchup_project = project_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            if let Err(e) = run_connect_time_catchup(
                &catchup_db_path,
                catchup_embedder,
                &catchup_cfg,
                &catchup_project,
            ) {
                tracing::warn!("Connect-time catch-up failed: {e}");
            }
        });
    }

    // Set up Ctrl+C handler
    let _ctrl_c = tokio::spawn(async {
        let _ = tokio::signal::ctrl_c().await;
        info!("Received Ctrl+C, shutting down...");
        // The server loop will exit when stdin closes
    });

    // Run the MCP server main loop
    server.run().await?;

    Ok(())
}

/// Run connect-time catch-up: reconcile files changed since last index (spec §14.3).
///
/// Compares file mtimes on disk against the `files` table and runs incremental
/// sync on any files that changed while no MCP server was running.
///
/// Called from `spawn_blocking` so the MCP message loop starts immediately
/// and the client's `initialize` request is answered without timeout.
fn run_connect_time_catchup(
    db_path: &std::path::Path,
    embedder: Arc<dyn crate::embedder::Embedder>,
    cfg: &config::schema::Config,
    project_path: &std::path::Path,
) -> Result<()> {
    info!("Running connect-time catch-up sync...");

    // Open DB to list tracked files
    let db = Database::open(db_path).map_err(|e| anyhow::anyhow!("{e}"))?;

    let tracked_files = files::list_all_files(db.conn())?;
    let mut changed_paths = Vec::new();

    for record in &tracked_files {
        let full_path = project_path.join(&record.path);
        if !full_path.exists() {
            continue;
        }

        if let Ok(metadata) = full_path.metadata() {
            let current_mtime = metadata
                .modified()
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                })
                .unwrap_or(0);
            let current_size = metadata.len() as i64;

            if current_mtime != record.mtime || current_size != record.size {
                changed_paths.push(full_path);
            }
        }
    }

    if changed_paths.is_empty() {
        info!("Connect-time catch-up: no changes detected");
        return Ok(());
    }

    info!(
        "Connect-time catch-up: {} files changed, running incremental sync",
        changed_paths.len()
    );

    // Open a fresh DB handle for the indexer (SQLite handles concurrent access)
    let sync_db = Database::open(db_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let indexer = Indexer::new(
        std::sync::Arc::new(std::sync::Mutex::new(sync_db)),
        embedder,
        cfg.indexing.clone(),
    );

    // block_on is safe here because we are on a spawn_blocking thread,
    // not the main async runtime.
    let rt = tokio::runtime::Handle::current();
    let report = rt.block_on(indexer.index_files(&changed_paths, project_path))?;

    info!(
        "Connect-time catch-up complete: {} files, {} new chunks in {:.2}s",
        report.files_indexed,
        report.chunks_new,
        report.duration.as_secs_f64()
    );

    Ok(())
}

/// Background task that waits for debounced file change batches and runs
/// incremental sync via the Indexer. Also handles file deletion events.
async fn run_watcher_background(
    watcher: Arc<tokio::sync::RwLock<FileWatcher>>,
    db_path: std::path::PathBuf,
    project_path: std::path::PathBuf,
    embedder: Arc<dyn crate::embedder::Embedder>,
    indexing_config: crate::config::schema::IndexingConfig,
) {
    info!("File watcher background task started");

    loop {
        let batch = {
            let mut w = watcher.write().await;
            w.next_batch().await
        };

        let entries = match batch {
            Some(entries) => entries,
            None => {
                info!("File watcher channel closed, background task exiting");
                break;
            }
        };

        if entries.is_empty() {
            continue;
        }

        // Partition into modifications and removals
        let mut mod_paths: Vec<PathBuf> = Vec::new();
        let mut removal_paths: Vec<PathBuf> = Vec::new();
        for (path, is_removal) in entries {
            if is_removal {
                removal_paths.push(path);
            } else {
                mod_paths.push(path);
            }
        }

        // Handle file removals: delete chunks and file records from the index
        if !removal_paths.is_empty() {
            info!(
                "Watcher batch: {} files removed, cleaning up index",
                removal_paths.len()
            );
            let db_path_rm = db_path.clone();
            let project_path_rm = project_path.clone();
            let removals = removal_paths.clone();

            let rm_result = tokio::task::spawn_blocking(move || {
                let rm_db = Database::open(&db_path_rm)?;
                for abs_path in &removals {
                    // Convert absolute path to relative path for DB lookup
                    let rel_path = abs_path
                        .strip_prefix(&project_path_rm)
                        .unwrap_or(abs_path)
                        .to_string_lossy()
                        .to_string();
                    let _ = crate::store::chunks::delete_chunks_for_file(rm_db.conn(), &rel_path);
                    let _ = crate::store::files::remove_file(rm_db.conn(), &rel_path);
                }
                Ok::<_, anyhow::Error>(())
            })
            .await;

            match rm_result {
                Ok(Ok(())) => {
                    info!(
                        "Watcher removal cleanup complete for {} files",
                        removal_paths.len()
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!("Watcher removal cleanup failed: {e}");
                }
                Err(e) => {
                    tracing::error!("Watcher removal cleanup task panicked: {e}");
                }
            }
        }

        // Handle file modifications: run incremental sync
        if mod_paths.is_empty() {
            continue;
        }

        info!(
            "Watcher batch: {} files changed, running incremental sync",
            mod_paths.len()
        );

        // Run the sync in a blocking thread to avoid Send issues with rusqlite
        let db_path_clone = db_path.clone();
        let project_path_clone = project_path.clone();
        let embedder_clone = embedder.clone();
        let indexing_config_clone = indexing_config.clone();
        let paths_clone = mod_paths.clone();

        let sync_result = tokio::task::spawn_blocking(move || {
            let sync_db = Database::open(&db_path_clone)?;
            let indexer = Indexer::new(
                std::sync::Arc::new(std::sync::Mutex::new(sync_db)),
                embedder_clone,
                indexing_config_clone,
            );

            // Create a local runtime for the async index_files call
            let rt = tokio::runtime::Handle::current();
            rt.block_on(indexer.index_files(&paths_clone, &project_path_clone))
        })
        .await;

        match sync_result {
            Ok(Ok(report)) => {
                info!(
                    "Watcher sync complete: {} files, {} new chunks",
                    report.files_indexed, report.chunks_new
                );

                // Clear the synced files from pending
                let w = watcher.read().await;
                w.clear_pending_paths(&mod_paths).await;
            }
            Ok(Err(e)) => {
                tracing::error!("Watcher sync failed: {e}");
            }
            Err(e) => {
                tracing::error!("Watcher sync task panicked: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn serve_args_parse_mcp() {
        let cli = Cli::parse_from(["vectorcode", "serve", "--mcp"]);
        match cli.command {
            crate::cli::Commands::Serve(args) => {
                assert!(args.mcp);
                assert!(!args.no_watch);
                assert_eq!(args.debounce, 2000);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn serve_args_parse_custom_debounce() {
        let cli = Cli::parse_from(["vectorcode", "serve", "--mcp", "--debounce", "5000"]);
        match cli.command {
            crate::cli::Commands::Serve(args) => {
                assert_eq!(args.debounce, 5000);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn serve_args_parse_no_watch() {
        let cli = Cli::parse_from(["vectorcode", "serve", "--mcp", "--no-watch"]);
        match cli.command {
            crate::cli::Commands::Serve(args) => {
                assert!(args.mcp);
                assert!(args.no_watch);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn serve_fails_without_mcp_flag() {
        let args = ServeArgs {
            mcp: false,
            no_watch: false,
            debounce: 2000,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(execute(&args, std::path::Path::new("/tmp")));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("--mcp"), "Got: {err}");
    }

    #[test]
    fn serve_fails_without_init() {
        let dir = tempfile::tempdir().unwrap();
        let args = ServeArgs {
            mcp: true,
            no_watch: false,
            debounce: 2000,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(execute(&args, dir.path()));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not initialized"), "Got: {err}");
    }

    #[test]
    fn serve_fails_with_gemini_no_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        // Init with gemini provider
        let init_args = crate::cli::init::InitArgs {
            provider: Some(crate::cli::ProviderArg::Gemini),
            model: None,
            dims: None,
            index: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(crate::cli::init::execute(&init_args, project_path, true))
            .unwrap();

        // Serve should fail because Gemini api_key is empty
        let serve_args = ServeArgs {
            mcp: true,
            no_watch: false,
            debounce: 2000,
        };
        let result = rt.block_on(execute(&serve_args, project_path));
        assert!(
            result.is_err(),
            "Should fail with Gemini provider missing key"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("API key") || err.contains("GEMINI_API_KEY"),
            "Got: {err}"
        );
    }
}
