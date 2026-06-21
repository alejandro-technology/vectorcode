//! Verdict logic for the phase-3 store evaluation (R4).
//!
//! Given two `StoreMetricsReport`s (the incumbent sqlite-vec and the candidate
//! LanceDB), `compare_reports` applies the binding thresholds:
//! - **indexing**: candidate must be ≥1.5x faster than incumbent
//! - **peak RSS**: candidate must be ≤1.2x of incumbent
//! - **on-disk size**: candidate must be ≤1.2x of incumbent
//! - **query p95 latency**: candidate must be ≤1.2x of incumbent
//!
//! ALL 4 axes must pass for `Migrate`. Any failure produces `Stay` with a
//! list of human-readable reasons citing the measured values.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bench::schema::{BenchmarkResult, StoreMetricsReport, Verdict};

/// Tolerance policy for a single metric in the comparison.
///
/// `Absolute(t)` — pass if `|current - baseline| < t`. The boundary is a
/// regress, matching the spec's "exactly ±0.01 fails" rule.
///
/// `Relative(r)` — pass if `current - baseline < baseline * r`, i.e. the
/// metric may grow by at most `r` of the baseline. The boundary
/// `current == baseline * (1 + r)` is also a regress.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Tolerance {
    /// Absolute delta tolerance (e.g. 0.01 for IR metrics).
    Absolute(f64),
    /// Relative tolerance as a fraction of the baseline (e.g. 0.5 = +50%).
    Relative(f64),
}

/// Pass/regress status of a single metric in a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricStatus {
    Pass,
    Regress,
}

/// A single metric's comparison result — name, current/baseline values,
/// signed delta (`current - baseline`), and pass/regress status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDelta {
    pub name: String,
    pub current: f64,
    pub baseline: f64,
    pub delta: f64,
    pub status: MetricStatus,
}

/// Overall verdict of a baseline comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BaselineVerdict {
    /// All metrics pass.
    Pass,
    /// At least one metric regressed. `reasons` is non-empty and lists the
    /// failing metric names with the measured values.
    Regress { reasons: Vec<String> },
}

/// The full output of `compare_to_baseline`: per-metric deltas plus an
/// overall verdict. Regress in any metric -> overall `Regress`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub metrics: Vec<MetricDelta>,
    pub verdict: BaselineVerdict,
}

impl Comparison {
    /// Convenience: did the comparison pass overall?
    pub fn passed(&self) -> bool {
        matches!(self.verdict, BaselineVerdict::Pass)
    }
}

/// Trait implemented by report types that can be compared against a baseline.
///
/// `metrics` returns `(name, value, tolerance)` triples for every metric the
/// report tracks. Names should be stable strings ("recall_at_5", "indexing_secs")
/// because they appear in the delta report and JSON artifact.
pub trait BaselineReport {
    /// The list of `(name, value, tolerance)` for every metric the report
    /// exposes to the baseline comparator.
    fn metrics(&self) -> Vec<(&'static str, f64, Tolerance)>;
}

/// Pure function: compare a current report to a baseline report of the same
/// type and return per-metric deltas + an overall verdict.
///
/// Rules (per spec REQ-4.1-CMP):
/// - Each IR metric uses `Tolerance::Absolute(0.01)`. Boundary `|delta| == 0.01`
///   is a regress.
/// - Store metrics use relative tolerances (per `benchmarks/baseline/SCHEMA.md`).
/// - Missing metric in either report → regress.
/// - `NaN` / infinite values → regress.
pub fn compare_to_baseline<R: BaselineReport>(current: &R, baseline: &R) -> Comparison {
    let current_metrics = current.metrics();
    let baseline_metrics = baseline.metrics();

    let mut deltas: Vec<MetricDelta> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();

    // Index baseline metrics by name for fast lookup. We iterate in the
    // current's metric order so the output ordering is deterministic and
    // matches the report's natural metric order.
    for (name, current_value) in current_metrics.iter().map(|(n, v, _)| (*n, *v)) {
        let baseline_entry = baseline_metrics
            .iter()
            .find(|(n, _, _)| *n == name)
            .copied();
        let (baseline_value, tolerance) = match baseline_entry {
            Some((_, v, t)) => (v, t),
            None => {
                // Metric present in current but not in baseline → regress.
                let reason = format!("missing metric in baseline: {name}");
                deltas.push(MetricDelta {
                    name: name.to_string(),
                    current: current_value,
                    baseline: 0.0,
                    delta: current_value,
                    status: MetricStatus::Regress,
                });
                reasons.push(reason);
                continue;
            }
        };

        let delta = current_value - baseline_value;
        let status = evaluate_metric(current_value, baseline_value, delta, tolerance);
        if status == MetricStatus::Regress {
            reasons.push(format!(
                "{name}: current={current_value:.4} baseline={baseline_value:.4} delta={delta:+.4}"
            ));
        }
        deltas.push(MetricDelta {
            name: name.to_string(),
            current: current_value,
            baseline: baseline_value,
            delta,
            status,
        });
    }

    // Any baseline metric absent from current → regress.
    for (name, baseline_value, _) in &baseline_metrics {
        let present = current_metrics.iter().any(|(n, _, _)| *n == *name);
        if !present {
            reasons.push(format!("missing metric in current: {name}"));
            deltas.push(MetricDelta {
                name: (*name).to_string(),
                current: 0.0,
                baseline: *baseline_value,
                delta: -*baseline_value,
                status: MetricStatus::Regress,
            });
        }
    }

    let verdict = if reasons.is_empty() {
        BaselineVerdict::Pass
    } else {
        BaselineVerdict::Regress { reasons }
    };

    Comparison {
        metrics: deltas,
        verdict,
    }
}

/// Apply a tolerance policy to a single (current, baseline, delta) triple.
///
/// Returns `Regress` for non-finite values (NaN / ±Inf) on either side,
/// otherwise the tolerance decides.
fn evaluate_metric(current: f64, baseline: f64, delta: f64, tolerance: Tolerance) -> MetricStatus {
    if !current.is_finite() || !baseline.is_finite() || !delta.is_finite() {
        return MetricStatus::Regress;
    }

    match tolerance {
        Tolerance::Absolute(t) => {
            if delta.abs() < t {
                MetricStatus::Pass
            } else {
                MetricStatus::Regress
            }
        }
        Tolerance::Relative(r) => {
            // Allow growth up to baseline * r above baseline. The boundary
            // current == baseline * (1 + r) is a regress (strict less-than).
            let allowed_growth = baseline * r;
            if delta < allowed_growth {
                MetricStatus::Pass
            } else {
                MetricStatus::Regress
            }
        }
    }
}

// ─── BaselineReport impls for the existing schema types ─────────────────

/// IR metrics with `Absolute(0.01)` tolerance — recall@5, recall@10, nDCG@10,
/// MRR. Symbol metrics (structural-only) are intentionally excluded from the
/// IR baseline; the structural baseline covers them.
impl BaselineReport for BenchmarkResult {
    fn metrics(&self) -> Vec<(&'static str, f64, Tolerance)> {
        vec![
            (
                "recall_at_5",
                self.aggregate.recall_at_5,
                Tolerance::Absolute(0.01),
            ),
            (
                "recall_at_10",
                self.aggregate.recall_at_10,
                Tolerance::Absolute(0.01),
            ),
            (
                "ndcg_at_10",
                self.aggregate.ndcg_at_10,
                Tolerance::Absolute(0.01),
            ),
            ("mrr", self.aggregate.mrr, Tolerance::Absolute(0.01)),
        ]
    }
}

/// Store metrics with the relative tolerances from `SCHEMA.md`:
/// - `indexing_secs` may grow by +50% (`Relative(0.5)`)
/// - `query_p95_ms` may grow by +100% (`Relative(1.0)`)
/// - `peak_rss_bytes` and `disk_size_bytes` may grow by +20% (`Relative(0.2)`)
impl BaselineReport for StoreMetricsReport {
    fn metrics(&self) -> Vec<(&'static str, f64, Tolerance)> {
        vec![
            (
                "indexing_secs",
                self.indexing_secs,
                Tolerance::Relative(0.5),
            ),
            (
                "peak_rss_bytes",
                self.peak_rss_bytes as f64,
                Tolerance::Relative(0.2),
            ),
            (
                "disk_size_bytes",
                self.disk_size_bytes as f64,
                Tolerance::Relative(0.2),
            ),
            ("query_p95_ms", self.query_p95_ms, Tolerance::Relative(1.0)),
        ]
    }
}

// ─── Baseline file types (deserialized from benchmarks/baseline/*.json) ─

/// Common metadata header for every baseline JSON file. The actual metric
/// payload is in the `metrics` (IR / structural) or `store` (store) field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineHeader {
    pub version: String,
    pub corpus: String,
    pub embedder: String,
    pub generated_at: String,
}

/// IR-quality metric payload — what the published `baseline-mock-mini.json`
/// carries. One row per aggregate metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrBaselineMetrics {
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub ndcg_at_10: f64,
    pub mrr: f64,
}

/// Structural IR metric payload (symbol-level).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralBaselineMetrics {
    pub symbol_recall_at_5: f64,
    pub symbol_recall_at_10: f64,
    pub symbol_precision_at_5: f64,
}

/// Store-perf metric payload (indexing + memory + latency).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreBaselineMetrics {
    pub indexing_secs: f64,
    pub peak_rss_bytes: u64,
    pub disk_size_bytes: u64,
    pub query_p50_ms: f64,
    pub query_p95_ms: f64,
}

/// Top-level IR baseline file: header + search_mode + the 4 IR metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrBaseline {
    #[serde(flatten)]
    pub header: BaselineHeader,
    pub search_mode: String,
    pub metrics: IrBaselineMetrics,
}

impl IrBaseline {
    /// Convert the IR baseline to a `BenchmarkResult` so the same
    /// `compare_to_baseline` function used for live runs applies.
    pub fn to_benchmark_result(&self) -> BenchmarkResult {
        use crate::bench::schema::{AggregateMetrics, QueryResult};
        BenchmarkResult {
            corpus: self.header.corpus.clone(),
            search_mode: self.search_mode.clone(),
            files_indexed: 0,
            chunks_created: 0,
            queries_executed: 0,
            query_results: Vec::<QueryResult>::new(),
            aggregate: AggregateMetrics {
                recall_at_5: self.metrics.recall_at_5,
                recall_at_10: self.metrics.recall_at_10,
                ndcg_at_10: self.metrics.ndcg_at_10,
                mrr: self.metrics.mrr,
                latency_p50_ms: 0.0,
                latency_p95_ms: 0.0,
                latency_avg_ms: 0.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            duration_secs: 0.0,
            embedder: self.header.embedder.clone(),
        }
    }
}

/// Top-level structural baseline file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralBaseline {
    #[serde(flatten)]
    pub header: BaselineHeader,
    pub search_mode: String,
    pub metrics: StructuralBaselineMetrics,
}

impl StructuralBaseline {
    /// Convert to a `BenchmarkResult` (symbol metrics populated; IR metrics 0).
    pub fn to_benchmark_result(&self) -> BenchmarkResult {
        use crate::bench::schema::{AggregateMetrics, QueryResult};
        BenchmarkResult {
            corpus: self.header.corpus.clone(),
            search_mode: self.search_mode.clone(),
            files_indexed: 0,
            chunks_created: 0,
            queries_executed: 0,
            query_results: Vec::<QueryResult>::new(),
            aggregate: AggregateMetrics {
                recall_at_5: 0.0,
                recall_at_10: 0.0,
                ndcg_at_10: 0.0,
                mrr: 0.0,
                latency_p50_ms: 0.0,
                latency_p95_ms: 0.0,
                latency_avg_ms: 0.0,
                symbol_recall_at_5: self.metrics.symbol_recall_at_5,
                symbol_recall_at_10: self.metrics.symbol_recall_at_10,
                symbol_precision_at_5: self.metrics.symbol_precision_at_5,
            },
            duration_secs: 0.0,
            embedder: self.header.embedder.clone(),
        }
    }
}

/// Top-level store baseline file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreBaseline {
    #[serde(flatten)]
    pub header: BaselineHeader,
    pub store: StoreBaselineMetrics,
}

impl StoreBaseline {
    /// Convert to a `StoreMetricsReport` so `compare_to_baseline` works on it.
    pub fn to_store_metrics_report(&self) -> StoreMetricsReport {
        StoreMetricsReport {
            backend: "baseline".to_string(),
            corpus: self.header.corpus.clone(),
            indexing_secs: self.store.indexing_secs,
            files_indexed: 0,
            chunks_created: 0,
            peak_rss_bytes: self.store.peak_rss_bytes,
            disk_size_bytes: self.store.disk_size_bytes,
            query_p50_ms: self.store.query_p50_ms,
            query_p95_ms: self.store.query_p95_ms,
            query_sample_size: 0,
            slo_passed: true,
            slo_limit_secs: 360,
            embedder: self.header.embedder.clone(),
        }
    }
}

// ─── validate_baseline: load + sanity-check a baseline JSON file ────────

/// Load and validate a baseline JSON file. The header is checked for the
/// required fields and a supported `version` ("1"). Returns the raw header
/// so the caller can pick the appropriate variant (IR / structural / store)
/// from the file shape — the IR / structural / store variants are
/// deserialized separately because their `metrics` payload shape differs.
pub fn validate_baseline(path: &Path) -> Result<BaselineHeader, String> {
    if !path.exists() {
        return Err(format!(
            "unable to load baseline: path does not exist: {}",
            path.display()
        ));
    }
    let raw = fs::read_to_string(path).map_err(|e| {
        format!(
            "unable to load baseline: read failed for {}: {e}",
            path.display()
        )
    })?;

    let header: BaselineHeader = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "unable to load baseline: invalid JSON in {}: {e}",
            path.display()
        )
    })?;

    if header.version != "1" {
        return Err(format!(
            "unable to load baseline: unsupported version '{}' (expected '1') in {}",
            header.version,
            path.display()
        ));
    }
    if header.corpus.is_empty() {
        return Err(format!(
            "unable to load baseline: 'corpus' field is empty in {}",
            path.display()
        ));
    }
    if header.embedder.is_empty() {
        return Err(format!(
            "unable to load baseline: 'embedder' field is empty in {}",
            path.display()
        ));
    }

    Ok(header)
}

/// Load an IR baseline file end-to-end (header + IR metrics payload).
pub fn load_ir_baseline(path: &Path) -> Result<IrBaseline, String> {
    let _ = validate_baseline(path)?;
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("unable to load baseline: read failed for {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("unable to load IR baseline: {e}"))
}

/// Load a store baseline file end-to-end.
pub fn load_store_baseline(path: &Path) -> Result<StoreBaseline, String> {
    let _ = validate_baseline(path)?;
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("unable to load baseline: read failed for {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("unable to load store baseline: {e}"))
}

/// Load a structural baseline file end-to-end.
pub fn load_structural_baseline(path: &Path) -> Result<StructuralBaseline, String> {
    let _ = validate_baseline(path)?;
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("unable to load baseline: read failed for {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("unable to load structural baseline: {e}"))
}

/// Compare two backend reports and return a verdict.
///
/// The candidate is the proposed replacement; the incumbent is the current
/// production backend. See module docs for the threshold matrix.
pub fn compare_reports(incumbent: &StoreMetricsReport, candidate: &StoreMetricsReport) -> Verdict {
    let mut reasons = Vec::new();

    // ── 1. Indexing speed (≥1.5x faster) ──────────────────────────────
    let max_indexing_for_migrate = incumbent.indexing_secs / 1.5;
    if candidate.indexing_secs > max_indexing_for_migrate {
        reasons.push(format!(
            "Indexing not 1.5x faster: incumbent={:.1}s, candidate={:.1}s (max {:.1}s)",
            incumbent.indexing_secs, candidate.indexing_secs, max_indexing_for_migrate
        ));
    }

    // ── 2. Peak RSS (≤1.2x) ────────────────────────────────────────────
    if candidate.peak_rss_bytes as f64 > incumbent.peak_rss_bytes as f64 * 1.2 {
        reasons.push(format!(
            "Peak RSS > 1.2x: incumbent={}B, candidate={}B",
            incumbent.peak_rss_bytes, candidate.peak_rss_bytes
        ));
    }

    // ── 3. On-disk size (≤1.2x) ────────────────────────────────────────
    if candidate.disk_size_bytes as f64 > incumbent.disk_size_bytes as f64 * 1.2 {
        reasons.push(format!(
            "Disk size > 1.2x: incumbent={}B, candidate={}B",
            incumbent.disk_size_bytes, candidate.disk_size_bytes
        ));
    }

    // ── 4. Query p95 latency (≤1.2x) ───────────────────────────────────
    if incumbent.query_p95_ms > 0.0 && candidate.query_p95_ms > incumbent.query_p95_ms * 1.2 {
        reasons.push(format!(
            "Query p95 > 1.2x: incumbent={:.1}ms, candidate={:.1}ms",
            incumbent.query_p95_ms, candidate.query_p95_ms
        ));
    }

    // ── 5. SLO check (R3) ──────────────────────────────────────────────
    if !candidate.slo_passed {
        reasons.push(format!(
            "Candidate SLO failed: candidate indexing {:.1}s exceeded {}s SLO",
            candidate.indexing_secs, candidate.slo_limit_secs
        ));
    }
    if !incumbent.slo_passed {
        reasons.push(format!(
            "Incumbent SLO failed: incumbent indexing {:.1}s exceeded {}s SLO",
            incumbent.indexing_secs, incumbent.slo_limit_secs
        ));
    }

    if reasons.is_empty() {
        Verdict::Migrate
    } else {
        Verdict::Stay { reasons }
    }
}

/// Format a verdict as a one-line summary for CLI output.
pub fn verdict_summary(v: &Verdict) -> String {
    match v {
        Verdict::Migrate => "MIGRATE — all 4 axes pass; switch to LanceDB".to_string(),
        Verdict::Stay { reasons } => {
            format!(
                "STAY — {} axis/axes failed:\n  - {}",
                reasons.len(),
                reasons.join("\n  - ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_report(
        backend: &str,
        indexing_secs: f64,
        rss: u64,
        disk: u64,
        p50: f64,
        p95: f64,
        slo_passed: bool,
    ) -> StoreMetricsReport {
        StoreMetricsReport {
            backend: backend.to_string(),
            corpus: "vscode".to_string(),
            indexing_secs,
            files_indexed: 1000,
            chunks_created: 5000,
            peak_rss_bytes: rss,
            disk_size_bytes: disk,
            query_p50_ms: p50,
            query_p95_ms: p95,
            query_sample_size: 100,
            slo_passed,
            slo_limit_secs: 360,
            embedder: "test".to_string(),
        }
    }

    #[test]
    fn verdict_win_all_axes_returns_migrate() {
        let incumbent = make_report(
            "sqlite-vec",
            300.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        let candidate = make_report(
            "lancedb",
            150.0,
            1_000_000_000,
            1_000_000_000,
            25.0,
            50.0,
            true,
        );
        assert_eq!(compare_reports(&incumbent, &candidate), Verdict::Migrate);
    }

    #[test]
    fn verdict_indexing_too_slow_returns_stay() {
        let incumbent = make_report(
            "sqlite-vec",
            300.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        let candidate = make_report(
            "lancedb",
            250.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        match compare_reports(&incumbent, &candidate) {
            Verdict::Stay { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("Indexing")));
            }
            Verdict::Migrate => panic!("Expected Stay"),
        }
    }

    #[test]
    fn verdict_rss_too_high_returns_stay() {
        let incumbent = make_report(
            "sqlite-vec",
            300.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        let candidate = make_report(
            "lancedb",
            150.0,
            1_500_000_000,
            1_000_000_000,
            25.0,
            50.0,
            true,
        );
        match compare_reports(&incumbent, &candidate) {
            Verdict::Stay { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("RSS")));
            }
            Verdict::Migrate => panic!("Expected Stay"),
        }
    }

    #[test]
    fn verdict_disk_too_high_returns_stay() {
        let incumbent = make_report(
            "sqlite-vec",
            300.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        let candidate = make_report(
            "lancedb",
            150.0,
            1_000_000_000,
            1_500_000_000,
            25.0,
            50.0,
            true,
        );
        match compare_reports(&incumbent, &candidate) {
            Verdict::Stay { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("Disk")));
            }
            Verdict::Migrate => panic!("Expected Stay"),
        }
    }

    #[test]
    fn verdict_latency_too_high_returns_stay() {
        let incumbent = make_report(
            "sqlite-vec",
            300.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        let candidate = make_report(
            "lancedb",
            150.0,
            1_000_000_000,
            1_000_000_000,
            25.0,
            150.0,
            true,
        );
        match compare_reports(&incumbent, &candidate) {
            Verdict::Stay { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("p95")));
            }
            Verdict::Migrate => panic!("Expected Stay"),
        }
    }

    #[test]
    fn verdict_slo_failure_on_candidate_returns_stay() {
        let incumbent = make_report(
            "sqlite-vec",
            200.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        // Candidate: passes all 4 axes but blows the SLO
        let candidate = make_report(
            "lancedb",
            100.0,
            1_000_000_000,
            1_000_000_000,
            25.0,
            50.0,
            false,
        );
        match compare_reports(&incumbent, &candidate) {
            Verdict::Stay { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("SLO")));
            }
            Verdict::Migrate => panic!("Expected Stay due to SLO failure"),
        }
    }

    #[test]
    fn verdict_tie_returns_stay() {
        let incumbent = make_report(
            "sqlite-vec",
            300.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        let candidate = make_report(
            "lancedb",
            300.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        assert!(matches!(
            compare_reports(&incumbent, &candidate),
            Verdict::Stay { .. }
        ));
    }

    #[test]
    fn verdict_summary_migrate_is_one_line() {
        let v = Verdict::Migrate;
        let s = verdict_summary(&v);
        assert!(s.contains("MIGRATE"));
    }

    #[test]
    fn verdict_summary_stay_lists_reasons() {
        let v = Verdict::Stay {
            reasons: vec!["indexing too slow".to_string()],
        };
        let s = verdict_summary(&v);
        assert!(s.contains("STAY"));
        assert!(s.contains("indexing too slow"));
    }

    // ─── compare_to_baseline tests (t1.3) ────────────────────────────────

    use crate::bench::schema::{AggregateMetrics, BenchmarkResult, QueryResult};

    /// Build a minimal `BenchmarkResult` with the 4 IR metrics, so the
    /// `BaselineReport` trait returns what we want to compare.
    fn ir_result(recall5: f64, recall10: f64, ndcg: f64, mrr: f64) -> BenchmarkResult {
        BenchmarkResult {
            corpus: "mock-mini".to_string(),
            search_mode: "dense".to_string(),
            files_indexed: 0,
            chunks_created: 0,
            queries_executed: 0,
            query_results: Vec::<QueryResult>::new(),
            aggregate: AggregateMetrics {
                recall_at_5: recall5,
                recall_at_10: recall10,
                ndcg_at_10: ndcg,
                mrr,
                latency_p50_ms: 0.0,
                latency_p95_ms: 0.0,
                latency_avg_ms: 0.0,
                symbol_recall_at_5: 0.0,
                symbol_recall_at_10: 0.0,
                symbol_precision_at_5: 0.0,
            },
            duration_secs: 0.0,
            embedder: "mock-deterministic".to_string(),
        }
    }

    #[test]
    fn compare_pass_all_within_tolerance() {
        // Baseline r@5=0.85, current r@5=0.86 → delta +0.01 (boundary regress per design),
        // so use 0.859 to stay strictly inside ±0.01.
        let baseline = ir_result(0.85, 0.92, 0.90, 0.80);
        let current = ir_result(0.855, 0.915, 0.905, 0.805);

        let cmp = compare_to_baseline(&current, &baseline);

        assert!(
            cmp.passed(),
            "All metrics within tolerance should pass: {:?}",
            cmp.verdict
        );
        assert_eq!(cmp.metrics.len(), 4);
        assert!(cmp.metrics.iter().all(|m| m.status == MetricStatus::Pass));
    }

    #[test]
    fn compare_regress_when_one_metric_outside_tolerance() {
        // baseline r@5 = 0.85, current r@5 = 0.83 → delta -0.02 (regress).
        let baseline = ir_result(0.85, 0.92, 0.90, 0.80);
        let current = ir_result(0.83, 0.92, 0.90, 0.80);

        let cmp = compare_to_baseline(&current, &baseline);

        assert!(!cmp.passed(), "Out-of-tolerance metric must regress");
        match cmp.verdict {
            BaselineVerdict::Regress { reasons } => {
                assert!(
                    reasons.iter().any(|r| r.contains("recall_at_5")),
                    "Reason should mention the regressing metric, got: {reasons:?}"
                );
            }
            BaselineVerdict::Pass => panic!("Expected Regress"),
        }
        let r5 = cmp
            .metrics
            .iter()
            .find(|m| m.name == "recall_at_5")
            .unwrap();
        assert_eq!(r5.status, MetricStatus::Regress);
        assert!((r5.delta - (-0.02)).abs() < 1e-9);
    }

    #[test]
    fn compare_regress_when_metric_missing_in_current() {
        // The comparator iterates the current's metrics first; a metric in
        // baseline that current lacks is detected as missing. Use a custom
        // struct holding a Vec<(&'static str, f64, Tolerance)> so both sides
        // share the same type.
        struct CustomReport(Vec<(&'static str, f64, Tolerance)>);
        impl BaselineReport for CustomReport {
            fn metrics(&self) -> Vec<(&'static str, f64, Tolerance)> {
                self.0.clone()
            }
        }

        let baseline = CustomReport(vec![
            ("recall_at_5", 0.85, Tolerance::Absolute(0.01)),
            ("recall_at_10", 0.92, Tolerance::Absolute(0.01)),
        ]);
        let current = CustomReport(vec![("recall_at_5", 0.85, Tolerance::Absolute(0.01))]);

        let cmp = compare_to_baseline(&current, &baseline);

        assert!(!cmp.passed(), "Missing metric in current must regress");
        match cmp.verdict {
            BaselineVerdict::Regress { reasons } => {
                assert!(
                    reasons
                        .iter()
                        .any(|r| r.contains("recall_at_10") && r.contains("missing")),
                    "Reason should report the missing metric, got: {reasons:?}"
                );
            }
            BaselineVerdict::Pass => panic!("Expected Regress for missing metric"),
        }
    }

    #[test]
    fn compare_boundary_delta_of_exactly_0_01_is_regress() {
        // Spec REQ-4.1-CMP: "Exact boundary fails: baseline nDCG=0.50, current
        // nDCG=0.49 → nDCG verdict regress (delta -0.01 fails)"
        let baseline = ir_result(0.85, 0.92, 0.50, 0.80);
        let current = ir_result(0.85, 0.92, 0.49, 0.80);

        let cmp = compare_to_baseline(&current, &baseline);

        assert!(!cmp.passed(), "Boundary |delta|==0.01 must be regress");
        let ndcg = cmp.metrics.iter().find(|m| m.name == "ndcg_at_10").unwrap();
        assert_eq!(ndcg.status, MetricStatus::Regress);
    }

    #[test]
    fn compare_regress_when_value_is_nan() {
        // Non-finite current value → regress.
        let baseline = ir_result(0.85, 0.92, 0.90, 0.80);
        let mut current = ir_result(0.85, 0.92, 0.90, 0.80);
        current.aggregate.recall_at_5 = f64::NAN;

        let cmp = compare_to_baseline(&current, &baseline);

        assert!(!cmp.passed(), "NaN current value must regress");
        let r5 = cmp
            .metrics
            .iter()
            .find(|m| m.name == "recall_at_5")
            .unwrap();
        assert_eq!(r5.status, MetricStatus::Regress);
    }

    #[test]
    fn compare_store_report_uses_relative_tolerance() {
        // indexing_secs baseline 10.0, current 14.0 → +40% growth, under +50% → pass.
        // peak_rss baseline 1_000_000_000, current 1_300_000_000 → +30%, over +20% → regress.
        let baseline = make_report(
            "baseline",
            10.0,
            1_000_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        let mut current = make_report(
            "candidate",
            14.0,
            1_300_000_000,
            1_000_000_000,
            50.0,
            100.0,
            true,
        );
        // Remove the extra embedder so both reports look identical except for the
        // metrics we want to compare. (Not strictly needed for this test.)
        current.embedder = baseline.embedder.clone();

        let cmp = compare_to_baseline(&current, &baseline);

        // Overall regress because peak_rss_bytes is over the +20% tolerance.
        assert!(!cmp.passed(), "peak_rss over +20% should regress");
        let rss = cmp
            .metrics
            .iter()
            .find(|m| m.name == "peak_rss_bytes")
            .unwrap();
        assert_eq!(rss.status, MetricStatus::Regress);
        let idx = cmp
            .metrics
            .iter()
            .find(|m| m.name == "indexing_secs")
            .unwrap();
        assert_eq!(
            idx.status,
            MetricStatus::Pass,
            "indexing_secs +40% is inside +50% tolerance"
        );
    }

    // ─── validate_baseline + baseline loaders (t1.5) ─────────────────────

    #[test]
    fn validate_baseline_rejects_nonexistent_path() {
        let path = Path::new("/this/path/does/not/exist/baseline.json");
        let result = validate_baseline(path);
        assert!(result.is_err());
        let msg = result.err().unwrap();
        assert!(msg.contains("unable to load baseline"), "Got: {msg}");
        assert!(msg.contains("does not exist"), "Got: {msg}");
    }

    #[test]
    fn validate_baseline_rejects_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "this is not json {").unwrap();
        let result = validate_baseline(&path);
        assert!(result.is_err(), "Malformed JSON should fail validation");
    }

    #[test]
    fn validate_baseline_rejects_missing_required_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incomplete.json");
        std::fs::write(&path, r#"{ "version": "1", "corpus": "x" }"#).unwrap();
        let result = validate_baseline(&path);
        assert!(
            result.is_err(),
            "Missing embedder/generated_at fields should fail"
        );
    }

    #[test]
    fn validate_baseline_rejects_wrong_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrong-version.json");
        std::fs::write(
            &path,
            r#"{
                "version": "2",
                "corpus": "mock-mini",
                "embedder": "mock-deterministic",
                "generated_at": "2026-06-21T00:00:00Z"
            }"#,
        )
        .unwrap();
        let result = validate_baseline(&path);
        assert!(result.is_err(), "Wrong version should fail");
        let msg = result.err().unwrap();
        assert!(msg.contains("unsupported version"), "Got: {msg}");
    }

    #[test]
    fn validate_baseline_accepts_well_formed_ir_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("good.json");
        std::fs::write(
            &path,
            r#"{
                "version": "1",
                "corpus": "mock-mini",
                "embedder": "mock-deterministic",
                "generated_at": "2026-06-21T00:00:00Z",
                "search_mode": "dense",
                "metrics": {
                    "recall_at_5": 0.5,
                    "recall_at_10": 0.7,
                    "ndcg_at_10": 0.6,
                    "mrr": 0.4
                }
            }"#,
        )
        .unwrap();
        let header = validate_baseline(&path).expect("should validate");
        assert_eq!(header.version, "1");
        assert_eq!(header.corpus, "mock-mini");
        assert_eq!(header.embedder, "mock-deterministic");
    }

    #[test]
    fn load_ir_baseline_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ir.json");
        std::fs::write(
            &path,
            r#"{
                "version": "1",
                "corpus": "mock-mini",
                "embedder": "mock-deterministic",
                "generated_at": "2026-06-21T00:00:00Z",
                "search_mode": "dense",
                "metrics": {
                    "recall_at_5": 0.5,
                    "recall_at_10": 0.7,
                    "ndcg_at_10": 0.6,
                    "mrr": 0.4
                }
            }"#,
        )
        .unwrap();
        let baseline = load_ir_baseline(&path).expect("should load IR baseline");
        assert_eq!(baseline.header.embedder, "mock-deterministic");
        assert_eq!(baseline.search_mode, "dense");
        assert!((baseline.metrics.recall_at_5 - 0.5).abs() < 1e-9);

        // The to_benchmark_result conversion must propagate the 4 IR metrics.
        let report = baseline.to_benchmark_result();
        assert!((report.aggregate.recall_at_5 - 0.5).abs() < 1e-9);
        assert!((report.aggregate.mrr - 0.4).abs() < 1e-9);
    }

    #[test]
    fn load_store_baseline_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        std::fs::write(
            &path,
            r#"{
                "version": "1",
                "corpus": "mock-mini",
                "embedder": "mock-deterministic",
                "generated_at": "2026-06-21T00:00:00Z",
                "store": {
                    "indexing_secs": 12.5,
                    "peak_rss_bytes": 100000000,
                    "disk_size_bytes": 50000000,
                    "query_p50_ms": 5.0,
                    "query_p95_ms": 25.0
                }
            }"#,
        )
        .unwrap();
        let baseline = load_store_baseline(&path).expect("should load store baseline");
        assert!((baseline.store.indexing_secs - 12.5).abs() < 1e-9);
        assert_eq!(baseline.store.peak_rss_bytes, 100_000_000);

        let report = baseline.to_store_metrics_report();
        assert!((report.indexing_secs - 12.5).abs() < 1e-9);
        assert_eq!(report.peak_rss_bytes, 100_000_000);
    }
}
