//! `vectorcode serve` — start the MCP server (spec §12.5).

use anyhow::Result;
use clap::Args;

use crate::cli::create_embedder_from_config;
use crate::config;
use crate::mcp::{AppState, McpServer};
use crate::store::db::Database;

/// Arguments for `vectorcode serve`.
#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Start as MCP server (stdio transport).
    #[arg(long)]
    pub mcp: bool,

    /// Disable file watcher for auto-sync.
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

    // Open database
    let db_path = vc_dir.join("index.db");
    let db = Database::open(&db_path).map_err(|e| anyhow::anyhow!("{e}"))?;

    // Read dimensions from meta to init schema if needed
    let meta = crate::store::meta::read_index_meta(db.conn())?;
    if let Some(ref m) = meta {
        db.init_schema(m.dimensions)?;
    }

    // Create embedder
    let embedder = create_embedder_from_config(&cfg)?;

    let watch = !args.no_watch;
    tracing::info!("MCP server starting on stdio");
    tracing::info!("  Project: {}", project_path.display());
    tracing::info!("  Watch:   {watch}");
    tracing::info!("  Debounce: {}ms", args.debounce);

    // Create AppState and McpServer
    let state = AppState {
        db,
        embedder,
        config: cfg,
        project_path: project_path.to_path_buf(),
    };

    let mut server = McpServer::new(state);

    // Set up Ctrl+C handler
    let _ctrl_c = tokio::spawn(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Received Ctrl+C, shutting down...");
        // The server loop will exit when stdin closes
    });

    // Run the MCP server main loop
    server.run().await?;

    Ok(())
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
    fn serve_fails_with_onnx_provider_no_model() {
        // ONNX provider requires bundled model files which aren't available yet
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        // Init with onnx provider (default)
        let init_args = crate::cli::init::InitArgs {
            provider: crate::cli::ProviderArg::Onnx,
            model: None,
            dims: None,
            index: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(crate::cli::init::execute(&init_args, project_path, true))
            .unwrap();

        // Serve should fail because ONNX embedder can't be created
        let serve_args = ServeArgs {
            mcp: true,
            no_watch: false,
            debounce: 2000,
        };
        let result = rt.block_on(execute(&serve_args, project_path));
        assert!(result.is_err(), "Should fail with ONNX provider");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("ONNX"), "Got: {err}");
    }
}
