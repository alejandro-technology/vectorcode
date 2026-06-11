//! `vectorcode status` — show index status and health (spec §12.5).

use anyhow::Result;
use clap::Args;

use crate::store::db::Database;
use crate::store::meta;

/// Arguments for `vectorcode status`.
#[derive(Args, Debug)]
pub struct StatusArgs {}

/// Execute the `status` command.
///
/// Reads meta table and prints formatted status including:
/// - Provider, model, dimensions
/// - Version
/// - Files indexed, chunks stored
/// - Last sync time
pub fn execute(args: &StatusArgs, project_path: &std::path::Path) -> Result<()> {
    let _ = args; // No options currently

    let vc_dir = project_path.join(".vectorcode");
    let db_path = vc_dir.join("index.db");

    // Check initialization
    if !vc_dir.exists() {
        println!(
            "VectorCode is not initialized in {}.",
            project_path.display()
        );
        println!("Run `vectorcode init` to set up.");
        return Ok(());
    }

    if !db_path.exists() {
        println!(
            "VectorCode directory exists but index.db is missing in {}.",
            project_path.display()
        );
        println!("Run `vectorcode init` to reinitialize.");
        return Ok(());
    }

    // Open database and read meta
    let db = Database::open(&db_path)?;
    let index_meta = meta::read_index_meta(db.conn())?;

    match index_meta {
        Some(meta) => {
            println!("VectorCode Status");
            println!("=================");
            println!("  Project:       {}", project_path.display());
            println!("  Provider:      {}", meta.provider);
            println!("  Model:         {}", meta.model);
            println!("  Dimensions:    {}", meta.dimensions);
            println!("  Version:       {}", meta.vectorcode_version);
            println!("  Created:       {}", meta.created_at);
            println!("  Files indexed: {}", meta.files_indexed);
            println!("  Chunks stored: {}", meta.chunks_stored);
            match meta.last_sync_at {
                Some(ref sync) => println!("  Last sync:     {sync}"),
                None => println!("  Last sync:     never"),
            }

            // Show actual counts from DB
            let actual_chunks = meta::count_chunks(db.conn())?;
            let actual_files = meta::count_files(db.conn())?;
            if actual_chunks != meta.chunks_stored || actual_files != meta.files_indexed {
                println!();
                println!("  ⚠ Meta stats may be out of date:");
                println!("    Actual files:   {actual_files}");
                println!("    Actual chunks:  {actual_chunks}");
                println!("    Run `vectorcode index` to update.");
            }
        }
        None => {
            println!("VectorCode directory exists but index metadata is missing.");
            println!("Run `vectorcode init` to reinitialize.");
        }
    }

    Ok(())
}

/// Format the status output as a string (pure function for testing).
pub fn format_status(
    provider: &str,
    model: &str,
    dimensions: u32,
    version: &str,
    files_indexed: u32,
    chunks_stored: u32,
    last_sync: Option<&str>,
) -> String {
    let sync_display = last_sync.unwrap_or("never");
    format!(
        "Provider: {provider}\n\
         Model: {model}\n\
         Dimensions: {dimensions}\n\
         Version: {version}\n\
         Files: {files_indexed}\n\
         Chunks: {chunks_stored}\n\
         Last sync: {sync_display}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn status_args_parse() {
        let cli = Cli::parse_from(["vectorcode", "status"]);
        assert!(matches!(cli.command, crate::cli::Commands::Status(_)));
    }

    #[test]
    fn format_status_with_all_fields() {
        let output = format_status(
            "onnx",
            "all-MiniLM-L6-v2",
            384,
            "0.1.0",
            42,
            200,
            Some("2026-06-10T20:05:00Z"),
        );
        assert!(output.contains("onnx"), "Got: {output}");
        assert!(output.contains("all-MiniLM-L6-v2"), "Got: {output}");
        assert!(output.contains("384"), "Got: {output}");
        assert!(output.contains("42"), "Got: {output}");
        assert!(output.contains("200"), "Got: {output}");
        assert!(output.contains("2026-06-10T20:05:00Z"), "Got: {output}");
    }

    #[test]
    fn format_status_without_last_sync() {
        let output = format_status("gemini", "gemini-embedding-001", 768, "0.1.0", 0, 0, None);
        assert!(output.contains("never"), "Got: {output}");
        assert!(output.contains("gemini"), "Got: {output}");
    }

    #[test]
    fn status_shows_not_initialized_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute(&StatusArgs {}, dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn status_shows_info_after_init() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        // Init first
        let init_args = crate::cli::init::InitArgs {
            provider: Some(crate::cli::ProviderArg::Ollama),
            model: None,
            dims: None,
            index: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(crate::cli::init::execute(&init_args, project_path, true))
            .unwrap();

        // Status should work
        let result = execute(&StatusArgs {}, project_path);
        assert!(result.is_ok(), "Status should succeed: {:?}", result.err());
    }

    #[test]
    fn status_detects_missing_db() {
        let dir = tempfile::tempdir().unwrap();
        let vc_dir = dir.path().join(".vectorcode");
        std::fs::create_dir_all(&vc_dir).unwrap();
        // No index.db

        let result = execute(&StatusArgs {}, dir.path());
        assert!(result.is_ok());
        // Should print message about missing db (we can't easily capture stdout in unit tests,
        // but we verify it doesn't error)
    }
}
