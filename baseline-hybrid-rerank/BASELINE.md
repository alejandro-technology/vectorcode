# Benchmark Baseline

**Corpus**: mini
**Search Mode**: hybrid-rerank
**Date**: 2026-06-19 22:53:06 UTC
**VectorCode Version**: 0.1.0

## Setup

- Files indexed: 18
- Chunks created: 83
- Queries executed: 15
- Duration: 30.38s

## Aggregate Metrics

| Metric | Value |
|--------|-------|
| Recall@5 | 0.2000 |
| Recall@10 | 0.3667 |
| nDCG@10 | 0.1996 |
| MRR | 0.1951 |

## Reproducibility

Run this benchmark again with: `cargo run --release -- benchmark --corpus mini`

Expected variance: ±0.01 across 3 runs on ARM (REQ-BENCH-005).
