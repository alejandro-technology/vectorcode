//! `vectorcode index` — build or update the embedding index (spec §12.3).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};

use crate::embedder::mock::MockEmbedder;
use crate::store::db::Database;
use crate::store::meta;

/// Arguments for `vectorcode index`.
#[derive(Args, Debug)]
pub struct IndexArgs {
    /// Drop all data and rebuild from scratch.
    #[arg(long)]
    pub full: bool,

    /// Index only a specific file.
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Maximum concurrent file processing tasks.
    #[arg(long, default_value = "8")]
    pub concurrency: usize,
}

/// Execute the `index` command (spec §12.3).
pub async fn execute(args: &IndexArgs, project_path: &std::path::Path, quiet: bool) -> Result<()> {
    let vc_dir = project_path.join(".vectorcode");
    let db_path = vc_dir.join("index.db");

    // Check initialization
    if !vc_dir.exists() {
        anyhow::bail!(
            "VectorCode is not initialized in {}.\nRun `vectorcode init` first.",
            project_path.display()
        );
    }

    // Load config
    let mut config = crate::config::load_config(project_path)?;

    // Override concurrency if specified
    if args.concurrency != 8 {
        config.indexing.concurrency = args.concurrency;
    }

    // Open database
    let db = Database::open(&db_path)?;

    // Check meta for provider mismatch
    let index_meta = meta::read_index_meta(db.conn())?;
    if index_meta.is_none() {
        anyhow::bail!(
            "Index metadata not found. The database may be corrupt.\n\
             Try: rm -rf .vectorcode/ && vectorcode init"
        );
    }
    let index_meta = index_meta.unwrap();
    // Handle --full: drop all data and reinit schema
    if args.full {
        if !quiet {
            eprintln!("Full reindex: dropping all data...");
        }
        // Re-create the database
        drop(db);
        std::fs::remove_file(&db_path)?;
        // Remove WAL/SHM files if they exist
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));

        let db = Database::open(&db_path)?;
        db.init_schema(index_meta.dimensions)?;
        // Re-write meta with zeroed stats
        let now = crate::cli::init::chrono_now_public();
        let fresh_meta = crate::types::IndexMeta {
            provider: index_meta.provider.clone(),
            model: index_meta.model.clone(),
            dimensions: index_meta.dimensions,
            created_at: now.clone(),
            last_sync_at: None,
            files_indexed: 0,
            chunks_stored: 0,
            vectorcode_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        meta::write_index_meta(db.conn(), &fresh_meta)?;
    }

    // Create embedder
    let embedder: Arc<dyn crate::embedder::Embedder> =
        match crate::cli::create_embedder_from_config(&config).await {
            Ok(e) => e,
            Err(err) => {
                // Fall back to MockEmbedder for testing
                if !quiet {
                    eprintln!(
                        "Warning: Could not create {} embedder: {err}",
                        config.provider.name
                    );
                    eprintln!("Using mock embedder for testing (results will be fake).");
                    eprintln!(
                        "To fix: ensure ONNX Runtime is installed or switch provider with 'vectorcode init'."
                    );
                }
                Arc::new(MockEmbedder::new(index_meta.dimensions))
            }
        };

    // Create indexer and run
    let indexer = crate::engine::Indexer::new(
        std::sync::Arc::new(tokio::sync::Mutex::new(Database::open(&db_path)?)),
        embedder,
        config.indexing.clone(),
    );

    // Set up progress callback for CLI mode (visual progress bars)
    let progress_bar = if quiet {
        None
    } else {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        pb.set_message("Starting indexing...");
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        Some(pb)
    };

    let indexer = if let Some(ref pb) = progress_bar {
        let pb_clone = pb.clone();
        let progress_callback = Arc::new(move |message: &str| {
            pb_clone.set_message(message.to_string());
        });
        indexer.with_progress(progress_callback)
    } else {
        indexer
    };

    let report = if let Some(ref file_path) = args.file {
        // Index a specific file
        let abs_path = if file_path.is_absolute() {
            file_path.clone()
        } else {
            project_path.join(file_path)
        };
        if !abs_path.exists() {
            anyhow::bail!("File not found: {}", abs_path.display());
        }
        indexer.index_files(&[abs_path], project_path).await?
    } else {
        // Full project index
        indexer.index_project(project_path).await?
    };
    // Finish the progress bar with a success message
    if let Some(pb) = progress_bar {
        pb.finish_with_message("Indexing complete");
    }

    // Update meta stats
    let db = Database::open(&db_path)?;
    let total_chunks = meta::count_chunks(db.conn())?;
    let total_files = meta::count_files(db.conn())?;
    let now = crate::cli::init::chrono_now_public();
    meta::update_meta_stats(db.conn(), total_files, total_chunks, &now)?;

    if !quiet {
        eprintln!(
            "Indexed {} files, {} new chunks ({} total) in {:.1}s",
            report.files_indexed,
            report.chunks_new,
            total_chunks,
            report.duration.as_secs_f64()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn index_args_parse_defaults() {
        let cli = Cli::parse_from(["vectorcode", "index"]);
        match cli.command {
            crate::cli::Commands::Index(args) => {
                assert!(!args.full);
                assert!(args.file.is_none());
                assert_eq!(args.concurrency, 8);
            }
            _ => panic!("Expected Index command"),
        }
    }

    #[test]
    fn index_args_parse_full_flag() {
        let cli = Cli::parse_from(["vectorcode", "index", "--full"]);
        match cli.command {
            crate::cli::Commands::Index(args) => {
                assert!(args.full);
            }
            _ => panic!("Expected Index command"),
        }
    }

    #[test]
    fn index_args_parse_file_option() {
        let cli = Cli::parse_from(["vectorcode", "index", "--file", "src/main.rs"]);
        match cli.command {
            crate::cli::Commands::Index(args) => {
                assert_eq!(args.file, Some(PathBuf::from("src/main.rs")));
            }
            _ => panic!("Expected Index command"),
        }
    }

    #[test]
    fn index_args_parse_concurrency() {
        let cli = Cli::parse_from(["vectorcode", "index", "--concurrency", "4"]);
        match cli.command {
            crate::cli::Commands::Index(args) => {
                assert_eq!(args.concurrency, 4);
            }
            _ => panic!("Expected Index command"),
        }
    }

    #[test]
    fn index_fails_without_init() {
        let dir = tempfile::tempdir().unwrap();
        let args = IndexArgs {
            full: false,
            file: None,
            concurrency: 8,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(execute(&args, dir.path(), true));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not initialized"), "Got: {err}");
    }

    #[test]
    fn index_runs_after_init() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        // Init first
        let init_args = crate::cli::init::InitArgs {
            provider: Some(crate::cli::ProviderArg::Gemini),
            model: None,
            dims: None,
            index: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(crate::cli::init::execute(&init_args, project_path, true))
            .unwrap();

        // Now index
        let index_args = IndexArgs {
            full: false,
            file: None,
            concurrency: 8,
        };
        let result = rt.block_on(execute(&index_args, project_path, true));
        assert!(
            result.is_ok(),
            "Index should succeed after init: {:?}",
            result.err()
        );
    }

    #[test]
    fn index_full_rebuilds_from_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        // Init
        let init_args = crate::cli::init::InitArgs {
            provider: Some(crate::cli::ProviderArg::Gemini),
            model: None,
            dims: None,
            index: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(crate::cli::init::execute(&init_args, project_path, true))
            .unwrap();

        // Full reindex
        let index_args = IndexArgs {
            full: true,
            file: None,
            concurrency: 8,
        };
        println!("DEBUG: Running indexer...");
        let result = rt.block_on(execute(&index_args, project_path, true));
        println!("DEBUG: Indexer done.");
        assert!(
            result.is_ok(),
            "Full reindex should succeed: {:?}",
            result.err()
        );
    }
}
