# Benchmarks — verification guide

This guide is the single source of truth for *how to verify the
benchmark numbers* published in [`../BASELINE.md`](../BASELINE.md).
It is written for a developer who has never run a VectorCode benchmark
before and wants to reproduce the published numbers in under 5 minutes.

> **TL;DR.** A Rust toolchain is the only prerequisite. The
> deterministic mock-mini path runs in <30s on a developer laptop and
> <60s on a CI runner. No Ollama, no API keys, no model download.

## Quick path

```bash
# 1. Clone the repo.
git clone https://github.com/alejandro-technology/vectorcode
cd vectorcode

# 2. Verify every committed baseline in one shot.
bash scripts/verify-baseline.sh
# → exit 0: all baselines match a re-run.
# → exit 2: a regression tripped the gate.
# → exit 1: harness error (bad baseline, missing toolchain, etc.).
```

If you want to capture a fresh JSON instead of comparing to a baseline:

```bash
bash scripts/run-benchmarks.sh
ls benchmarks/results/
# → benchmark-mock-mini-ir-dense.json
# → benchmark-mock-mini-structural-dense.json
# → bench-store-mock-mini.json
```

## Prerequisites

- **Rust 1.75+** (`rustc --version`). The benchmark CLI links only into
  the standard library + the dependencies already in `Cargo.toml`.
- A POSIX shell (bash) for the scripts.
- ~200 MB of free disk for the release build artifacts.

You do **not** need:

- Ollama, ONNX runtime setup, or a model download.
- API keys for Gemini, OpenAI, OpenRouter.
- A GPU.
- Network access (the mock-mini corpus is the local
  `tests/fixtures/mini/` directory).

## What the verify script does

The script runs three comparisons against the committed baselines under
`benchmarks/baseline/`:

1. **IR quality** — `benchmark --corpus mock-mini --mock-embedder
   --compare baseline-mock-mini.json`. Compares recall@5, recall@10,
   nDCG@10, MRR for 12 semantic queries over the 4-file mock-mini
   fixture.
2. **Structural IR** — `benchmark --corpus mock-mini --mock-embedder
   --queries mock-mini-structural.toml --compare
   baseline-mock-mini-structural.json`. Compares symbol-level metrics
   for 5 structural queries.
3. **Store performance** — `bench-store --corpus mock-mini
   --mock-embedder --query-sample 0 --compare
   baseline-store-mock-mini.json`. Compares indexing wall-clock, peak
   RSS, and on-disk size against the store SLO with a relative
   tolerance (per [`benchmarks/baseline/SCHEMA.md`](../benchmarks/baseline/SCHEMA.md)).

Each call writes a `delta-report.json` artifact next to the baseline
file and prints a human-readable table to stdout. Exit code 0 means
every metric is within tolerance; exit code 2 means at least one
metric regressed.

## Output format

A passing run prints something like:

```
Metric             Current   Baseline   Delta    Verdict
--------------------------------------------------------------
recall_at_5          0.8333     0.8333   +0.0000  pass
recall_at_10         0.8333     0.8333   +0.0000  pass
ndcg_at_10           0.6410     0.6410   +0.0000  pass
mrr                  0.5764     0.5764   +0.0000  pass
symbol_recall_at_5   0.0000     0.0000   +0.0000  pass
symbol_recall_at_10   0.0000     0.0000   +0.0000  pass
symbol_precision_at_5   0.0000     0.0000   +0.0000  pass

Overall: PASS
Delta report written to: benchmarks/baseline/delta-report.json
```

A regression prints the failing metric in the verdict column and a
list of reasons below the table.

## Metric ranges (what is "normal")

The mock-mini baselines are a **smoke test**, not a measurement of
retrieval quality. Their absolute numbers are not interesting on their
own — what matters is that they stay stable. Rough expectations:

- `recall_at_5` and `recall_at_10` typically land in `0.7`–`0.9`.
- `ndcg_at_10` typically lands in `0.5`–`0.7`.
- `mrr` typically lands in `0.5`–`0.6`.
- `indexing_secs` for 4 files is sub-second.
- `peak_rss_bytes` is ~25–35 MB.
- `disk_size_bytes` is ~4 KB.

The tolerances are tight (Absolute 0.01 for IR metrics) because the
mock embedder is fully deterministic; any change in rankings is a real
regression. If your machine is much slower than the baseline machine
and trips the store SLO, run with `RUST_LOG=debug` and look at the
delta table — the relative tolerances for store metrics are designed
to absorb ±50% (indexing) / ±100% (query p95) / ±20% (RSS, disk) CI
runner noise.

## Mock corpus is smoke-only — important caveat

The mock-mini baseline is **not** a published retrieval-quality
measurement. The mock embedder (`MockDeterministicEmbedder`) produces
deterministic but semantically random vectors. The 4-file corpus is
too small to give a stable estimate of precision, recall, or nDCG in
any meaningful sense.

What the mock-mini baseline *does* guarantee:

1. The indexing pipeline still produces the same chunks.
2. The store interface still satisfies the SLO within tolerance.
3. The comparator still parses the JSON shapes correctly.
4. The CI gate still fires on real regressions (e.g. a missing chunk
   in the indexer, a wrong tolerance, a unit test that no longer
   matches the schema).

For real IR-quality numbers with a real model, see phase 4.4. The
infrastructure added in 4.1 (the `--compare` flag, the comparator,
the SCHEMA, the scripts) is model-agnostic — phase 4.4 only needs to
commit new baselines and update the CI workflow to use them on a
heavier runner.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `comparison requires deterministic embeddings` | `--compare` is set but neither `--mock-embedder` nor a working configured provider is in use. | Pass `--mock-embedder` for the mock path, or fix the provider config. |
| `unable to load baseline: path does not exist` | The baseline file is missing or the path is wrong. | `ls benchmarks/baseline/` and verify the path passed to `--compare`. |
| `query_p95_ms: current=... baseline=0.0` regresses unexpectedly | The store baseline was captured with `--query-sample 0`. | The zero-baseline rule (see `SCHEMA.md`) treats any non-zero `query_p95_ms` as a regress. Re-run `scripts/run-benchmarks.sh` with a non-zero sample. |
| The store comparison regresses on a slow CI runner | Runner noise exceeded the +50% indexing / +100% p95 tolerance. | The tolerance is intentionally generous. If a real regression is hidden, the next baseline regen after the fix will lock in the new value. |
| The IR metrics look different from the published `BASELINE.md` | `BASELINE.md` captures historical numbers from a different model + corpus. The mock-mini baselines under `benchmarks/baseline/` are the regression gate. | This is expected. See [Mock vs real](#mock-vs-real-important-caveat) above. |

## What is gated in CI

`.github/workflows/benchmark.yml` runs `scripts/verify-baseline.sh` on
every PR. The PR is blocked when the script exits non-zero (regression
or error). The workflow also uploads `delta-report.json` artifacts so
the regression reason is visible in the Actions UI even when the PR is
closed.

If you intend to intentionally change a baseline (e.g. you added a new
query to the mock-mini set), update the JSON file in the same PR and
the gate will adapt.
