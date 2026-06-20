//! Report generation — table, JSON, and BASELINE.md output.

use std::io::Write;
use std::path::Path;

use anyhow::Result;

use crate::bench::schema::BenchmarkResult;

/// Output format for benchmark results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Print table to stdout.
    Table,
    /// Write JSON to file.
    Json,
    /// Write BASELINE.md + results.json.
    Baseline,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "table" => Ok(OutputFormat::Table),
            "json" => Ok(OutputFormat::Json),
            "baseline" => Ok(OutputFormat::Baseline),
            _ => Err(format!(
                "Invalid output format: {s}. Use: table, json, baseline"
            )),
        }
    }
}

/// Write benchmark results as a formatted table to stdout.
pub fn write_table(result: &BenchmarkResult) -> Result<()> {
    println!();
    println!(
        "Benchmark Results: {} (mode: {})",
        result.corpus, result.search_mode
    );
    println!("{}", "=".repeat(60));
    println!(
        "Files indexed: {} | Chunks: {} | Queries: {}",
        result.files_indexed, result.chunks_created, result.queries_executed
    );
    println!("Duration: {:.2}s", result.duration_secs);
    println!();

    // Aggregate metrics
    println!("Aggregate Metrics:");
    println!("  Recall@5:  {:.4}", result.aggregate.recall_at_5);
    println!("  Recall@10: {:.4}", result.aggregate.recall_at_10);
    println!("  nDCG@10:   {:.4}", result.aggregate.ndcg_at_10);
    println!("  MRR:       {:.4}", result.aggregate.mrr);
    println!();

    // Per-query results
    if !result.query_results.is_empty() {
        println!("Per-Query Results:");
        println!(
            "{:<40} {:>8} {:>8} {:>8} {:>8}",
            "Query", "R@5", "R@10", "nDCG", "MRR"
        );
        println!("{}", "-".repeat(76));

        for qr in &result.query_results {
            let query_display = if qr.query.len() > 38 {
                format!("{}...", &qr.query[..37])
            } else {
                qr.query.clone()
            };
            println!(
                "{:<40} {:>8.4} {:>8.4} {:>8.4} {:>8.4}",
                query_display, qr.recall_at_5, qr.recall_at_10, qr.ndcg_at_10, qr.mrr
            );
        }
    }

    Ok(())
}

/// Write a comparison table for multiple benchmark results (one per mode).
pub fn write_multi_mode_table(results: &[BenchmarkResult]) -> Result<()> {
    if results.is_empty() {
        return Ok(());
    }

    println!();
    println!("Multi-Mode Benchmark Comparison: {}", results[0].corpus);
    println!("{}", "=".repeat(80));

    // Summary header
    println!(
        "{:<15} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "Mode", "Files", "Chunks", "R@5", "nDCG@10", "MRR"
    );
    println!("{}", "-".repeat(80));

    for result in results {
        println!(
            "{:<15} {:>10} {:>10} {:>10.4} {:>10.4} {:>10.4}",
            result.search_mode,
            result.files_indexed,
            result.chunks_created,
            result.aggregate.recall_at_5,
            result.aggregate.ndcg_at_10,
            result.aggregate.mrr,
        );
    }

    println!();

    // Per-mode timing
    println!("Duration by mode:");
    for result in results {
        println!("  {:<15} {:.2}s", result.search_mode, result.duration_secs);
    }
    println!();

    Ok(())
}

/// Write benchmark results as JSON to a file.
pub fn write_json(result: &BenchmarkResult, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(result)?;
    let mut file = std::fs::File::create(path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

/// Write BASELINE.md and results.json for baseline recording.
pub fn write_baseline(result: &BenchmarkResult, output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;

    // Write results.json
    let json_path = output_dir.join("results.json");
    write_json(result, &json_path)?;

    // Write BASELINE.md
    let baseline_path = output_dir.join("BASELINE.md");
    let mut file = std::fs::File::create(&baseline_path)?;

    writeln!(file, "# Benchmark Baseline")?;
    writeln!(file)?;
    writeln!(file, "**Corpus**: {}", result.corpus)?;
    writeln!(file, "**Search Mode**: {}", result.search_mode)?;
    writeln!(file, "**Date**: {}", chrono_now())?;
    writeln!(
        file,
        "**VectorCode Version**: {}",
        env!("CARGO_PKG_VERSION")
    )?;
    writeln!(file)?;
    writeln!(file, "## Setup")?;
    writeln!(file)?;
    writeln!(file, "- Files indexed: {}", result.files_indexed)?;
    writeln!(file, "- Chunks created: {}", result.chunks_created)?;
    writeln!(file, "- Queries executed: {}", result.queries_executed)?;
    writeln!(file, "- Duration: {:.2}s", result.duration_secs)?;
    writeln!(file)?;
    writeln!(file, "## Aggregate Metrics")?;
    writeln!(file)?;
    writeln!(file, "| Metric | Value |")?;
    writeln!(file, "|--------|-------|")?;
    writeln!(file, "| Recall@5 | {:.4} |", result.aggregate.recall_at_5)?;
    writeln!(file, "| Recall@10 | {:.4} |", result.aggregate.recall_at_10)?;
    writeln!(file, "| nDCG@10 | {:.4} |", result.aggregate.ndcg_at_10)?;
    writeln!(file, "| MRR | {:.4} |", result.aggregate.mrr)?;
    writeln!(file)?;
    writeln!(file, "## Reproducibility")?;
    writeln!(file)?;
    writeln!(
        file,
        "Run this benchmark again with: `cargo run --release -- benchmark --corpus {}`",
        result.corpus
    )?;
    writeln!(file)?;
    writeln!(
        file,
        "Expected variance: ±0.01 across 3 runs on ARM (REQ-BENCH-005)."
    )?;

    Ok(())
}

/// Get current timestamp in ISO 8601 format.
fn chrono_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::schema::{AggregateMetrics, BenchmarkResult, QueryResult};
    use tempfile::TempDir;

    fn sample_result() -> BenchmarkResult {
        BenchmarkResult {
            corpus: "test".to_string(),
            search_mode: "dense".to_string(),
            files_indexed: 10,
            chunks_created: 50,
            queries_executed: 2,
            query_results: vec![
                QueryResult {
                    query: "error handling".to_string(),
                    predicted: vec!["error.rs".to_string()],
                    recall_at_5: 0.8,
                    recall_at_10: 1.0,
                    ndcg_at_10: 0.9,
                    mrr: 1.0,
                    symbol_recall_at_5: 0.0,
                    symbol_recall_at_10: 0.0,
                    symbol_precision_at_5: 0.0,
                },
                QueryResult {
                    query: "search function".to_string(),
                    predicted: vec!["search.rs".to_string()],
                    recall_at_5: 0.6,
                    recall_at_10: 0.8,
                    ndcg_at_10: 0.7,
                    mrr: 0.5,
                    symbol_recall_at_5: 0.0,
                    symbol_recall_at_10: 0.0,
                    symbol_precision_at_5: 0.0,
                },
            ],
            aggregate: AggregateMetrics {
                recall_at_5: 0.7,
                recall_at_10: 0.9,
                ndcg_at_10: 0.8,
                mrr: 0.75,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            duration_secs: 5.5,
        }
    }

    #[test]
    fn test_write_table_output() {
        let result = sample_result();
        // Just verify it doesn't panic
        write_table(&result).unwrap();
    }

    #[test]
    fn test_write_json() {
        let result = sample_result();
        let dir = TempDir::new().unwrap();
        let json_path = dir.path().join("results.json");

        write_json(&result, &json_path).unwrap();

        assert!(json_path.exists());
        let content = std::fs::read_to_string(&json_path).unwrap();
        assert!(content.contains("test"));
        assert!(content.contains("0.7"));
    }

    #[test]
    fn test_write_baseline() {
        let result = sample_result();
        let dir = TempDir::new().unwrap();

        write_baseline(&result, dir.path()).unwrap();

        let baseline_path = dir.path().join("BASELINE.md");
        let json_path = dir.path().join("results.json");

        assert!(baseline_path.exists());
        assert!(json_path.exists());

        let baseline_content = std::fs::read_to_string(&baseline_path).unwrap();
        assert!(baseline_content.contains("# Benchmark Baseline"));
        assert!(baseline_content.contains("test"));
        assert!(baseline_content.contains("dense"));
        assert!(baseline_content.contains("0.7"));
    }

    #[test]
    fn test_write_multi_mode_table_output() {
        let dense = sample_result();
        let hybrid = BenchmarkResult {
            search_mode: "hybrid".to_string(),
            aggregate: AggregateMetrics {
                recall_at_5: 0.8,
                recall_at_10: 0.95,
                ndcg_at_10: 0.85,
                mrr: 0.8,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            ..sample_result()
        };
        // Just verify it doesn't panic
        write_multi_mode_table(&[dense, hybrid]).unwrap();
    }

    #[test]
    fn test_write_multi_mode_table_empty() {
        write_multi_mode_table(&[]).unwrap();
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(
            "table".parse::<OutputFormat>().unwrap(),
            OutputFormat::Table
        );
        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!(
            "baseline".parse::<OutputFormat>().unwrap(),
            OutputFormat::Baseline
        );
        assert!("invalid".parse::<OutputFormat>().is_err());
    }
}
