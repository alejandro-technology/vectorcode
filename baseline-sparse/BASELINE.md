# Benchmark Baseline

**Corpus**: mini
**Search Mode**: sparse
**Date**: 2026-06-19 22:53:06 UTC
**VectorCode Version**: 0.1.0

## Setup

- Files indexed: 18
- Chunks created: 83
- Queries executed: 15
- Duration: 7.74s

## Aggregate Metrics

| Metric | Value |
|--------|-------|
| Recall@5 | 0.0333 |
| Recall@10 | 0.0333 |
| nDCG@10 | 0.0469 |
| MRR | 0.0667 |

## Reproducibility

Run this benchmark again with: `cargo run --release -- benchmark --corpus mini`

Expected variance: ±0.01 across 3 runs on ARM (REQ-BENCH-005).
