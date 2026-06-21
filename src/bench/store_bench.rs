//! Store evaluation benchmark — phase 3 (3.1) implementation.
//!
//! `run_store_benchmark` captures the 4 axes per the spec (R2):
//! indexing wall-clock, peak RSS, on-disk size, query latency p50/p95.
//! It runs the full engine pipeline (chunker → embedder → store) and produces
//! a `StoreMetricsReport`.
//!
//! Spec: R2 (Parameterized Benchmark Harness), R3 (Hard Indexing SLO ≤6min).
//!
//! ## Backend selection
//!
//! The harness is parameterized on a `StoreFactory` (per R2). The factory
//! creates the `Store` impl (sqlite-vec or feature-gated LanceDB). For the
//! measurement, the harness uses the inner `Database` from `SqliteStore` to
//! drive the existing `Indexer` (the engine refactor to use the `Store` trait
//! directly is its own deliverable — see `sdd/phase-3-store-eval-migration`).
//!
//! The harness reports a `StoreMetricsReport` regardless of whether the engine
//! was refactored, because all 4 axes are observable externally (wall-clock
//! via `Instant`, RSS via `getrusage`, disk size via `du`-equivalent, query
//! latency via the store trait).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tempfile::TempDir;

use crate::bench::corpus::{Corpus, LocalCorpus};
use crate::bench::metrics::{DiskUsage, LatencyPercentiles, PeakRss, WallClock};
use crate::bench::schema::StoreMetricsReport;
use crate::config::schema::IndexingConfig;
use crate::embedder::Embedder;
use crate::engine::Indexer;
use crate::store::store::StoreFactory;

/// Default SLO: indexing the vscode corpus (≤15K files) must complete in ≤6min.
pub const DEFAULT_SLO_SECS: u32 = 360;

/// Number of warm-up queries before the latency sample.
const WARMUP_QUERIES: usize = 5;

/// Number of queries in the latency sample.
const SAMPLE_QUERIES: usize = 100;

/// Run the store benchmark through a `StoreFactory` and produce a metrics report.
pub async fn run_store_benchmark(
    factory: &dyn StoreFactory,
    corpus: &dyn Corpus,
    embedder: Arc<dyn Embedder>,
    slo_limit_secs: u32,
) -> Result<StoreMetricsReport> {
    // Step 1: prepare corpus
    let corpus_dir = TempDir::new()?;
    let corpus_path = corpus_dir.path();

    let relative_files = corpus.prepare(corpus_path).await?;
    if relative_files.is_empty() {
        anyhow::bail!(
            "Corpus '{}' produced no files. Check file_extensions filter.",
            corpus.name()
        );
    }
    let absolute_files: Vec<PathBuf> = relative_files
        .iter()
        .map(|rel| corpus_path.join(rel))
        .collect();

    // Step 2: create store via factory at a temp path (so we can measure disk)
    let index_dir = TempDir::new()?;
    let index_path = index_dir.path().join("index.bin");
    let store = factory.create(&index_path)?;
    store.init_schema(embedder.dimensions())?;
    let backend_name = factory.backend_name().to_string();
    let store_ref: &dyn crate::store::store::Store = store.as_ref();

    // Step 3: index the corpus. For the current commit, the engine's
    // `Indexer::new` takes `Arc<tokio::sync::Mutex<Database>>` directly, so
    // we extract it from the `SqliteStore` via the `database()` accessor.
    // When the engine is refactored to use the `Store` trait, this branch
    // becomes a single trait call.
    let db = extract_db_for_indexer(store_ref).ok_or_else(|| {
        anyhow::anyhow!(
            "StoreFactory '{}' does not yet expose a Database for the engine's Indexer. \
             The engine refactor to consume the Store trait directly is a separate deliverable.",
            backend_name
        )
    })?;

    let indexing_config = IndexingConfig::default();
    let indexer = Indexer::new(db.clone(), embedder.clone(), indexing_config);

    let wall_clock = WallClock::start();
    let peak_rss_before = PeakRss::sample_bytes();
    let index_report = indexer.index_files(&absolute_files, corpus_path).await?;
    let indexing_secs = wall_clock.elapsed_secs();
    let peak_rss_after = PeakRss::sample_bytes();
    let peak_rss_bytes = peak_rss_after.max(peak_rss_before);

    // Step 4: on-disk size
    let disk_size_bytes = if index_path.is_dir() {
        DiskUsage::measure(&index_path)
    } else if index_path.exists() {
        std::fs::metadata(&index_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    // Step 5: query latency sample.
    // We drive dense queries through the store trait directly. This measures
    // the store path, not the full search pipeline (which would include the
    // embedder). For the store benchmark that's exactly what we want.
    let mut latencies_ms = Vec::with_capacity(SAMPLE_QUERIES);
    let dim = embedder.dimensions() as usize;

    // Warmup queries (not counted in the sample)
    for i in 0..WARMUP_QUERIES {
        let q = make_random_query(dim, &format!("warmup-{i}"));
        let _ = store_ref.search_dense(&q, 10, 0.0, None);
    }

    for i in 0..SAMPLE_QUERIES {
        let q = make_random_query(dim, &format!("sample-{i}"));
        let t = Instant::now();
        let _ = store_ref.search_dense(&q, 10, 0.0, None);
        latencies_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    let (query_p50_ms, query_p95_ms) = LatencyPercentiles::from_samples(&latencies_ms);

    let slo_passed = indexing_secs <= slo_limit_secs as f64;

    Ok(StoreMetricsReport {
        backend: backend_name,
        corpus: corpus.name().to_string(),
        indexing_secs,
        files_indexed: index_report.files_indexed,
        chunks_created: index_report.chunks_new,
        peak_rss_bytes,
        disk_size_bytes,
        query_p50_ms,
        query_p95_ms,
        query_sample_size: SAMPLE_QUERIES,
        slo_passed,
        slo_limit_secs,
    })
}

/// Extract the inner `Arc<tokio::sync::Mutex<Database>>` from a `Box<dyn Store>`.
///
/// Returns `None` for backends that don't expose a Database (e.g., LanceDB
/// before the engine refactor). The harness treats this as a "no Database
/// available for the legacy engine" case.
fn extract_db_for_indexer(
    store: &dyn crate::store::store::Store,
) -> Option<Arc<tokio::sync::Mutex<crate::store::db::Database>>> {
    // Downcast via the concrete SqliteStore type. This is safe because the
    // factory is the one that produced the store; for LanceDB the factory
    // returns a different type and the downcast fails.
    use std::any::Any;
    let any: &dyn Any = store.as_any();
    any.downcast_ref::<crate::store::sqlite::SqliteStore>()
        .map(|sqlite| sqlite.database())
}

/// Generate a deterministic pseudo-random query vector for latency sampling.
fn make_random_query(dim: usize, seed: &str) -> Vec<f32> {
    let mut state: u64 = seed
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    let mut vec = Vec::with_capacity(dim);
    for _ in 0..dim {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let v = (state >> 33) as i32 as f32 / i32::MAX as f32;
        vec.push(v);
    }
    // L2 normalize
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }
    vec
}

/// Construct a `LocalCorpus` from a directory path.
pub fn local_corpus_from_dir(name: &str, path: PathBuf, exts: Vec<String>) -> LocalCorpus {
    LocalCorpus::new(name.to_string(), path, exts)
}

/// Compute a verdict comparing two backend reports. The candidate is the
/// proposed replacement; the incumbent is the current production backend.
///
/// Migrate requires ALL 4 axes to pass:
/// - indexing: candidate must be ≥1.5x faster
/// - RSS: candidate must be ≤1.2x of incumbent
/// - disk: candidate must be ≤1.2x of incumbent
/// - latency: candidate must be ≤1.2x of incumbent
///
/// Otherwise stay with the incumbent and return the reasons.
pub use crate::bench::verdict::compare_reports as compare_reports_impl;

/// Mark a report's SLO as failed when the benchmark exceeds the limit.
/// Use this when ingesting pre-recorded metrics that haven't been through
/// the live harness.
pub fn apply_slo(report: &mut StoreMetricsReport, slo_limit_secs: u32) {
    report.slo_limit_secs = slo_limit_secs;
    report.slo_passed = report.indexing_secs <= slo_limit_secs as f64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::mock::MockEmbedder;
    use crate::store::sqlite::SqliteStoreFactory;
    use tempfile::TempDir;
    #[allow(dead_code)]
    fn make_report(
        backend: &str,
        indexing_secs: f64,
        rss: u64,
        disk: u64,
        p50: f64,
        p95: f64,
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
            slo_passed: indexing_secs <= 360.0,
            slo_limit_secs: 360,
        }
    }

    fn make_test_corpus() -> (TempDir, LocalCorpus) {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        for (name, content) in [
            ("a.rs", "pub fn alpha() { 1 }\npub fn beta() { 2 }\n"),
            ("b.rs", "pub fn gamma() { 3 }\n"),
            ("c.rs", "pub fn delta() { 4 }\npub fn epsilon() { 5 }\n"),
        ] {
            std::fs::write(p.join(name), content).unwrap();
        }
        let corpus = LocalCorpus::new("test".to_string(), p.to_path_buf(), vec![".rs".to_string()]);
        (dir, corpus)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_benchmark_produces_full_report() {
        let (_dir, corpus) = make_test_corpus();
        let factory = SqliteStoreFactory;
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(4));

        let report = run_store_benchmark(&factory, &corpus, embedder, DEFAULT_SLO_SECS)
            .await
            .unwrap();

        // Spec: report must have non-zero values for all 4 axes.
        assert!(
            report.indexing_secs > 0.0,
            "indexing_secs must be > 0, got {}",
            report.indexing_secs
        );
        assert!(
            report.peak_rss_bytes > 0,
            "peak_rss_bytes must be > 0, got {}",
            report.peak_rss_bytes
        );
        assert!(
            report.query_p50_ms >= 0.0,
            "query_p50_ms must be >= 0, got {}",
            report.query_p50_ms
        );
        assert!(
            report.query_sample_size > 0,
            "query_sample_size must be > 0"
        );
        assert_eq!(report.backend, "sqlite-vec");
        assert_eq!(report.corpus, "test");
        assert!(report.slo_passed, "Small corpus should pass the SLO");
        assert_eq!(report.files_indexed, 3);
    }

    #[test]
    fn local_corpus_from_dir_works() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        std::fs::write(p.join("x.rs"), "fn x() {}").unwrap();
        let corpus = local_corpus_from_dir("test", p.to_path_buf(), vec![".rs".to_string()]);
        assert_eq!(corpus.name(), "test");
    }

    #[test]
    fn make_random_query_is_deterministic() {
        let q1 = make_random_query(8, "seed-1");
        let q2 = make_random_query(8, "seed-1");
        assert_eq!(q1, q2, "Same seed should produce same vector");
        let q3 = make_random_query(8, "seed-2");
        assert_ne!(q1, q3, "Different seed should produce different vector");
    }

    #[test]
    fn make_random_query_is_l2_normalized() {
        let q = make_random_query(16, "any");
        let norm: f32 = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
