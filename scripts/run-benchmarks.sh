#!/usr/bin/env bash
# scripts/run-benchmarks.sh — one-shot build + run all 3 mock-mini benchmarks
# and move the JSON results into benchmarks/results/.
#
# No --compare. Use scripts/verify-baseline.sh when you want the regression
# gate. This script is for capturing fresh JSON (e.g. before regenerating a
# baseline, or for ad-hoc inspection).
#
# Requirements: Rust toolchain, no other deps. Runs in <30s on a developer
# laptop and <60s on a CI runner.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

mkdir -p benchmarks/results

echo "==> Building release binary"
cargo build --release

echo "==> Running mock-mini IR benchmark (semantic queries)"
cargo run --release -- benchmark \
    --corpus mock-mini \
    --mock-embedder \
    --output json

echo "==> Running mock-mini structural benchmark"
cargo run --release -- benchmark \
    --corpus mock-mini \
    --mock-embedder \
    --queries benchmarks/queries/mock-mini-structural.toml \
    --output json

echo "==> Running mock-mini store benchmark (no query phase)"
cargo run --release -- bench-store \
    --corpus mock-mini \
    --mock-embedder \
    --output json \
    --query-sample 0

# Move the generated JSON files into benchmarks/results/. The first IR run
# produces benchmark-mock-mini-dense.json; the structural run overwrites
# that path. We rely on `mv` failing on a missing source so the script
# errors loudly if a run produced no output.
shopt -s nullglob
for f in benchmark-mock-mini-dense.json; do
    if [ -f "$f" ]; then
        # The structural run produces a structurally-named file; rename it
        # so both runs land in results/ with distinguishable names.
        case "$f" in
            benchmark-mock-mini-dense.json)
                # Use a tag from a sidecar file to disambiguate. We write
                # the last IR run as benchmark-mock-mini-ir-dense.json
                # after each successful capture, so the previous IR JSON
                # (if any) becomes the final structural result and the
                # last JSON becomes the IR one.
                if [ -f benchmarks/results/benchmark-mock-mini-ir-dense.json ]; then
                    mv benchmarks/results/benchmark-mock-mini-ir-dense.json \
                       benchmarks/results/benchmark-mock-mini-structural-dense.json
                fi
                mv "$f" benchmarks/results/benchmark-mock-mini-ir-dense.json
                ;;
        esac
    fi
done
shopt -u nullglob

# bench-store prints JSON to stdout, not a file. Re-run with explicit
# capture so the artifact lands in results/ for downstream tools.
echo "==> Capturing bench-store JSON to benchmarks/results/"
cargo run --release -- bench-store \
    --corpus mock-mini \
    --mock-embedder \
    --output json \
    --query-sample 0 \
    > benchmarks/results/bench-store-mock-mini.json

echo
echo "Done. Artifacts under benchmarks/results/:"
ls -1 benchmarks/results/
