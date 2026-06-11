//! CLI module — clap derive definitions and command dispatch (spec §12).
//!
//! Each subcommand is implemented in its own module. This module defines
//! the top-level `Cli` struct and shared helpers.

pub mod index;
pub mod init;
pub mod install;
pub mod search;
pub mod serve;
pub mod status;
pub mod uninstall;
pub mod upgrade;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

use crate::config::schema::Config;
use crate::embedder::mock::MockEmbedder;
use crate::embedder::Embedder;

/// VectorCode — semantic code search MCP server.
#[derive(Parser, Debug)]
#[command(name = "vectorcode", version, about)]
pub struct Cli {
    /// Path to the project directory (default: current directory).
    #[arg(long, global = true)]
    pub project_path: Option<PathBuf>,

    /// Enable verbose logging to stderr.
    #[arg(long, short, global = true)]
    pub verbose: bool,

    /// Suppress progress output.
    #[arg(long, short, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Commands,
}

/// Available subcommands per spec §12.1.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize VectorCode in a project directory.
    Init(init::InitArgs),
    /// Build or update the embedding index.
    Index(index::IndexArgs),
    /// Search the index from the command line.
    Search(search::SearchArgs),
    /// Show index status and health.
    Status(status::StatusArgs),
    /// Start the MCP server.
    Serve(serve::ServeArgs),
    /// Auto-configure agents (OpenCode, Claude Code, Cursor, etc.).
    Install(install::InstallArgs),
    /// Remove VectorCode from agent configurations.
    Uninstall(uninstall::UninstallArgs),
    /// Self-update the binary.
    Upgrade(upgrade::UpgradeArgs),
}

/// Supported embedding providers for the `init --provider` flag.
#[derive(Debug, Clone, ValueEnum)]
pub enum ProviderArg {
    Onnx,
    Gemini,
    Ollama,
    Openai,
}

impl ProviderArg {
    /// Convert to the string used in config.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Onnx => "onnx",
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
            Self::Openai => "openai",
        }
    }
}

/// Resolve the project path from CLI args.
///
/// If no explicit path is provided, it starts at the current directory and walks up
/// the directory tree looking for a `.vectorcode` folder. If none is found, it falls
/// back to the current directory.
pub fn resolve_project_path(cli_path: Option<&PathBuf>) -> PathBuf {
    if let Some(path) = cli_path {
        return path.clone();
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Walk up the tree looking for .vectorcode
    let mut current = cwd.as_path();
    loop {
        if current.join(".vectorcode").exists() {
            return current.to_path_buf();
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break, // Reached root without finding it
        }
    }

    // Fallback to cwd if not found anywhere (commands will handle the "not initialized" error)
    cwd
}

/// Create an embedder from config — maps ProviderConfig to the correct implementation.
///
/// For ONNX: loads from cached model via `OnnxEmbedder::from_cache()`.
/// For API providers: reads API keys from config (which already has env overrides applied).
/// Falls back to MockEmbedder for testing when real providers aren't available.
pub fn create_embedder_from_config(config: &Config) -> Result<Arc<dyn Embedder>> {
    match config.provider.name.as_str() {
        "onnx" => {
            let embedder = crate::embedder::onnx::OnnxEmbedder::from_cache().map_err(|e| {
                anyhow::anyhow!(
                    "ONNX model not downloaded. Run `vectorcode init` to download it. ({e})"
                )
            })?;
            Ok(Arc::new(embedder))
        }
        "gemini" => {
            let gemini_cfg = config.provider.gemini.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Gemini provider selected but no [provider.gemini] in config")
            })?;
            let embedder = crate::embedder::gemini::GeminiEmbedder::new(
                gemini_cfg.api_key.clone(),
                gemini_cfg.dimensions,
            )?;
            Ok(Arc::new(embedder))
        }
        "ollama" => {
            let ollama_cfg = config.provider.ollama.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Ollama provider selected but no [provider.ollama] in config")
            })?;
            let embedder = crate::embedder::ollama::OllamaEmbedder::with_config(
                ollama_cfg.url.clone(),
                ollama_cfg.model.clone(),
            )?;
            Ok(Arc::new(embedder))
        }
        "openai" => {
            let openai_cfg = config.provider.openai.as_ref().ok_or_else(|| {
                anyhow::anyhow!("OpenAI provider selected but no [provider.openai] in config")
            })?;
            let embedder =
                crate::embedder::openai::OpenAiEmbedder::new(openai_cfg.api_key.clone())?;
            Ok(Arc::new(embedder))
        }
        "mock" => {
            // Special provider for testing
            Ok(Arc::new(MockEmbedder::new(384)))
        }
        other => anyhow::bail!("Unknown provider: {other}"),
    }
}

/// Initialize tracing subscriber for CLI commands.
///
/// Logs go to stderr (stdout is reserved for command output).
pub fn init_tracing(verbose: bool, quiet: bool) {
    let level = if quiet {
        tracing::Level::ERROR
    } else if verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(level.into()),
        )
        .with_writer(std::io::stderr)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use serial_test::serial;

    #[test]
    fn cli_debug_assert_passes() {
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_has_all_subcommands() {
        let cmd = Cli::command();
        let subcommand_names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();

        assert!(
            subcommand_names.contains(&"init"),
            "Missing init: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"index"),
            "Missing index: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"search"),
            "Missing search: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"status"),
            "Missing status: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"serve"),
            "Missing serve: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"install"),
            "Missing install: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"uninstall"),
            "Missing uninstall: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"upgrade"),
            "Missing upgrade: {subcommand_names:?}"
        );
    }

    #[test]
    fn cli_parse_init_defaults() {
        let cli = Cli::parse_from(["vectorcode", "init"]);
        assert!(matches!(cli.command, Commands::Init(_)));
        assert!(cli.project_path.is_none());
        assert!(!cli.verbose);
        assert!(!cli.quiet);
    }

    #[test]
    fn cli_parse_global_options() {
        let cli = Cli::parse_from([
            "vectorcode",
            "--project-path",
            "/tmp/test",
            "--verbose",
            "status",
        ]);
        assert_eq!(cli.project_path, Some(PathBuf::from("/tmp/test")));
        assert!(cli.verbose);
        assert!(matches!(cli.command, Commands::Status(_)));
    }

    #[test]
    fn cli_parse_search_with_query() {
        let cli = Cli::parse_from(["vectorcode", "search", "payment retry logic"]);
        match cli.command {
            Commands::Search(args) => {
                assert_eq!(args.query, "payment retry logic");
            }
            _ => panic!("Expected Search command"),
        }
    }

    #[test]
    fn resolve_project_path_uses_cli_path_when_given() {
        let path = PathBuf::from("/custom/path");
        let resolved = resolve_project_path(Some(&path));
        assert_eq!(resolved, PathBuf::from("/custom/path"));
    }

    #[test]
    fn resolve_project_path_falls_back_to_cwd() {
        // If we run this test in a directory without .vectorcode anywhere in its parents,
        // it falls back to cwd. To ensure a predictable test environment, we use a temp dir.
        let temp = tempfile::tempdir().unwrap();
        let temp_path = std::fs::canonicalize(temp.path()).unwrap();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_path).unwrap();

        let resolved = resolve_project_path(None);
        assert_eq!(resolved, temp_path);

        std::env::set_current_dir(prev_cwd).unwrap();
    }

    #[test]
    fn resolve_project_path_finds_parent_dir() {
        let temp = tempfile::tempdir().unwrap();
        let project_root = std::fs::canonicalize(temp.path()).unwrap();

        // Create .vectorcode in root
        std::fs::create_dir(project_root.join(".vectorcode")).unwrap();

        // Create a deep subdirectory
        let deep_dir = project_root.join("src").join("cli").join("nested");
        std::fs::create_dir_all(&deep_dir).unwrap();

        // Change cwd to deep directory
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&deep_dir).unwrap();

        // Resolve without explicit path should find the root
        let resolved = resolve_project_path(None);
        assert_eq!(resolved, project_root);

        // Restore cwd
        std::env::set_current_dir(prev_cwd).unwrap();
    }

    #[test]
    fn provider_arg_as_str_values() {
        assert_eq!(ProviderArg::Onnx.as_str(), "onnx");
        assert_eq!(ProviderArg::Gemini.as_str(), "gemini");
        assert_eq!(ProviderArg::Ollama.as_str(), "ollama");
        assert_eq!(ProviderArg::Openai.as_str(), "openai");
    }

    #[test]
    fn create_embedder_unknown_provider_errors() {
        let mut config = Config::default();
        config.provider.name = "nonexistent".to_string();
        let result = create_embedder_from_config(&config);
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().unwrap());
        assert!(err_msg.contains("Unknown provider"), "Got: {err_msg}");
    }

    #[test]
    fn create_embedder_mock_provider_works() {
        let mut config = Config::default();
        config.provider.name = "mock".to_string();
        let result = create_embedder_from_config(&config);
        assert!(result.is_ok());
        let embedder = result.unwrap();
        assert_eq!(embedder.provider_name(), "mock");
        assert_eq!(embedder.dimensions(), 384);
    }

    #[test]
    #[serial(onnx)]
    fn create_embedder_onnx_without_model_errors() {
        // Deterministic: uses an empty temp dir — model not cached, so
        // ModelManager returns an error before reaching ONNX Runtime init.
        let empty_dir = tempfile::tempdir().unwrap();
        let result =
            crate::embedder::onnx::OnnxEmbedder::from_model_dir(empty_dir.path().to_path_buf());
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().unwrap());
        assert!(
            err_msg.contains("vectorcode init"),
            "Error should suggest running vectorcode init, got: {err_msg}"
        );
    }

    #[test]
    #[serial(onnx)]
    fn create_embedder_onnx_error_is_embedder_error_variant() {
        // Deterministic: empty model dir → error, verify it's the right
        // error variant (EmbedderError, not a panic or unexpected type).
        let empty_dir = tempfile::tempdir().unwrap();
        let result =
            crate::embedder::onnx::OnnxEmbedder::from_model_dir(empty_dir.path().to_path_buf());
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().unwrap());
        assert!(
            err_msg.contains("ONNX"),
            "Error should be an ONNX-related embedder error, got: {err_msg}"
        );
    }

    #[test]
    fn create_embedder_gemini_without_config_errors() {
        let mut config = Config::default();
        config.provider.name = "gemini".to_string();
        // No gemini config section
        let result = create_embedder_from_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn create_embedder_gemini_with_empty_api_key_errors() {
        let mut config = Config::default();
        config.provider.name = "gemini".to_string();
        config.provider.gemini = Some(crate::config::schema::GeminiConfig {
            api_key: "".to_string(),
            model: "gemini-embedding-001".to_string(),
            dimensions: 768,
        });
        let result = create_embedder_from_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn create_embedder_openai_without_config_errors() {
        let mut config = Config::default();
        config.provider.name = "openai".to_string();
        let result = create_embedder_from_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn create_embedder_ollama_with_config_works() {
        let mut config = Config::default();
        config.provider.name = "ollama".to_string();
        config.provider.ollama = Some(crate::config::schema::OllamaConfig {
            url: "http://localhost:11434".to_string(),
            model: "nomic-embed-text".to_string(),
        });
        let result = create_embedder_from_config(&config);
        assert!(result.is_ok());
        let embedder = result.unwrap();
        assert_eq!(embedder.provider_name(), "ollama");
    }
}
