//! `vectorcode bench-store` — run a parameterized store benchmark against a
//! real corpus (vscode by default) and report the 4 spec axes + SLO verdict.
//!
//! Spec: R2 (Parameterized Benchmark Harness), R3 (Hard Indexing SLO ≤6min).
//!
//! This subcommand is the operational entrypoint for the harness in
//! `src/bench/store_bench.rs::run_store_benchmark`. Without it, the harness
//! was only callable from unit tests with a 3-file corpus — a gap that left
//! spec R2 and R3 unverifiable from the CLI.
//!
//! ## Honesty rule
//!
//! The `--backend lancedb` selection uses the feature-gated `LanceStore`,
//! which is an in-memory shim. The numbers it produces do NOT represent real
//! LanceDB performance. Callers who need a real comparison must wire up the
//! real LanceDB integration first; this CLI refuses to lie about it.
//!
//! ## Output
//!
//! Default: TOML to stdout (machine-readable, captures every axis field).
//! Use `--output json` for the same payload as JSON.
//!
//! ## Comparison gate
//!
//! Pass `--compare <baseline.json>` to gate the run against a committed
//! store baseline. Exit codes: 0 = pass, 2 = regression, 1 = error.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::bench::corpus::Corpus;
use crate::bench::report::{write_delta_json, write_delta_table};
use crate::bench::store_bench::run_store_benchmark;
use crate::bench::verdict::{compare_to_baseline, load_store_baseline, validate_baseline};
use crate::embedder::mock::{MockDeterministicEmbedder, MockEmbedder};
use crate::store::store::StoreFactory;

/// CLI argument for the store backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    /// The production sqlite-vec backed store. Default.
    SqliteVec,
    /// Feature-gated LanceDB backend. Currently an in-memory shim; numbers
    /// are NOT representative of real LanceDB performance.
    Lancedb,
}

impl BackendArg {
    /// Convert to the string used in `StoreFactory::backend_name()`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SqliteVec => "sqlite-vec",
            Self::Lancedb => "lancedb",
        }
    }
}

impl std::fmt::Display for BackendArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for BackendArg {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sqlite-vec" | "sqlite_vec" | "sqlite" => Ok(Self::SqliteVec),
            "lancedb" | "lance" => Ok(Self::Lancedb),
            other => Err(format!(
                "Unknown backend '{other}'. Valid options: sqlite-vec, lancedb"
            )),
        }
    }
}

/// Output format for the store benchmark report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreBenchOutput {
    /// Pretty-printed TOML to stdout.
    #[default]
    Toml,
    /// Pretty-printed JSON to stdout.
    Json,
}

/// Arguments for `vectorcode bench-store`.
#[derive(Args, Debug)]
pub struct BenchStoreArgs {
    /// Corpus to benchmark: matches a key in `benchmarks/corpus.toml`.
    #[arg(long, default_value = "vscode")]
    pub corpus: String,

    /// Path to corpus configuration file.
    #[arg(long)]
    pub corpus_config: Option<PathBuf>,

    /// Backend to measure.
    #[arg(long, value_enum, default_value_t = BackendArg::SqliteVec)]
    pub backend: BackendArg,

    /// Indexing SLO in seconds. Default 360 (6min) per spec R3.
    #[arg(long, default_value_t = 360)]
    pub slo_secs: u32,

    /// Output format.
    #[arg(long, value_enum, default_value_t = StoreBenchOutput::Toml)]
    pub output: StoreBenchOutput,

    /// Force the MockEmbedder (deterministic, 384d) instead of the configured
    /// provider. Required to measure the STORE SLO in isolation, because the
    /// configured embedder (e.g. ollama) often dominates the wall-clock and
    /// confounds the store's contribution to the ≤6min SLO.
    #[arg(long)]
    pub mock_embedder: bool,

    /// Number of latency queries to sample. Default 100. Pass 0 to skip the
    /// query phase entirely (useful when validating the indexing SLO only —
    /// the query phase is O(N) and can take 20+ min on 15K-file corpora).
    #[arg(long, default_value_t = 100)]
    pub query_sample: usize,

    /// Path to a store baseline JSON file. When set, the run is compared
    /// against the baseline and the process exits 0 (pass), 2 (regression),
    /// or 1 (error). A `delta-report.json` artifact is written next to the
    /// baseline file.
    #[arg(long)]
    pub compare: Option<PathBuf>,
}

/// Exit code returned when the SLO is violated. Lets CI/scripts detect
/// regressions without parsing the report payload.
pub const EXIT_SLO_VIOLATION: i32 = 75;

/// Execute the `bench-store` command.
///
/// Wires the CLI args into `run_store_benchmark` and prints the resulting
/// `StoreMetricsReport` in the requested format. Exits with non-zero status
/// if the SLO is violated.
pub async fn execute(
    args: &BenchStoreArgs,
    project_path: &std::path::Path,
    quiet: bool,
) -> Result<()> {
    // 1. Resolve corpus config
    let corpus_config_path = args
        .corpus_config
        .clone()
        .unwrap_or_else(|| project_path.join("benchmarks/corpus.toml"));

    if !corpus_config_path.exists() {
        anyhow::bail!(
            "Corpus config not found: {}\nRun from the project root or specify --corpus-config",
            corpus_config_path.display()
        );
    }

    let corpus_config_str = std::fs::read_to_string(&corpus_config_path)?;
    let corpus_configs: std::collections::HashMap<String, toml::Value> =
        toml::from_str(&corpus_config_str)?;

    // 2. Build corpus via the shared helper (handles single-repo + multi-repo).
    let corpus: Box<dyn Corpus> =
        crate::cli::benchmark::build_corpus(&args.corpus, &corpus_configs, project_path)?;

    // 3. Build the StoreFactory. LanceDB is feature-gated.
    let factory: Box<dyn StoreFactory> = match args.backend {
        BackendArg::SqliteVec => Box::new(crate::store::sqlite::SqliteStoreFactory),
        BackendArg::Lancedb => {
            #[cfg(feature = "lancedb-store")]
            {
                Box::new(crate::store::lancedb::LanceStoreFactory)
            }
            #[cfg(not(feature = "lancedb-store"))]
            {
                anyhow::bail!("Backend 'lancedb' requires building with --features lancedb-store");
            }
        }
    };

    if args.backend == BackendArg::Lancedb && !quiet {
        eprintln!(
            "WARNING: --backend lancedb uses the in-memory LanceStore shim. \
             The numbers it produces are NOT representative of real LanceDB \
             performance. See ADR 0001 (Re-evaluation Conditions) for the \
             required real-LanceDB wiring before drawing conclusions."
        );
    }

    // 4. Build the embedder. The SLO is a STORE SLO (R3), so the embedder
    //    must not be the bottleneck. Three modes:
    //    a) --mock-embedder: forced MockDeterministicEmbedder (384d, deterministic
    //       variant whose `provider_name` is "mock-deterministic"). The right
    //       default for measuring the store in isolation and for the
    //       `--compare` gate.
    //    b) Configured provider: when the user explicitly wants the real
    //       configured embedder in the loop. Often too slow (e.g. ollama
    //       over HTTP) to validate a 6min SLO on a 15K-file corpus.
    //    c) Fallback MockEmbedder: when no real embedder can be built.
    //       Forbidden when `--compare` is set: the comparison requires
    //       deterministic embeddings and the fallback MockEmbedder would
    //       produce different vectors run-to-run only if the hashing
    //       implementation changes — but we keep the rule strict.
    let embedder: Arc<dyn crate::embedder::Embedder> = if args.mock_embedder {
        if !quiet {
            eprintln!("Using MockDeterministicEmbedder (forced via --mock-embedder).");
        }
        Arc::new(MockDeterministicEmbedder::new(384))
    } else {
        match crate::cli::create_embedder_from_config(
            &crate::config::load_config(project_path).unwrap_or_default(),
        )
        .await
        {
            Ok(e) => e,
            Err(err) => {
                if args.compare.is_some() {
                    anyhow::bail!(
                        "comparison requires deterministic embeddings; \
                         use --mock-embedder or configure a provider ({err})"
                    );
                }
                if !quiet {
                    eprintln!("Warning: Could not create embedder: {err}");
                    eprintln!("Using mock embedder (results will not reflect real semantics).");
                }
                Arc::new(MockEmbedder::new(384))
            }
        }
    };

    if !quiet {
        eprintln!(
            "Running store benchmark: backend={}, corpus={}, slo={}s, embedder={}/{}",
            args.backend,
            corpus.name(),
            args.slo_secs,
            embedder.provider_name(),
            embedder.model_name(),
        );
    }

    // 4b. If --compare is set, validate the baseline up front so we fail
    //     fast before paying the indexing cost.
    if let Some(compare_path) = &args.compare {
        let header = validate_baseline(compare_path).map_err(|e| anyhow::anyhow!("{e}"))?;
        if !quiet {
            eprintln!(
                "Comparing to baseline: version={} corpus={} embedder={}",
                header.version, header.corpus, header.embedder
            );
        }
    }

    // 5. Run the harness.
    let report = run_store_benchmark(
        factory.as_ref(),
        corpus.as_ref(),
        embedder,
        args.slo_secs,
        args.query_sample,
    )
    .await?;

    // 6. Print the report.
    match args.output {
        StoreBenchOutput::Toml => {
            let s = toml::to_string_pretty(&report)?;
            println!("{s}");
        }
        StoreBenchOutput::Json => {
            let s = serde_json::to_string_pretty(&report)?;
            println!("{s}");
        }
    }

    // 7. SLO verdict.
    if !report.slo_passed {
        eprintln!(
            "\nSLO VIOLATION: indexing took {:.1}s (limit {}s) for backend '{}' on corpus '{}'.",
            report.indexing_secs, report.slo_limit_secs, report.backend, report.corpus,
        );
        // If --compare is set, the SLO violation is itself a regression;
        // surface it as exit 2 so the gate is honored uniformly.
        if args.compare.is_some() {
            std::process::exit(2);
        }
        std::process::exit(EXIT_SLO_VIOLATION);
    } else if !quiet {
        eprintln!(
            "\nSLO PASS: indexing took {:.1}s (limit {}s) for backend '{}' on corpus '{}'.",
            report.indexing_secs, report.slo_limit_secs, report.backend, report.corpus,
        );
    }

    // 8. If --compare is set, run the store comparison and exit 0/2.
    if let Some(compare_path) = &args.compare {
        return run_store_comparison(&report, compare_path, quiet);
    }

    Ok(())
}

/// Run the store-level comparison against a baseline file and emit the
/// delta report. Returns `Ok(())` after calling `std::process::exit` with
/// the appropriate code (0 = pass, 2 = regression, 1 = error already
/// surfaced via the error return path).
fn run_store_comparison(
    current: &crate::bench::schema::StoreMetricsReport,
    baseline_path: &std::path::Path,
    quiet: bool,
) -> Result<()> {
    let baseline = load_store_baseline(baseline_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let baseline_report = baseline.to_store_metrics_report();

    let comparison = compare_to_baseline(current, &baseline_report);

    // Emit the table to stdout.
    let mut buf: Vec<u8> = Vec::new();
    if write_delta_table(&comparison, &mut buf).is_err() {
        anyhow::bail!("failed to render delta table");
    }
    print!("{}", String::from_utf8_lossy(&buf));

    // Write the JSON artifact next to the baseline file.
    let delta_path = baseline_path.with_file_name("delta-report.json");
    let mut json_buf: Vec<u8> = Vec::new();
    if write_delta_json(&comparison, &mut json_buf).is_err() {
        anyhow::bail!("failed to render delta JSON");
    }
    std::fs::write(&delta_path, &json_buf)?;
    if !quiet {
        eprintln!("Delta report written to: {}", delta_path.display());
    }

    if comparison.passed() {
        Ok(())
    } else {
        // Surface the regression to the process so the CI gate fires.
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    /// Helper: parse an argv into the Cli, panicking if the command is not
    /// BenchStore. Returns the inner BenchStoreArgs.
    fn parse_bench_store(argv: &[&str]) -> BenchStoreArgs {
        let full: Vec<&str> = std::iter::once("vectorcode")
            .chain(argv.iter().copied())
            .collect();
        let cli = Cli::parse_from(&full);
        match cli.command {
            crate::cli::Commands::BenchStore(args) => args,
            other => panic!("Expected BenchStore, got {other:?}"),
        }
    }

    #[test]
    fn bench_store_args_parse_defaults() {
        let args = parse_bench_store(&["bench-store"]);
        assert_eq!(args.corpus, "vscode");
        assert_eq!(args.backend, BackendArg::SqliteVec);
        assert_eq!(args.slo_secs, 360);
        assert_eq!(args.output, StoreBenchOutput::Toml);
        assert!(args.corpus_config.is_none());
        assert!(!args.mock_embedder);
    }

    #[test]
    fn bench_store_args_parse_mock_embedder() {
        let args = parse_bench_store(&["bench-store", "--mock-embedder"]);
        assert!(
            args.mock_embedder,
            "--mock-embedder should be parsed as true"
        );
    }

    #[test]
    fn bench_store_args_parse_query_sample() {
        let args = parse_bench_store(&["bench-store", "--query-sample", "0"]);
        assert_eq!(args.query_sample, 0);

        let args = parse_bench_store(&["bench-store", "--query-sample", "10"]);
        assert_eq!(args.query_sample, 10);
    }

    #[test]
    fn bench_store_args_query_sample_default_is_100() {
        let args = parse_bench_store(&["bench-store"]);
        assert_eq!(args.query_sample, 100);
    }

    #[test]
    fn bench_store_args_parse_backend() {
        let args = parse_bench_store(&["bench-store", "--backend", "lancedb"]);
        assert_eq!(args.backend, BackendArg::Lancedb);
    }

    #[test]
    fn bench_store_args_parse_corpus() {
        let args = parse_bench_store(&["bench-store", "--corpus", "mini"]);
        assert_eq!(args.corpus, "mini");
    }

    #[test]
    fn bench_store_args_parse_slo() {
        let args = parse_bench_store(&["bench-store", "--slo-secs", "120"]);
        assert_eq!(args.slo_secs, 120);
    }

    #[test]
    fn bench_store_args_parse_output_json() {
        let args = parse_bench_store(&["bench-store", "--output", "json"]);
        assert_eq!(args.output, StoreBenchOutput::Json);
    }

    #[test]
    fn bench_store_args_parse_corpus_config() {
        let args =
            parse_bench_store(&["bench-store", "--corpus-config", "/custom/path/corpus.toml"]);
        assert_eq!(
            args.corpus_config,
            Some(PathBuf::from("/custom/path/corpus.toml"))
        );
    }

    #[test]
    fn bench_store_args_parse_combined() {
        let args = parse_bench_store(&[
            "bench-store",
            "--corpus",
            "vscode",
            "--backend",
            "sqlite-vec",
            "--slo-secs",
            "600",
            "--output",
            "json",
        ]);
        assert_eq!(args.corpus, "vscode");
        assert_eq!(args.backend, BackendArg::SqliteVec);
        assert_eq!(args.slo_secs, 600);
        assert_eq!(args.output, StoreBenchOutput::Json);
    }

    #[test]
    fn bench_store_args_rejects_unknown_backend() {
        let result = Cli::try_parse_from(["vectorcode", "bench-store", "--backend", "nonexistent"]);
        assert!(
            result.is_err(),
            "Unknown backend should be rejected by clap"
        );
    }

    #[test]
    fn bench_store_args_rejects_invalid_output() {
        let result = Cli::try_parse_from(["vectorcode", "bench-store", "--output", "xml"]);
        assert!(result.is_err(), "Invalid output format should be rejected");
    }

    #[test]
    fn backend_arg_roundtrips() {
        for raw in ["sqlite-vec", "sqlite_vec", "sqlite", "lancedb", "lance"] {
            let parsed: BackendArg = raw.parse().expect(raw);
            let s = parsed.to_string();
            let reparsed: BackendArg = s.parse().expect(&s);
            assert_eq!(parsed, reparsed, "round-trip failed for {raw}");
        }
    }

    #[test]
    fn backend_arg_rejects_unknown() {
        assert!("postgres".parse::<BackendArg>().is_err());
    }

    #[test]
    fn store_bench_output_default_is_toml() {
        assert_eq!(StoreBenchOutput::default(), StoreBenchOutput::Toml);
    }

    #[test]
    fn exit_slo_violation_is_nonzero() {
        // Sanity: ensure SLO violation exit code is distinguishable from
        // generic error (1) and success (0).
        assert_ne!(EXIT_SLO_VIOLATION, 0);
        assert_ne!(EXIT_SLO_VIOLATION, 1);
    }

    #[test]
    fn bench_store_args_parse_compare() {
        let args = parse_bench_store(&["bench-store", "--compare", "/path/to/store.json"]);
        assert_eq!(args.compare, Some(PathBuf::from("/path/to/store.json")));
    }

    #[test]
    fn bench_store_args_compare_default_is_none() {
        let args = parse_bench_store(&["bench-store"]);
        assert!(args.compare.is_none());
    }

    #[test]
    fn bench_store_args_parse_compare_combined() {
        let args = parse_bench_store(&[
            "bench-store",
            "--corpus",
            "mock-mini",
            "--mock-embedder",
            "--query-sample",
            "0",
            "--compare",
            "benchmarks/baseline/baseline-store-mock-mini.json",
        ]);
        assert_eq!(args.corpus, "mock-mini");
        assert!(args.mock_embedder);
        assert_eq!(args.query_sample, 0);
        assert_eq!(
            args.compare,
            Some(PathBuf::from(
                "benchmarks/baseline/baseline-store-mock-mini.json"
            ))
        );
    }
}
