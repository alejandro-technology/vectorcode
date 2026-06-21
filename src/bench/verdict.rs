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

use crate::bench::schema::{StoreMetricsReport, Verdict};

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
}
