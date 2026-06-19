//! `vectorcode benchmark` — run code-search quality benchmarks.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Args;

use crate::bench::corpus::{Corpus, GitCorpus, LocalCorpus, MultiCorpus};
use crate::bench::report::{self, OutputFormat};
use crate::bench::runner;
use crate::bench::schema::{CorpusSource, QuerySet};
use crate::embedder::mock::MockEmbedder;
use crate::engine::SearchMode;

/// Corpus selection argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorpusArg {
    Mini,
    Vscode,
    All,
    Custom(String),
}

impl std::str::FromStr for CorpusArg {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mini" => Ok(CorpusArg::Mini),
            "vscode" => Ok(CorpusArg::Vscode),
            "all" => Ok(CorpusArg::All),
            other => Ok(CorpusArg::Custom(other.to_string())),
        }
    }
}

impl std::fmt::Display for CorpusArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CorpusArg::Mini => write!(f, "mini"),
            CorpusArg::Vscode => write!(f, "vscode"),
            CorpusArg::All => write!(f, "all"),
            CorpusArg::Custom(s) => write!(f, "{s}"),
        }
    }
}

/// Arguments for `vectorcode benchmark`.
#[derive(Args, Debug)]
pub struct BenchmarkArgs {
    /// Corpus to benchmark: mini, vscode, all, or a custom corpus name.
    #[arg(long, default_value = "mini")]
    pub corpus: CorpusArg,

    /// Output format: table, json, or baseline.
    #[arg(long, default_value = "table")]
    pub output: OutputFormat,

    /// Search mode: dense, sparse, hybrid, hybrid-rerank, or all (runs all modes).
    #[arg(long, default_value = "dense")]
    pub mode: String,

    /// Path to corpus configuration file (default: benchmarks/corpus.toml).
    #[arg(long)]
    pub corpus_config: Option<PathBuf>,

    /// Path to queries file (default: benchmarks/queries/<corpus>.toml).
    #[arg(long)]
    pub queries: Option<PathBuf>,
}

/// Execute the `benchmark` command.
pub async fn execute(
    args: &BenchmarkArgs,
    project_path: &std::path::Path,
    quiet: bool,
) -> Result<()> {
    // Determine paths
    let corpus_config_path = args
        .corpus_config
        .clone()
        .unwrap_or_else(|| project_path.join("benchmarks/corpus.toml"));

    let queries_path = args
        .queries
        .clone()
        .unwrap_or_else(|| project_path.join(format!("benchmarks/queries/{}.toml", args.corpus)));

    // Load corpus config
    if !corpus_config_path.exists() {
        anyhow::bail!(
            "Corpus config not found: {}\nRun from the project root or specify --corpus-config",
            corpus_config_path.display()
        );
    }

    let corpus_config_str = std::fs::read_to_string(&corpus_config_path)?;
    let corpus_configs: std::collections::HashMap<String, toml::Value> =
        toml::from_str(&corpus_config_str)?;

    // Select corpus(es) to run
    let corpora_to_run: Vec<String> = match &args.corpus {
        CorpusArg::Mini => vec!["mini".to_string()],
        CorpusArg::Vscode => vec!["vscode".to_string()],
        CorpusArg::All => corpus_configs.keys().cloned().collect(),
        CorpusArg::Custom(name) => vec![name.clone()],
    };

    // Create embedder (try real, fall back to mock)
    let embedder: Arc<dyn crate::embedder::Embedder> =
        match crate::cli::create_embedder_from_config(
            &crate::config::load_config(project_path).unwrap_or_default(),
        )
        .await
        {
            Ok(e) => e,
            Err(err) => {
                if !quiet {
                    eprintln!("Warning: Could not create embedder: {err}");
                    eprintln!("Using mock embedder (results will be meaningless).");
                }
                Arc::new(MockEmbedder::new(384))
            }
        };

    // Parse search mode(s)
    let modes = parse_modes(&args.mode)?;

    // Run benchmark for each corpus
    for corpus_name in &corpora_to_run {
        if !quiet {
            eprintln!("Running benchmark for corpus: {corpus_name}");
        }

        // Build corpus adapter
        let corpus: Box<dyn Corpus> = build_corpus(corpus_name, &corpus_configs, project_path)?;

        // Load queries
        if !queries_path.exists() {
            anyhow::bail!(
                "Queries file not found: {}\nCreate it or specify --queries",
                queries_path.display()
            );
        }

        let queries_str = std::fs::read_to_string(&queries_path)?;
        let queries: QuerySet = toml::from_str(&queries_str)?;

        // Run benchmark(s)
        if modes.len() == 1 {
            let result =
                runner::run_benchmark(corpus.as_ref(), &queries, embedder.clone(), modes[0])
                    .await?;

            // Output results
            output_single_result(&result, &args.output, corpus_name, project_path, quiet)?;
        } else {
            let results = runner::run_multi_mode_benchmark(
                corpus.as_ref(),
                &queries,
                embedder.clone(),
                &modes,
            )
            .await?;

            // Output multi-mode results
            output_multi_results(&results, &args.output, corpus_name, project_path, quiet)?;
        }
    }

    Ok(())
}

/// Parse mode string into a list of SearchMode values.
///
/// "all" expands to [Dense, Sparse, Hybrid, HybridRerank].
/// Individual mode names parse to a single-element list.
fn parse_modes(mode_str: &str) -> Result<Vec<SearchMode>> {
    if mode_str == "all" {
        return Ok(vec![
            SearchMode::Dense,
            SearchMode::Sparse,
            SearchMode::Hybrid,
            SearchMode::HybridRerank,
        ]);
    }

    let mode: SearchMode = mode_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;
    Ok(vec![mode])
}

/// Output a single benchmark result in the requested format.
fn output_single_result(
    result: &crate::bench::schema::BenchmarkResult,
    output: &OutputFormat,
    corpus_name: &str,
    project_path: &std::path::Path,
    quiet: bool,
) -> Result<()> {
    match output {
        OutputFormat::Table => report::write_table(result)?,
        OutputFormat::Json => {
            let json_path = project_path.join(format!(
                "benchmark-{corpus_name}-{}.json",
                result.search_mode
            ));
            report::write_json(result, &json_path)?;
            if !quiet {
                eprintln!("Results written to: {}", json_path.display());
            }
        }
        OutputFormat::Baseline => {
            report::write_baseline(result, project_path)?;
            if !quiet {
                eprintln!(
                    "Baseline written to: {}/BASELINE.md",
                    project_path.display()
                );
            }
        }
    }
    Ok(())
}

/// Output multi-mode benchmark results in the requested format.
fn output_multi_results(
    results: &[crate::bench::schema::BenchmarkResult],
    output: &OutputFormat,
    corpus_name: &str,
    project_path: &std::path::Path,
    quiet: bool,
) -> Result<()> {
    match output {
        OutputFormat::Table => report::write_multi_mode_table(results)?,
        OutputFormat::Json => {
            let json_path = project_path.join(format!("benchmark-{corpus_name}-multi.json"));
            let json = serde_json::to_string_pretty(results)?;
            std::fs::write(&json_path, json)?;
            if !quiet {
                eprintln!("Results written to: {}", json_path.display());
            }
        }
        OutputFormat::Baseline => {
            // Write each mode as a separate baseline subdirectory
            for result in results {
                let mode_dir = project_path.join(format!("baseline-{}", result.search_mode));
                report::write_baseline(result, &mode_dir)?;
            }
            if !quiet {
                eprintln!(
                    "Baselines written to: {}/baseline-<mode>/BASELINE.md",
                    project_path.display()
                );
            }
        }
    }
    Ok(())
}

/// Build a corpus adapter from config.
fn build_corpus(
    name: &str,
    configs: &std::collections::HashMap<String, toml::Value>,
    project_path: &std::path::Path,
) -> Result<Box<dyn Corpus>> {
    let config_value = configs
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Corpus '{name}' not found in config"))?;

    // Try to parse as multi-repo first (for mini-corpus)
    if let Ok(multi) = config_value
        .clone()
        .try_into::<crate::bench::schema::CorpusEntry>()
    {
        match multi {
            crate::bench::schema::CorpusEntry::Multi { repos } => {
                if repos.is_empty() {
                    anyhow::bail!("Multi-repo corpus '{name}' has no repos");
                }

                let mut corpora: Vec<Box<dyn Corpus>> = Vec::new();
                for (idx, repo) in repos.iter().enumerate() {
                    let corpus_name = format!("{name}_{idx}");
                    let corpus: Box<dyn Corpus> =
                        if repo.url.starts_with("http") || repo.url.starts_with("git@") {
                            Box::new(GitCorpus::new(
                                corpus_name,
                                repo.url.clone(),
                                repo.sparse_paths.clone(),
                                repo.file_extensions.clone(),
                            ))
                        } else {
                            let full_path = if std::path::Path::new(&repo.url).is_absolute() {
                                std::path::PathBuf::from(&repo.url)
                            } else {
                                project_path.join(&repo.url)
                            };
                            Box::new(LocalCorpus::new(
                                corpus_name,
                                full_path,
                                repo.file_extensions.clone(),
                            ))
                        };
                    corpora.push(corpus);
                }

                return Ok(Box::new(MultiCorpus::new(name.to_string(), corpora)));
            }
            crate::bench::schema::CorpusEntry::Single(repo) => {
                let source = if repo.url.starts_with("http") || repo.url.starts_with("git@") {
                    CorpusSource::Git { url: repo.url }
                } else {
                    let full_path = if std::path::Path::new(&repo.url).is_absolute() {
                        std::path::PathBuf::from(&repo.url)
                    } else {
                        project_path.join(&repo.url)
                    };
                    CorpusSource::Local { path: full_path }
                };

                return match source {
                    CorpusSource::Local { path } => Ok(Box::new(LocalCorpus::new(
                        name.to_string(),
                        path,
                        repo.file_extensions,
                    ))),
                    CorpusSource::Git { url } => Ok(Box::new(GitCorpus::new(
                        name.to_string(),
                        url,
                        repo.sparse_paths,
                        repo.file_extensions,
                    ))),
                };
            }
        }
    }

    anyhow::bail!("Failed to parse corpus config for '{name}'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn benchmark_args_parse_defaults() {
        let cli = crate::cli::Cli::parse_from(["vectorcode", "benchmark"]);
        match cli.command {
            crate::cli::Commands::Benchmark(args) => {
                assert_eq!(args.corpus, CorpusArg::Mini);
                assert_eq!(args.output, OutputFormat::Table);
                assert_eq!(args.mode, "dense");
            }
            _ => panic!("Expected Benchmark command"),
        }
    }

    #[test]
    fn benchmark_args_parse_corpus() {
        let cli = crate::cli::Cli::parse_from(["vectorcode", "benchmark", "--corpus", "vscode"]);
        match cli.command {
            crate::cli::Commands::Benchmark(args) => {
                assert_eq!(args.corpus, CorpusArg::Vscode);
            }
            _ => panic!("Expected Benchmark command"),
        }
    }

    #[test]
    fn benchmark_args_parse_output() {
        let cli = crate::cli::Cli::parse_from(["vectorcode", "benchmark", "--output", "json"]);
        match cli.command {
            crate::cli::Commands::Benchmark(args) => {
                assert_eq!(args.output, OutputFormat::Json);
            }
            _ => panic!("Expected Benchmark command"),
        }
    }

    #[test]
    fn corpus_arg_from_str() {
        assert_eq!("mini".parse::<CorpusArg>().unwrap(), CorpusArg::Mini);
        assert_eq!("vscode".parse::<CorpusArg>().unwrap(), CorpusArg::Vscode);
        assert_eq!("all".parse::<CorpusArg>().unwrap(), CorpusArg::All);
        assert_eq!(
            "custom".parse::<CorpusArg>().unwrap(),
            CorpusArg::Custom("custom".to_string())
        );
    }

    #[test]
    fn benchmark_args_parse_mode() {
        let cli = crate::cli::Cli::parse_from(["vectorcode", "benchmark", "--mode", "hybrid"]);
        match cli.command {
            crate::cli::Commands::Benchmark(args) => {
                assert_eq!(args.mode, "hybrid");
            }
            _ => panic!("Expected Benchmark command"),
        }
    }

    #[test]
    fn parse_modes_single() {
        let modes = parse_modes("dense").unwrap();
        assert_eq!(modes, vec![SearchMode::Dense]);

        let modes = parse_modes("hybrid-rerank").unwrap();
        assert_eq!(modes, vec![SearchMode::HybridRerank]);
    }

    #[test]
    fn parse_modes_all() {
        let modes = parse_modes("all").unwrap();
        assert_eq!(modes.len(), 4);
        assert_eq!(modes[0], SearchMode::Dense);
        assert_eq!(modes[1], SearchMode::Sparse);
        assert_eq!(modes[2], SearchMode::Hybrid);
        assert_eq!(modes[3], SearchMode::HybridRerank);
    }

    #[test]
    fn parse_modes_invalid_returns_error() {
        assert!(parse_modes("invalid-mode").is_err());
    }
}
