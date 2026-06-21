#!/usr/bin/env bash
# scripts/verify-baseline.sh — clone-and-verify the committed baselines.
#
# Exits 0 if every baseline passes against a re-run, non-zero otherwise
# (exit 2 for regression, exit 1 for harness error). This is the script CI
# calls in .github/workflows/benchmark.yml.
#
# Requirements: Rust toolchain, no other deps. Runs in <30s on a developer
# laptop and <60s on a CI runner.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> Building release binary"
cargo build --release

echo "==> Verifying baseline-mock-mini.json (IR quality)"
cargo run --release -- benchmark \
    --corpus mock-mini \
    --mock-embedder \
    --compare benchmarks/baseline/baseline-mock-mini.json

echo
echo "==> Verifying baseline-mock-mini-structural.json (structural IR)"
cargo run --release -- benchmark \
    --corpus mock-mini \
    --mock-embedder \
    --queries benchmarks/queries/mock-mini-structural.toml \
    --compare benchmarks/baseline/baseline-mock-mini-structural.json

echo
echo "==> Verifying baseline-store-mock-mini.json (store performance)"
cargo run --release -- bench-store \
    --corpus mock-mini \
    --mock-embedder \
    --output json \
    --query-sample 0 \
    --compare benchmarks/baseline/baseline-store-mock-mini.json

echo
echo "All baselines verified."
