//! `vectorcode benchmark` — run code-search quality benchmarks.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Args;

use crate::bench::corpus::{Corpus, GitCorpus, LocalCorpus};
use crate::bench::report::{self, OutputFormat};
use crate::bench::runner;
use crate::bench::schema::{CorpusConfig, CorpusSource, QuerySet};
use crate::embedder::mock::MockEmbedder;

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

        // Run benchmark
        let result = runner::run_benchmark(corpus.as_ref(), &queries, embedder.clone()).await?;

        // Output results
        match args.output {
            OutputFormat::Table => report::write_table(&result)?,
            OutputFormat::Json => {
                let json_path = project_path.join(format!("benchmark-{corpus_name}.json"));
                report::write_json(&result, &json_path)?;
                if !quiet {
                    eprintln!("Results written to: {}", json_path.display());
                }
            }
            OutputFormat::Baseline => {
                report::write_baseline(&result, project_path)?;
                if !quiet {
                    eprintln!(
                        "Baseline written to: {}/BASELINE.md",
                        project_path.display()
                    );
                }
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

    // Parse the corpus config
    let config: CorpusConfig = parse_corpus_config(name, config_value, project_path)?;

    match config.source {
        CorpusSource::Local { path } => {
            let full_path = if path.is_absolute() {
                path
            } else {
                project_path.join(path)
            };
            Ok(Box::new(LocalCorpus::new(
                name.to_string(),
                full_path,
                config.file_extensions,
            )))
        }
        CorpusSource::Git { url } => Ok(Box::new(GitCorpus::new(
            name.to_string(),
            url,
            config.sparse_paths,
            config.file_extensions,
        ))),
    }
}

/// Parse a corpus config from TOML value.
fn parse_corpus_config(
    name: &str,
    value: &toml::Value,
    _project_path: &std::path::Path,
) -> Result<CorpusConfig> {
    // Try to parse as a single repo
    if let Ok(repo) = value.clone().try_into::<crate::bench::schema::CorpusRepo>() {
        let source = if repo.url.starts_with("http") || repo.url.starts_with("git@") {
            CorpusSource::Git { url: repo.url }
        } else {
            CorpusSource::Local {
                path: PathBuf::from(repo.url),
            }
        };

        return Ok(CorpusConfig {
            name: name.to_string(),
            source,
            file_extensions: repo.file_extensions,
            sparse_paths: repo.sparse_paths,
        });
    }

    // Try to parse as multi-repo (for mini-corpus)
    if let Ok(multi) = value
        .clone()
        .try_into::<crate::bench::schema::CorpusEntry>()
    {
        match multi {
            crate::bench::schema::CorpusEntry::Multi { repos } => {
                // For multi-repo, use the first repo's config (simplified for now)
                // In Fase 1.1-1.2, we'll use LocalCorpus with test fixtures
                let first_repo = repos
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("Multi-repo corpus '{name}' has no repos"))?;

                let source = if first_repo.url.starts_with("http") {
                    CorpusSource::Git {
                        url: first_repo.url.clone(),
                    }
                } else {
                    CorpusSource::Local {
                        path: PathBuf::from(&first_repo.url),
                    }
                };

                return Ok(CorpusConfig {
                    name: name.to_string(),
                    source,
                    file_extensions: first_repo.file_extensions.clone(),
                    sparse_paths: first_repo.sparse_paths.clone(),
                });
            }
            crate::bench::schema::CorpusEntry::Single(repo) => {
                return parse_corpus_config(name, &toml::Value::try_from(repo)?, _project_path);
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
}
