# Baseline JSON Schema (`benchmarks/baseline/*.json`)

This document is the contract between the committed baselines under
`benchmarks/baseline/` and the `benchmark --compare` / `bench-store --compare`
flow. CI gates PRs on these comparisons.

## Files

| File | What it gates |
|------|---------------|
| `baseline-mock-mini.json` | IR quality on the 4-file mock-mini corpus (`tests/fixtures/mini`). 4 IR metrics (recall@5, recall@10, nDCG@10, MRR) with `Absolute(0.01)` tolerance. |
| `baseline-mock-mini-structural.json` | Structural (symbol-level) quality on the mock-mini corpus. 3 symbol metrics (symbol_recall@5, symbol_recall@10, symbol_precision@5). |
| `baseline-store-mock-mini.json` | Store performance on the mock-mini corpus. 4 store metrics (indexing_secs, peak_rss_bytes, disk_size_bytes, query_p95_ms) with relative tolerances (see below). |

## Common header

Every baseline file shares this top-level shape:

```json
{
  "version": "1",
  "corpus": "<corpus-name>",
  "embedder": "<provider-name>",
  "generated_at": "<ISO-8601 timestamp>"
}
```

- `version` — schema version. Currently `"1"`. Bump only on a breaking
  structural change to the JSON shape.
- `corpus` — must match the `--corpus` argument of the run that consumes
  the baseline.
- `embedder` — must match the `provider_name()` of the embedder used at
  generation. The compare path uses this to detect accidental model
  drift between baseline capture and the current run.
- `generated_at` — informational.

## Metric payloads

Each baseline type carries one of three metric payloads. The keys and units
are stable; the comparator is hard-coded to them.

### IR (`baseline-mock-mini.json`)

```json
{
  "search_mode": "dense",
  "metrics": {
    "recall_at_5": 0.85,
    "recall_at_10": 0.92,
    "ndcg_at_10": 0.64,
    "mrr": 0.58
  }
}
```

### Structural IR (`baseline-mock-mini-structural.json`)

```json
{
  "search_mode": "dense",
  "metrics": {
    "symbol_recall_at_5": 0.0,
    "symbol_recall_at_10": 0.0,
    "symbol_precision_at_5": 0.0
  }
}
```

### Store (`baseline-store-mock-mini.json`)

```json
{
  "store": {
    "indexing_secs": 0.017,
    "peak_rss_bytes": 27426816,
    "disk_size_bytes": 4096,
    "query_p50_ms": 0.0,
    "query_p95_ms": 0.0
  }
}
```

`query_p50_ms` is captured for visibility but not currently part of the
regression gate.

## Tolerance policy

The comparator is implemented in `src/bench/verdict.rs::compare_to_baseline`.
The policy is per-metric:

| Metric | Tolerance | Rationale |
|--------|-----------|-----------|
| `recall_at_5`, `recall_at_10`, `ndcg_at_10`, `mrr` | `Absolute(0.01)` | IR metrics move in 0.01-sized steps when the ranking changes by one position. The boundary `|delta| == 0.01` is a regress (strict less-than). |
| `symbol_recall_at_5`, `symbol_recall_at_10`, `symbol_precision_at_5` | `Absolute(0.01)` | Same shape as the IR metrics. |
| `indexing_secs` | `Relative(0.5)` | Indexing time may grow by up to +50% before tripping the gate. Allows CI runner noise. |
| `query_p95_ms` | `Relative(1.0)` | Query p95 may grow by up to +100%. Catches "store is 2x slower" regressions but tolerates single-runner spikes. |
| `peak_rss_bytes`, `disk_size_bytes` | `Relative(0.2)` | Memory and disk regressions trip the gate at +20%. |

When a baseline value is exactly 0 (e.g. `query_p95_ms` captured with
`--query-sample 0`), the relative tolerance collapses to strict equality:
any non-zero current value is a regress. This protects the "no query
phase" guarantee from silently going un-gated.

Missing baseline field in the current report → regress.
NaN or ±Inf on either side → regress.

## Exit codes from the CLI

| Code | Meaning |
|------|---------|
| 0 | All metrics pass. |
| 1 | Error (bad baseline path, non-deterministic embedder, harness failure). |
| 2 | At least one metric regressed beyond its tolerance. |
| 75 | `bench-store` only: SLO was violated (legacy code, kept for back-compat). When `--compare` is set, this is folded into exit code 2. |

## Update policy

Regenerate the baselines when:

- The mock embedder implementation changes (vector hashing algorithm,
  normalization, etc.).
- The mock-mini fixture files (`tests/fixtures/mini/`) change.
- The query set for mock-mini or mock-mini-structural changes.
- The metric definitions change (a new metric is added to the report).

For real-corpus baselines (thiserror / defu / itsdangerous / vscode) the
regeneration policy is identical but also requires a model pin update.
Real-corpus verification is phase 4.4 and out of scope here.

To regenerate, run from the project root:

```bash
# IR
cargo run --release -- benchmark --corpus mock-mini --mock-embedder --output json
# Structural
cargo run --release -- benchmark --corpus mock-mini --mock-embedder \
    --queries benchmarks/queries/mock-mini-structural.toml --output json
# Store
cargo run --release -- bench-store --corpus mock-mini --mock-embedder \
    --output json --query-sample 0
```

Each output is the raw `BenchmarkResult` / `StoreMetricsReport` JSON; the
baselines commit a slimmer shape (just the metric values, not the
per-query detail). The transformation is documented in
`src/bench/verdict.rs::IrBaseline::to_benchmark_result` and friends.
