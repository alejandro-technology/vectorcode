#!/usr/bin/env python3
"""
Phase 1 IR Precision Benchmark for VectorCode.

Measures cold indexing time, per-query search latency, and P@1/P@3/P@5
precision over a 50-pair evaluation dataset by orchestrating the `vectorcode`
CLI as a black-box subprocess.

Usage:
    # From the repository root:
    python -m venv .venv && source .venv/bin/activate
    pip install -r benchmarks/requirements.txt
    python benchmarks/phase1_precision.py

    # Override provider (default: onnx):
    VECTORCODE_PROVIDER=gemini python benchmarks/phase1_precision.py

    # Use installed binary instead of cargo run:
    VECTORCODE_BIN=vectorcode python benchmarks/phase1_precision.py

Requirements:
    - Python 3.10+
    - psutil (optional — graceful fallback if unavailable)
    - vectorcode CLI available via `cargo run --` or on PATH
    - The project must be a git repo with source files to index

Output:
    - benchmarks/results/phase1_report.json  (structured metrics)
    - stdout summary table (human-readable)
"""

from __future__ import annotations

import json
import os
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Optional dependency: psutil
# ---------------------------------------------------------------------------
try:
    import psutil

    _HAS_PSUTIL = True
except ImportError:
    _HAS_PSUTIL = False

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
DATASET_PATH = SCRIPT_DIR / "eval-dataset.json"
RESULTS_DIR = SCRIPT_DIR / "results"
REPORT_PATH = RESULTS_DIR / "phase1_report.json"
NUM_ITERATIONS = 3
SEARCH_LIMIT = 5


# ---------------------------------------------------------------------------
# Core functions
# ---------------------------------------------------------------------------


def find_vectorcode() -> list[str]:
    """Detect how to invoke vectorcode.

    Priority:
      1. VECTORCODE_BIN env var (explicit path or binary name)
      2. `vectorcode` on PATH
      3. `cargo run --` fallback (builds from source)

    Returns:
        Command prefix list, e.g. ["vectorcode"] or ["cargo", "run", "--"].
    """
    # 1. Explicit override
    env_bin = os.environ.get("VECTORCODE_BIN")
    if env_bin:
        return [env_bin]

    # 2. Binary on PATH
    if shutil.which("vectorcode") is not None:
        return ["vectorcode"]

    # 3. Fallback to cargo run
    return ["cargo", "run", "--"]


def load_dataset(path: Path) -> list[dict]:
    """Load and validate the evaluation dataset.

    Args:
        path: Path to the JSON dataset file.

    Returns:
        List of dicts with keys: query, expected_file, description.

    Raises:
        FileNotFoundError: If the dataset file does not exist.
        ValueError: If the schema is invalid.
    """
    if not path.is_file():
        raise FileNotFoundError(f"Dataset not found: {path}")

    with open(path, encoding="utf-8") as f:
        data = json.load(f)

    if not isinstance(data, list):
        raise ValueError("Dataset must be a JSON array")

    required_keys = {"query", "expected_file", "description"}
    for i, entry in enumerate(data):
        missing = required_keys - set(entry.keys())
        if missing:
            raise ValueError(
                f"Entry {i} missing keys: {missing}"
            )

    return data


def _run_subprocess(
    cmd: list[str],
    cwd: Path,
    env_extra: dict[str, str] | None = None,
    timeout: float = 600.0,
) -> subprocess.CompletedProcess[str]:
    """Run a subprocess with common settings."""
    env = os.environ.copy()
    if env_extra:
        env.update(env_extra)

    return subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def _clean_vectorcode_dir(project_path: Path) -> None:
    """Remove .vectorcode/ directory and stale lock file for a clean init."""
    vc_dir = project_path / ".vectorcode"
    if vc_dir.is_dir():
        shutil.rmtree(vc_dir)
    lock_file = project_path / ".vectorcode.init.lock"
    if lock_file.exists():
        lock_file.unlink()


def run_init(cmd_prefix: list[str], project_path: Path) -> float:
    """Run `vectorcode init --provider <provider>` and return elapsed seconds.

    Cleans up any existing .vectorcode/ directory first, since init fails
    if the project is already initialized.

    Uses VECTORCODE_PROVIDER env var (default: onnx) for non-interactive mode.
    Also passes --provider flag explicitly for redundancy.

    Args:
        cmd_prefix: Command prefix (e.g. ["cargo", "run", "--"]).
        project_path: Path to the project root.

    Returns:
        Wall-clock time in seconds.

    Raises:
        RuntimeError: If init exits with non-zero code.
    """
    provider = os.environ.get("VECTORCODE_PROVIDER", "onnx")
    model = os.environ.get("VECTORCODE_MODEL")

    # Clean up before init — init errors if .vectorcode/ already exists
    _clean_vectorcode_dir(project_path)

    cmd = [*cmd_prefix, "--quiet", "--project-path", str(project_path), "init", "--provider", provider]
    if model:
        cmd.extend(["--model", model])

    start = time.perf_counter()
    result = _run_subprocess(cmd, project_path, timeout=600.0)
    elapsed = time.perf_counter() - start

    if result.returncode != 0:
        raise RuntimeError(
            f"init failed (exit {result.returncode}): {result.stderr}"
        )

    return elapsed


def run_index(cmd_prefix: list[str], project_path: Path) -> float:
    """Run `vectorcode index --full` and return elapsed seconds.

    Args:
        cmd_prefix: Command prefix.
        project_path: Path to the project root.

    Returns:
        Wall-clock time in seconds.

    Raises:
        RuntimeError: If index exits with non-zero code.
    """
    cmd = [*cmd_prefix, "--quiet", "--project-path", str(project_path), "index", "--full"]

    start = time.perf_counter()
    result = _run_subprocess(cmd, project_path, timeout=1800.0)
    elapsed = time.perf_counter() - start

    if result.returncode != 0:
        raise RuntimeError(
            f"index failed (exit {result.returncode}): {result.stderr}"
        )

    return elapsed


def run_search(
    cmd_prefix: list[str],
    project_path: Path,
    query: str,
    limit: int = SEARCH_LIMIT,
) -> tuple[list[dict], float]:
    """Run `vectorcode search "<query>" --json --limit <N>`.

    Args:
        cmd_prefix: Command prefix.
        project_path: Path to the project root.
        query: Natural language search query.
        limit: Max results to return.

    Returns:
        Tuple of (parsed JSON results list, elapsed seconds).
        Results are dicts with at least a 'file_path' key.
    """
    cmd = [
        *cmd_prefix,
        "--project-path",
        str(project_path),
        "search",
        query,
        "--json",
        "--limit",
        str(limit),
    ]

    start = time.perf_counter()
    result = _run_subprocess(cmd, project_path, timeout=60.0)
    elapsed = time.perf_counter() - start

    if result.returncode != 0:
        print(
            f"  WARNING: search failed for query '{query}': {result.stderr.strip()}",
            file=sys.stderr,
        )
        return ([], elapsed)

    try:
        results = json.loads(result.stdout)
        if not isinstance(results, list):
            results = []
    except json.JSONDecodeError:
        print(
            f"  WARNING: invalid JSON from search '{query}'",
            file=sys.stderr,
        )
        results = []

    return (results, elapsed)


def compute_precision(results: list[dict], expected_file: str, k: int) -> int:
    """Check if expected_file appears in the top-K results.

    Args:
        results: List of result dicts (each with 'file_path' key).
        expected_file: Relative path to check for.
        k: Number of top results to consider.

    Returns:
        1 if found in top-K, 0 otherwise.
    """
    top_k = results[:k]
    for r in top_k:
        if r.get("file_path") == expected_file:
            return 1
    return 0


def aggregate_stats(values: list[float]) -> tuple[float, float]:
    """Compute median and P95 from a list of values.

    Args:
        values: Numeric values (e.g. latencies in ms).

    Returns:
        Tuple of (median, p95).
    """
    if not values:
        return (0.0, 0.0)

    sorted_vals = sorted(values)
    n = len(sorted_vals)
    median = statistics.median(sorted_vals)

    # P95: nearest-rank method
    p95_index = min(int(n * 0.95), n - 1)
    p95 = sorted_vals[p95_index]

    return (median, p95)


def sample_ram() -> int | None:
    """Sample current process RSS in bytes.

    Returns:
        RSS bytes if psutil is available, None otherwise.
    """
    if not _HAS_PSUTIL:
        return None

    try:
        process = psutil.Process(os.getpid())
        return process.memory_info().rss
    except Exception:
        return None


def get_version(cmd_prefix: list[str], project_path: Path) -> str:
    """Get vectorcode version string."""
    cmd = [*cmd_prefix, "--version"]
    try:
        result = _run_subprocess(cmd, project_path, timeout=30.0)
        if result.returncode == 0:
            return result.stdout.strip()
    except Exception:
        pass
    return "unknown"


def generate_report(
    metadata: dict[str, Any],
    metrics: dict[str, Any],
    per_query: list[dict[str, Any]],
) -> dict[str, Any]:
    """Build the final report dict."""
    return {
        "metadata": metadata,
        "metrics": metrics,
        "per_query": per_query,
    }


# ---------------------------------------------------------------------------
# Main orchestration
# ---------------------------------------------------------------------------


def main() -> int:
    """Run the Phase 1 benchmark: 3 iterations x 50 queries."""
    print("=" * 60)
    print("  VectorCode Phase 1 IR Precision Benchmark")
    print("=" * 60)

    # Setup
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    cmd_prefix = find_vectorcode()
    print(f"\nCommand prefix: {' '.join(cmd_prefix)}")

    provider = os.environ.get("VECTORCODE_PROVIDER", "onnx")
    print(f"Embedding provider: {provider}")

    # Load dataset
    dataset = load_dataset(DATASET_PATH)
    print(f"Dataset: {len(dataset)} queries")

    if not _HAS_PSUTIL:
        print("WARNING: psutil not available — RAM metric will be null")

    # Get version
    version = get_version(cmd_prefix, REPO_ROOT)
    print(f"VectorCode version: {version}")

    # Storage for all iterations
    all_cold_index_times: list[float] = []
    all_latencies: list[float] = []
    all_p1: list[int] = []
    all_p3: list[int] = []
    all_p5: list[int] = []
    per_query_records: list[dict[str, Any]] = []
    peak_ram_bytes: int | None = None
    iterations_succeeded = 0

    for iteration in range(1, NUM_ITERATIONS + 1):
        print(f"\n{'─' * 50}")
        print(f"  Iteration {iteration}/{NUM_ITERATIONS}")
        print(f"{'─' * 50}")

        iter_latencies: list[float] = []
        iter_p1: list[int] = []
        iter_p3: list[int] = []
        iter_p5: list[int] = []

        try:
            # Cold index: init + index
            print("  Running init...")
            init_time = run_init(cmd_prefix, REPO_ROOT)
            print(f"  Init: {init_time:.2f}s")

            print("  Running index...")
            index_time = run_index(cmd_prefix, REPO_ROOT)
            print(f"  Index: {index_time:.2f}s")

            cold_time = init_time + index_time
            all_cold_index_times.append(cold_time)
            print(f"  Cold index total: {cold_time:.2f}s")

        except RuntimeError as e:
            print(f"  ERROR during init/index: {e}", file=sys.stderr)
            print("  Skipping this iteration.", file=sys.stderr)
            continue

        # Search burst
        print(f"  Running {len(dataset)} searches...")
        ram_before = sample_ram()

        for i, entry in enumerate(dataset):
            query = entry["query"]
            expected = entry["expected_file"]

            results, latency = run_search(
                cmd_prefix, REPO_ROOT, query, SEARCH_LIMIT
            )
            latency_ms = latency * 1000.0
            iter_latencies.append(latency_ms)

            p1 = compute_precision(results, expected, 1)
            p3 = compute_precision(results, expected, 3)
            p5 = compute_precision(results, expected, 5)
            iter_p1.append(p1)
            iter_p3.append(p3)
            iter_p5.append(p5)

            # Progress indicator
            if (i + 1) % 10 == 0:
                print(f"    [{i + 1}/{len(dataset)}] done")

        ram_after = sample_ram()

        # Track peak RAM
        for ram_val in (ram_before, ram_after):
            if ram_val is not None:
                if peak_ram_bytes is None or ram_val > peak_ram_bytes:
                    peak_ram_bytes = ram_val

        # Accumulate
        all_latencies.extend(iter_latencies)
        all_p1.extend(iter_p1)
        all_p3.extend(iter_p3)
        all_p5.extend(iter_p5)
        iterations_succeeded += 1

        # Per-query records (accumulate across iterations)
        for i, entry in enumerate(dataset):
            # Find or create per-query record
            key = entry["query"]
            existing = next(
                (r for r in per_query_records if r["query"] == key), None
            )
            if existing is None:
                per_query_records.append(
                    {
                        "query": key,
                        "expected_file": entry["expected_file"],
                        "description": entry["description"],
                        "latencies_ms": [],
                        "p1_scores": [],
                        "p3_scores": [],
                        "p5_scores": [],
                        "status": "ok",
                    }
                )
                existing = per_query_records[-1]

            existing["latencies_ms"].append(iter_latencies[i])
            existing["p1_scores"].append(iter_p1[i])
            existing["p3_scores"].append(iter_p3[i])
            existing["p5_scores"].append(iter_p5[i])

    # ── Aggregate metrics ──────────────────────────────────────────────
    lat_median, lat_p95 = aggregate_stats(all_latencies)
    cold_median, cold_p95 = aggregate_stats(all_cold_index_times)

    p_at_1 = sum(all_p1) / len(all_p1) if all_p1 else 0.0
    p_at_3 = sum(all_p3) / len(all_p3) if all_p3 else 0.0
    p_at_5 = sum(all_p5) / len(all_p5) if all_p5 else 0.0

    peak_ram_mb = (
        round(peak_ram_bytes / (1024 * 1024), 1) if peak_ram_bytes else None
    )

    metrics = {
        "cold_index_time_median_s": round(cold_median, 2),
        "cold_index_time_p95_s": round(cold_p95, 2),
        "search_latency_median_ms": round(lat_median, 2),
        "search_latency_p95_ms": round(lat_p95, 2),
        "precision_at_1": round(p_at_1, 4),
        "precision_at_3": round(p_at_3, 4),
        "precision_at_5": round(p_at_5, 4),
        "peak_rss_mb": peak_ram_mb,
    }

    # Finalize per-query records with aggregates
    for rec in per_query_records:
        med, _ = aggregate_stats(rec["latencies_ms"])
        rec["median_latency_ms"] = round(med, 2)
        rec["p1"] = round(sum(rec["p1_scores"]) / len(rec["p1_scores"]), 4) if rec["p1_scores"] else 0
        rec["p3"] = round(sum(rec["p3_scores"]) / len(rec["p3_scores"]), 4) if rec["p3_scores"] else 0
        rec["p5"] = round(sum(rec["p5_scores"]) / len(rec["p5_scores"]), 4) if rec["p5_scores"] else 0
        # Remove raw lists from output
        del rec["latencies_ms"]
        del rec["p1_scores"]
        del rec["p3_scores"]
        del rec["p5_scores"]

    metadata = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "vectorcode_version": version,
        "embedding_provider": provider,
        "iterations": NUM_ITERATIONS,
        "iterations_succeeded": iterations_succeeded,
        "dataset_size": len(dataset),
        "search_limit": SEARCH_LIMIT,
    }

    report = generate_report(metadata, metrics, per_query_records)

    # ── Write report ───────────────────────────────────────────────────
    with open(REPORT_PATH, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)

    # ── Print summary ──────────────────────────────────────────────────
    print(f"\n{'=' * 60}")
    print("  RESULTS SUMMARY")
    print(f"{'=' * 60}")
    print(f"  Iterations succeeded: {iterations_succeeded}/{NUM_ITERATIONS}")
    print(f"  Embedding provider:   {provider}")
    print(f"  Dataset size:         {len(dataset)} queries")
    print()
    print(f"  Cold index time (median): {metrics['cold_index_time_median_s']:.2f}s")
    print(f"  Cold index time (P95):    {metrics['cold_index_time_p95_s']:.2f}s")
    print()
    print(f"  Search latency (median):  {metrics['search_latency_median_ms']:.2f} ms")
    print(f"  Search latency (P95):     {metrics['search_latency_p95_ms']:.2f} ms")
    print()
    print(f"  Precision@1:  {metrics['precision_at_1']:.2%}")
    print(f"  Precision@3:  {metrics['precision_at_3']:.2%}")
    print(f"  Precision@5:  {metrics['precision_at_5']:.2%}")
    print()
    if peak_ram_mb is not None:
        print(f"  Peak RSS:     {peak_ram_mb:.1f} MB")
    else:
        print("  Peak RSS:     N/A (psutil unavailable)")
    print()
    print(f"  Report written to: {REPORT_PATH}")
    print(f"{'=' * 60}")

    return 0


# ---------------------------------------------------------------------------
# Unit tests
# ---------------------------------------------------------------------------


def _test_load_dataset(tmp_path: Path) -> None:
    """Test load_dataset with valid, invalid, and missing files."""
    # Valid dataset
    valid = tmp_path / "valid.json"
    valid.write_text(
        json.dumps(
            [
                {
                    "query": "test query",
                    "expected_file": "src/main.rs",
                    "description": "test",
                }
            ]
        )
    )
    result = load_dataset(valid)
    assert len(result) == 1
    assert result[0]["query"] == "test query"
    print("  PASS: load_dataset valid file")

    # Missing file
    try:
        load_dataset(tmp_path / "nonexistent.json")
        assert False, "Should have raised FileNotFoundError"
    except FileNotFoundError:
        print("  PASS: load_dataset missing file raises FileNotFoundError")

    # Missing key
    invalid = tmp_path / "invalid.json"
    invalid.write_text(json.dumps([{"query": "test"}]))
    try:
        load_dataset(invalid)
        assert False, "Should have raised ValueError"
    except ValueError as e:
        assert "missing keys" in str(e)
        print("  PASS: load_dataset missing key raises ValueError")


def _test_compute_precision() -> None:
    """Test compute_precision at various positions."""
    results = [
        {"file_path": "src/a.rs"},
        {"file_path": "src/b.rs"},
        {"file_path": "src/c.rs"},
        {"file_path": "src/d.rs"},
        {"file_path": "src/e.rs"},
    ]

    # Found at position 1 (k=1)
    assert compute_precision(results, "src/a.rs", 1) == 1
    print("  PASS: precision found at position 1, k=1")

    # Found at position 3 (k=3)
    assert compute_precision(results, "src/c.rs", 3) == 1
    print("  PASS: precision found at position 3, k=3")

    # Found at position 5 (k=5)
    assert compute_precision(results, "src/e.rs", 5) == 1
    print("  PASS: precision found at position 5, k=5")

    # Not found (k=3, file at position 5)
    assert compute_precision(results, "src/e.rs", 3) == 0
    print("  PASS: precision not found when beyond k")

    # Absent entirely
    assert compute_precision(results, "src/missing.rs", 5) == 0
    print("  PASS: precision absent file returns 0")

    # Empty results
    assert compute_precision([], "src/a.rs", 5) == 0
    print("  PASS: precision empty results returns 0")


def _test_aggregate_stats() -> None:
    """Test aggregate_stats with various inputs."""
    # Odd count
    median, p95 = aggregate_stats([1.0, 2.0, 3.0, 4.0, 5.0])
    assert median == 3.0, f"Expected 3.0, got {median}"
    assert p95 == 5.0, f"Expected 5.0, got {p95}"
    print("  PASS: aggregate_stats odd count")

    # Even count
    median, p95 = aggregate_stats([1.0, 2.0, 3.0, 4.0])
    assert median == 2.5, f"Expected 2.5, got {median}"
    print("  PASS: aggregate_stats even count")

    # Single value
    median, p95 = aggregate_stats([42.0])
    assert median == 42.0
    assert p95 == 42.0
    print("  PASS: aggregate_stats single value")

    # Empty
    median, p95 = aggregate_stats([])
    assert median == 0.0
    assert p95 == 0.0
    print("  PASS: aggregate_stats empty list")


def run_unit_tests() -> None:
    """Run all unit tests."""
    import tempfile

    print("\nRunning unit tests...")

    with tempfile.TemporaryDirectory() as tmp:
        _test_load_dataset(Path(tmp))

    _test_compute_precision()
    _test_aggregate_stats()

    print("All unit tests passed!\n")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    if "--test" in sys.argv:
        run_unit_tests()
        sys.exit(0)

    sys.exit(main())
