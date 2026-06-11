//! `vectorcode serve` — start the MCP server (spec §12.5).

use anyhow::Result;
use clap::Args;

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
/// Currently a stub — MCP server implementation is in the next phase.
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

    // TODO: Implement MCP server (Phase 6)
    // This will:
    // 1. Load config and create embedder
    // 2. Open database
    // 3. Start file watcher if not --no-watch
    // 4. Start MCP server on stdio
    let watch = !args.no_watch;
    eprintln!("MCP server starting on stdio...");
    eprintln!("  Project: {}", project_path.display());
    eprintln!("  Watch:   {watch}");
    eprintln!("  Debounce: {}ms", args.debounce);
    eprintln!();
    eprintln!("Note: MCP server implementation is coming in the next phase.");
    eprintln!("The serve command will be fully functional after Phase 6.");

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
    fn serve_succeeds_after_init() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        // Init first
        let init_args = crate::cli::init::InitArgs {
            provider: crate::cli::ProviderArg::Onnx,
            model: None,
            dims: None,
            index: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(crate::cli::init::execute(&init_args, project_path, true))
            .unwrap();

        // Serve should succeed (stub)
        let serve_args = ServeArgs {
            mcp: true,
            no_watch: false,
            debounce: 2000,
        };
        let result = rt.block_on(execute(&serve_args, project_path));
        assert!(
            result.is_ok(),
            "Serve stub should succeed: {:?}",
            result.err()
        );
    }
}
