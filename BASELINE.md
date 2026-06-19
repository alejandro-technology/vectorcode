# Baseline — Fase 1.2

VectorCode dense-only search quality baseline. Run against the curated
mini-corpus (3 repos: thiserror Rust, defu TypeScript, itsdangerous Python)
using the embeddinggemma:latest embedding model via Ollama on ARM (Apple Silicon).

## Run Info

| Field | Value |
|-------|-------|
| Date | 2026-06-19 |
| Embedder | Ollama / embeddinggemma:latest (768d) |
| Platform | ARM (Apple Silicon) |
| Corpus | mini (thiserror + defu + itsdangerous) |
| Files indexed | 18 |
| Chunks | 83 |
| Queries | 15 |
| Duration | ~13.4s |

## Aggregate Metrics

| Metric | Value |
|--------|-------|
| Recall@5 | 0.3000 |
| Recall@10 | 0.3000 |
| nDCG@10 | 0.2415 |
| MRR | 0.3500 |

## Per-Language Breakdown

| Language | Queries | R@5 > 0 | Best R@5 |
|----------|---------|---------|-----------|
| Rust (thiserror) | 5 | 5/5 | 1.0000 |
| TypeScript (defu) | 5 | 0/5 | 0.0000 |
| Python (itsdangerous) | 5 | 1/5 | 0.5000 |

## Notes

- Dense search with embeddinggemma performs well on Rust code (100% hit rate
  on thiserror queries) but poorly on TypeScript (0%) and mixed on Python (20%).
- The zero TypeScript results suggest the embedding model struggles with TS
  semantic search or the defu corpus is too small (3 files).
- This baseline will be used to measure improvement in Fase 1.3-1.6
  (sparse search, RRF fusion, reranker).

## Reproducibility

```bash
# Requires: Ollama running with embeddinggemma:latest
ollama pull embeddinggemma:latest
cargo run -- benchmark --corpus mini
```
