//! Metrics probes for the phase-3 store evaluation harness (R2) and IR metrics
//! for the search quality benchmark.
//!
//! Two layers:
//! - **Probes** (R2): WallClock, PeakRss, DiskUsage, LatencyPercentiles —
//!   used to produce `StoreMetricsReport`.
//! - **IR metrics** (REQ-BENCH-001): recall_at_k, ndcg_at_k, mrr, etc. —
//!   used by `runner.rs` to score search quality.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

// ─── WallClock ──────────────────────────────────────────────────────────

/// `std::time::Instant`-backed wall-clock probe.
pub struct WallClock {
    start: Instant,
}

impl WallClock {
    /// Start the clock.
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Elapsed seconds since `start()`.
    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Elapsed milliseconds since `start()`.
    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

// ─── PeakRss (getrusage) ────────────────────────────────────────────────

/// Peak resident set size probe, normalized to bytes.
///
/// `getrusage(RUSAGE_SELF)` returns `ru_maxrss`:
/// - **Linux**: kilobytes
/// - **macOS**: bytes
/// - **BSD**: kilobytes
///
/// We normalize to bytes at the call site so downstream comparisons are
/// unit-free.
pub struct PeakRss;

impl PeakRss {
    /// Sample current peak RSS in bytes. Returns 0 if the syscall fails.
    pub fn sample_bytes() -> u64 {
        #[cfg(unix)]
        unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            let r = libc::getrusage(libc::RUSAGE_SELF, &mut usage);
            if r != 0 {
                return 0;
            }
            // ru_maxrss is c_long. On Linux = KB, on macOS = bytes.
            #[cfg(target_os = "macos")]
            {
                usage.ru_maxrss as u64
            }
            #[cfg(not(target_os = "macos"))]
            {
                (usage.ru_maxrss as u64) * 1024
            }
        }
        #[cfg(not(unix))]
        {
            0
        }
    }
}

// ─── DiskUsage ──────────────────────────────────────────────────────────

/// Recursive file-size sum for a directory.
pub struct DiskUsage;

impl DiskUsage {
    /// Sum file sizes in `dir` in bytes. Returns 0 for nonexistent dirs.
    pub fn measure(dir: &Path) -> u64 {
        let mut total = 0u64;
        Self::walk(dir, &mut total);
        total
    }

    fn walk(path: &Path, total: &mut u64) {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    Self::walk(&p, total);
                } else if let Ok(meta) = std::fs::metadata(&p) {
                    *total += meta.len();
                }
            }
        }
    }
}

// ─── LatencyPercentiles ─────────────────────────────────────────────────

/// Manual percentile computation over a sample of latencies (in milliseconds).
///
/// p50 = ceil(0.50 * N) - 1 (clamped)
/// p95 = ceil(0.95 * N) - 1 (clamped)
pub struct LatencyPercentiles;

impl LatencyPercentiles {
    /// Compute p50 and p95 from a sample of latency observations (in ms).
    /// Returns (0.0, 0.0) for an empty sample.
    pub fn from_samples(samples: &[f64]) -> (f64, f64) {
        if samples.is_empty() {
            return (0.0, 0.0);
        }
        let mut sorted: Vec<f64> = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = sorted[Self::index_for_quantile(sorted.len(), 0.50)];
        let p95 = sorted[Self::index_for_quantile(sorted.len(), 0.95)];
        (p50, p95)
    }

    /// Nearest-rank index for a given quantile in a sample of size n.
    /// Uses the convention p_i = ceil(q * n) - 1 (0-indexed).
    fn index_for_quantile(n: usize, q: f64) -> usize {
        let rank = (q * n as f64).ceil() as usize;
        rank.saturating_sub(1).min(n - 1)
    }
}

// ─── IR metrics (search quality scoring — REQ-BENCH-001) ────────────────

/// Recall@k — fraction of relevant documents found in the top-k results.
///
/// `predicted`: ordered list of file paths (ranked by score, highest first)
/// `relevant`: set of file paths that are relevant (grade >= 1)
/// `k`: cutoff position
///
/// Returns 0.0 if `relevant` is empty or `predicted` is empty.
pub fn recall_at_k(predicted: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() || predicted.is_empty() {
        return 0.0;
    }

    let top_k = &predicted[..predicted.len().min(k)];
    let found = top_k.iter().filter(|p| relevant.contains(*p)).count();

    found as f64 / relevant.len() as f64
}

/// nDCG@k — normalized discounted cumulative gain with graded relevance.
///
/// `predicted`: ordered list of file paths (ranked by score, highest first)
/// `grades`: map from file path to relevance grade (0-3, where 3 is most relevant)
/// `k`: cutoff position
///
/// Returns 0.0 if the ideal DCG is 0 (no relevant documents in the universe).
pub fn ndcg_at_k(predicted: &[String], grades: &HashMap<String, f64>, k: usize) -> f64 {
    let dcg = compute_dcg(predicted, grades, k);
    let ideal_dcg = compute_ideal_dcg(grades, k);

    if ideal_dcg == 0.0 {
        return 0.0;
    }

    dcg / ideal_dcg
}

fn compute_dcg(predicted: &[String], grades: &HashMap<String, f64>, k: usize) -> f64 {
    let top_k = &predicted[..predicted.len().min(k)];
    top_k
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let rel = grades.get(path).copied().unwrap_or(0.0);
            let position = (i + 1) as f64;
            rel / (position + 1.0).log2()
        })
        .sum()
}

fn compute_ideal_dcg(grades: &HashMap<String, f64>, k: usize) -> f64 {
    let mut sorted_grades: Vec<f64> = grades.values().copied().collect();
    sorted_grades.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    sorted_grades
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, &rel)| {
            let position = (i + 1) as f64;
            rel / (position + 1.0).log2()
        })
        .sum()
}

/// MRR (mean reciprocal rank) — average of 1/rank for the first relevant result.
pub fn mrr(predicted: &[String], relevant: &HashSet<String>) -> f64 {
    if predicted.is_empty() || relevant.is_empty() {
        return 0.0;
    }

    for (i, path) in predicted.iter().enumerate() {
        if relevant.contains(path) {
            return 1.0 / (i + 1) as f64;
        }
    }

    0.0
}

/// Symbol recall@k — fraction of expected symbols found in top-k predicted symbols.
pub fn symbol_recall_at_k(predicted: &[String], expected: &HashSet<String>, k: usize) -> f64 {
    if expected.is_empty() || predicted.is_empty() {
        return 0.0;
    }

    let top_k = &predicted[..predicted.len().min(k)];
    let found = top_k.iter().filter(|p| expected.contains(*p)).count();

    found as f64 / expected.len() as f64
}

/// Symbol precision@k — fraction of top-k predicted symbols that are expected.
pub fn symbol_precision_at_k(predicted: &[String], expected: &HashSet<String>, k: usize) -> f64 {
    if predicted.is_empty() {
        return 0.0;
    }

    let top_k = &predicted[..predicted.len().min(k)];
    let found = top_k.iter().filter(|p| expected.contains(*p)).count();

    found as f64 / k as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── WallClock ────────────────────────────────────────────────────

    #[test]
    fn wall_clock_measures_elapsed() {
        let clock = WallClock::start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = clock.elapsed_secs();
        assert!(elapsed > 0.0, "elapsed should be > 0, got {elapsed}");
        assert!(
            elapsed < 1.0,
            "elapsed should be < 1s for 10ms sleep, got {elapsed}"
        );
    }

    // ─── PeakRss ──────────────────────────────────────────────────────

    #[test]
    fn peak_rss_returns_positive_value() {
        let rss = PeakRss::sample_bytes();
        // On Linux/macOS the process is at least a few MB; if getrusage
        // failed (returned 0), that itself is reported.
        if cfg!(unix) {
            assert!(rss > 0, "Peak RSS should be > 0 on unix, got {rss}");
        }
    }

    // ─── DiskUsage ────────────────────────────────────────────────────

    #[test]
    fn disk_usage_measures_files_in_dir() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "hello world").unwrap(); // 11 bytes
        std::fs::write(p.join("b.txt"), "rust").unwrap(); // 4 bytes
        let total = DiskUsage::measure(p);
        assert_eq!(total, 15, "11 + 4 = 15 bytes");
    }

    #[test]
    fn disk_usage_recurses_into_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir(p.join("sub")).unwrap();
        std::fs::write(p.join("root.txt"), "x").unwrap();
        std::fs::write(p.join("sub/nested.txt"), "yyyyy").unwrap();
        let total = DiskUsage::measure(p);
        assert_eq!(total, 6, "1 + 5 = 6 bytes");
    }

    #[test]
    fn disk_usage_nonexistent_dir_returns_zero() {
        let total = DiskUsage::measure(std::path::Path::new("/nonexistent/dir/here"));
        assert_eq!(total, 0);
    }

    // ─── LatencyPercentiles ───────────────────────────────────────────

    #[test]
    fn percentiles_empty_sample_returns_zero() {
        let (p50, p95) = LatencyPercentiles::from_samples(&[]);
        assert_eq!(p50, 0.0);
        assert_eq!(p95, 0.0);
    }

    #[test]
    fn percentiles_single_sample_returns_same() {
        let (p50, p95) = LatencyPercentiles::from_samples(&[42.0]);
        assert_eq!(p50, 42.0);
        assert_eq!(p95, 42.0);
    }

    #[test]
    fn percentiles_sorted_samples_match_nearest_rank() {
        // 10 samples, sorted. p50 = index ceil(0.5*10) - 1 = 4
        // p95 = index ceil(0.95*10) - 1 = 9 (clamped to n-1)
        let samples: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let (p50, p95) = LatencyPercentiles::from_samples(&samples);
        assert_eq!(p50, 4.0, "p50 of 10 samples = index 4");
        assert_eq!(p95, 9.0, "p95 of 10 samples = index 9");
    }

    #[test]
    fn percentiles_unsorted_input_is_sorted_internally() {
        let samples = vec![5.0, 1.0, 3.0, 9.0, 2.0, 7.0];
        let (p50, p95) = LatencyPercentiles::from_samples(&samples);
        // Sorted: [1, 2, 3, 5, 7, 9]. p50 = idx 2 (0.5*6=3, -1=2) = 3
        // p95 = idx 5 (0.95*6=5.7 ceil=6, -1=5) = 9
        assert_eq!(p50, 3.0, "p50 = 3rd element after sort");
        assert_eq!(p95, 9.0, "p95 = 6th element after sort");
    }

    #[test]
    fn percentiles_100_samples_typical_workflow() {
        // Simulate 100 query latencies in ms
        let samples: Vec<f64> = (1..=100).map(|i| i as f64).collect();
        let (p50, p95) = LatencyPercentiles::from_samples(&samples);
        // p50 = idx ceil(0.5*100) - 1 = 49 → value 50.0
        // p95 = idx ceil(0.95*100) - 1 = 94 → value 95.0
        assert_eq!(p50, 50.0);
        assert_eq!(p95, 95.0);
    }
}
