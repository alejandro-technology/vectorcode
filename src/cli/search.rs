//! `vectorcode search` — semantic search from the command line (spec §12.4).

use std::sync::Arc;

use anyhow::Result;
use clap::Args;

use crate::embedder::mock::MockEmbedder;
use crate::engine::searcher::SearchOptions;
use crate::store::db::Database;
use crate::store::meta;
use crate::types::SearchResult;

/// Arguments for `vectorcode search`.
#[derive(Args, Debug)]
pub struct SearchArgs {
    /// The search query (natural language).
    pub query: String,

    /// Maximum number of results.
    #[arg(long, default_value = "10")]
    pub limit: usize,

    /// Minimum similarity score (0.0–1.0).
    #[arg(long, default_value = "0.3")]
    pub threshold: f32,

    /// Filter by programming language.
    #[arg(long)]
    pub language: Option<String>,

    /// Filter by file path prefix.
    #[arg(long)]
    pub path: Option<String>,

    /// Output results as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Execute the `search` command (spec §12.4).
pub async fn execute(args: &SearchArgs, project_path: &std::path::Path, quiet: bool) -> Result<()> {
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
    let config = crate::config::load_config(project_path)?;

    // Open database and check meta
    let db = Database::open(&db_path)?;
    let index_meta = meta::read_index_meta(db.conn())?
        .ok_or_else(|| anyhow::anyhow!("Index metadata not found. Run `vectorcode init` first."))?;

    // Create embedder
    let embedder: Arc<dyn crate::embedder::Embedder> =
        match crate::cli::create_embedder_from_config(&config) {
            Ok(e) => e,
            Err(_) => {
                if !quiet {
                    eprintln!(
                        "Warning: Could not create {} embedder, using mock embedder",
                        config.provider.name
                    );
                }
                Arc::new(MockEmbedder::new(index_meta.dimensions))
            }
        };

    // Create searcher
    let searcher =
        crate::engine::Searcher::new(Database::open(&db_path)?, embedder, config.search.clone());

    // Build search options
    let options = SearchOptions {
        limit: args.limit,
        threshold: args.threshold,
        language: args.language.clone(),
        path: args.path.clone(),
    };

    // Execute search
    let results = searcher.search(&args.query, options).await?;

    // Output results
    if args.json {
        output_json(&results)?;
    } else {
        output_text(&results, &args.query, quiet);
    }

    Ok(())
}

/// Format results as JSON to stdout.
fn output_json(results: &[SearchResult]) -> Result<()> {
    let json = serde_json::to_string_pretty(results)?;
    println!("{json}");
    Ok(())
}

/// Format results as human-readable text to stdout.
fn output_text(results: &[SearchResult], query: &str, quiet: bool) {
    if results.is_empty() {
        if !quiet {
            eprintln!("No results found for \"{query}\"");
        }
        return;
    }

    if !quiet {
        eprintln!(
            "Found {} results for \"{query}\" (threshold: {:.2})",
            results.len(),
            results.first().map_or(0.0, |r| r.score)
        );
        eprintln!();
    }

    for (i, result) in results.iter().enumerate() {
        let symbol = result.symbol.as_deref().unwrap_or("<anonymous>");
        println!(
            "[{}] {} ({}:{}, score: {:.3})",
            i + 1,
            result.file_path,
            symbol,
            result.start_line,
            result.score,
        );
        // Print first few lines of content
        let preview: String = result
            .content
            .lines()
            .take(5)
            .collect::<Vec<_>>()
            .join("\n");
        println!("{preview}");
        if result.content.lines().count() > 5 {
            println!("  ... ({} total lines)", result.content.lines().count());
        }
        println!();
    }
}

/// Format a single result for text output (pure function for testing).
pub fn format_result_brief(result: &SearchResult) -> String {
    let symbol = result.symbol.as_deref().unwrap_or("<anonymous>");
    format!(
        "{} ({}:{}, score: {:.3})",
        result.file_path, symbol, result.start_line, result.score,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn search_args_parse_query() {
        let cli = Cli::parse_from(["vectorcode", "search", "payment retry"]);
        match cli.command {
            crate::cli::Commands::Search(args) => {
                assert_eq!(args.query, "payment retry");
                assert_eq!(args.limit, 10);
                assert!((args.threshold - 0.3).abs() < f32::EPSILON);
                assert!(args.language.is_none());
                assert!(args.path.is_none());
                assert!(!args.json);
            }
            _ => panic!("Expected Search command"),
        }
    }

    #[test]
    fn search_args_parse_all_options() {
        let cli = Cli::parse_from([
            "vectorcode",
            "search",
            "auth flow",
            "--limit",
            "5",
            "--threshold",
            "0.5",
            "--language",
            "rust",
            "--path",
            "src/auth",
            "--json",
        ]);
        match cli.command {
            crate::cli::Commands::Search(args) => {
                assert_eq!(args.query, "auth flow");
                assert_eq!(args.limit, 5);
                assert!((args.threshold - 0.5).abs() < f32::EPSILON);
                assert_eq!(args.language, Some("rust".to_string()));
                assert_eq!(args.path, Some("src/auth".to_string()));
                assert!(args.json);
            }
            _ => panic!("Expected Search command"),
        }
    }

    #[test]
    fn format_result_brief_with_symbol() {
        let result = SearchResult {
            file_path: "src/auth.rs".to_string(),
            start_line: 42,
            end_line: 80,
            symbol: Some("authenticate".to_string()),
            kind: "function_item".to_string(),
            language: "rust".to_string(),
            parent_context: None,
            content: "fn authenticate() {}".to_string(),
            score: 0.87,
        };
        let brief = format_result_brief(&result);
        assert!(brief.contains("src/auth.rs"), "Got: {brief}");
        assert!(brief.contains("authenticate"), "Got: {brief}");
        assert!(brief.contains("42"), "Got: {brief}");
        assert!(brief.contains("0.870"), "Got: {brief}");
    }

    #[test]
    fn format_result_brief_without_symbol() {
        let result = SearchResult {
            file_path: "lib/utils.py".to_string(),
            start_line: 10,
            end_line: 20,
            symbol: None,
            kind: "function_definition".to_string(),
            language: "python".to_string(),
            parent_context: None,
            content: "def helper(): pass".to_string(),
            score: 0.45,
        };
        let brief = format_result_brief(&result);
        assert!(brief.contains("<anonymous>"), "Got: {brief}");
        assert!(brief.contains("0.450"), "Got: {brief}");
    }

    #[test]
    fn search_fails_without_init() {
        let dir = tempfile::tempdir().unwrap();
        let args = SearchArgs {
            query: "test".to_string(),
            limit: 10,
            threshold: 0.3,
            language: None,
            path: None,
            json: false,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(execute(&args, dir.path(), true));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not initialized"), "Got: {err}");
    }

    #[test]
    fn search_runs_after_init_with_empty_results() {
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

        // Search (should return empty results, not error)
        let search_args = SearchArgs {
            query: "test query".to_string(),
            limit: 10,
            threshold: 0.3,
            language: None,
            path: None,
            json: false,
        };
        let result = rt.block_on(execute(&search_args, project_path, true));
        assert!(result.is_ok(), "Search should succeed: {:?}", result.err());
    }

    #[test]
    fn search_json_output_format() {
        let results = vec![SearchResult {
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 10,
            symbol: Some("test_fn".to_string()),
            kind: "function_item".to_string(),
            language: "rust".to_string(),
            parent_context: None,
            content: "fn test_fn() {}".to_string(),
            score: 0.95,
        }];
        // Should not panic
        output_json(&results).unwrap();
    }
}
