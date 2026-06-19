# Benchmark Baseline

**Corpus**: mini
**Date**: Pending first run
**VectorCode Version**: 0.1.0

## Status

⚠️ **Baseline not yet established**

This file will be populated after running the benchmark with the ONNX embedder on an ARM runner.

## How to Generate Baseline

```bash
# Build release binary
cargo build --release

# Run benchmark with baseline output
cargo run --release -- benchmark --corpus mini --output baseline
```

This will:
1. Clone the mini-corpus repos (thiserror, p-limit, itsdangerous)
2. Index files with OnnxEmbedder
3. Execute golden queries
4. Write results to `BASELINE.md` and `results.json`

## Expected Metrics (Fase 1.2)

After establishing the baseline, we expect:
- **Recall@5**: 0.5-0.7 (dense embeddings on small corpus)
- **Recall@10**: 0.7-0.9
- **nDCG@10**: 0.6-0.8
- **MRR**: 0.5-0.8

These are initial estimates. Actual values depend on:
- ONNX model quality (all-MiniLM-L6-v2)
- Query set composition
- Corpus file diversity

## Reproducibility

Per REQ-BENCH-005, running the benchmark 3 times on the same ARM runner should yield metrics within ±0.01 variance.

## Next Steps

- [ ] Run baseline on `macos-14` (ARM) runner
- [ ] Record metrics in this file
- [ ] Save to engram for historical tracking
- [ ] Set tolerance thresholds for regression detection (Fase 1.6)
