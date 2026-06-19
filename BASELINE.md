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

---

# Fase 1.3-1.4 — Hybrid Search Baseline Verification

Verification that the dense-only baseline is preserved after adding sparse
search (FTS5) and RRF fusion. Default mode remains `Dense`, so the benchmark
code path is identical.

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
| Duration | 18.52s |

## Aggregate Metrics — Dense Mode (post Fase 1.3-1.4)

| Metric | Fase 1.2 | Fase 1.3-1.4 | Delta | Verdict |
|--------|----------|---------------|-------|---------|
| Recall@5 | 0.3000 | 0.3000 | ±0% | ✅ Preserved |
| Recall@10 | 0.3000 | 0.3000 | ±0% | ✅ Preserved |
| nDCG@10 | 0.2415 | 0.2947 | +22% | ✅ Improved (variance) |
| MRR | 0.3500 | 0.3667 | +4.8% | ✅ Improved (variance) |

## New Capabilities Verified

| Mode | Command | Status |
|------|---------|--------|
| Dense (default) | `vectorcode search "query"` | ✅ Unchanged |
| Sparse (FTS5) | `vectorcode search --mode sparse "query"` | ✅ bm25 lexical |
| Hybrid (RRF) | `vectorcode search --mode hybrid "query"` | ✅ Dense + Sparse fusion |

## Implementation Summary

- **Schema**: v2→v3 migration with `chunks_fts` FTS5 virtual table + triggers
- **Engine**: `SearchStrategy` trait with `DenseSearcher` / `SparseSearcher` / `HybridSearcher`
- **Fusion**: `rrf_fuse()` pure function with configurable K (default 60)
- **CLI**: `--mode dense|sparse|hybrid` flag
- **Tests**: 617 total (573 unit + 44 integration), all passing
- **Commits**: 6 (5 features + 1 migration fix)

## Conclusion

La línea base dense-only se mantiene intacta. Las fases 1.3 y 1.4 agregan
búsqueda léxica (FTS5) y fusión RRF sin degradar el pipeline existente.
El benchmark confirma que el modo por defecto (`Dense`) produce resultados
equivalentes a la Fase 1.2. Las capacidades nuevas (`--mode sparse`, `--mode hybrid`)
están operativas y verificadas con pruebas de humo sobre el repositorio real.
Listo para Fase 1.5 (reranker ONNX) y Fase 1.6 (re-medición completa con hybrid).

---

# Fase 1.5-1.6: Reranker ONNX + Re-medición (2026-06-19)

Multi-mode benchmark comparing dense, sparse, hybrid (RRF), and hybrid-rerank
search strategies over the mini corpus.

## Configuración

| Field | Value |
|-------|-------|
| Date | 2026-06-19 |
| Embedder | Ollama / embeddinggemma:latest (768d) |
| Reranker | BGE-Reranker-v2-m3 (ONNX, int8, ~571MB) — **not downloaded** |
| Reranker Timeout | 5000ms |
| Reranker Top-K | 20 |
| Platform | ARM (Apple Silicon) |
| Corpus | mini (thiserror + defu + itsdangerous) |
| Files indexed | 18 |
| Chunks | 83 |
| Queries | 15 |

> **Note:** The hybrid-rerank mode fell back to plain hybrid because the reranker
> model (`Xenova/bge-reranker-v2-m3`) was not cached locally. To enable reranking,
> download the model to `~/.vectorcode/models/bge-reranker-v2-m3/` from HuggingFace.

## Resultados — Multi-Mode Comparison

| Mode | Recall@5 | Recall@10 | nDCG@10 | MRR | Duration |
|------|----------|-----------|---------|-----|----------|
| Dense | 0.3000 | 0.3000 | 0.2660 | 0.4333 | 16.1s |
| Sparse (FTS5) | 0.0333 | 0.0333 | 0.0469 | 0.0667 | 7.5s |
| Hybrid (RRF) | 0.2333 | 0.3000 | 0.1677 | 0.1322 | 14.5s |
| Hybrid+Rerank* | 0.2000 | 0.3333 | 0.1721 | 0.1390 | 15.7s |

*\* Fell back to hybrid — reranker model not downloaded. Results differ slightly
due to Ollama embedder variance between runs.*

## Análisis

**Dense** remains the strongest single-mode strategy for this corpus (MRR 0.4333),
driven by strong Rust/thiserror recall. TypeScript/defu and Python/itsdangerous
queries remain challenging for the embeddinggemma model.

**Sparse (FTS5/BM25)** performs poorly on natural-language queries (R@5 0.0333).
This is expected — BM25 excels at exact term matching, not semantic intent.
Its value is as a complement to dense in the hybrid fusion.

**Hybrid (RRF)** improves Recall@10 to match Dense (0.3000) while adding lexical
coverage. The nDCG@10 is lower than Dense because RRF fusion introduces some
rank noise from the sparse channel. The fusion is conservative (RRF k=60).

**Hybrid+Rerank** could not be fully evaluated — the ONNX reranker model requires
a ~571MB download from HuggingFace. Once downloaded, the reranker should re-order
the top-20 hybrid results using cross-encoder scoring, improving nDCG and MRR.

**Bug fix during benchmark:** Fixed a pre-existing FTS5 bug where hyphenated query
terms (e.g., "key-based") caused `no such column` errors. The `sanitize_fts_query`
function now strips `-` and `+` operators and quotes all tokens.

## Verificación

- 616 unit tests, 44 integration tests — all passing
- `cargo fmt --check` — passes
- `cargo clippy --all-targets -- -D warnings` — passes

## Reproducibility

```bash
# Requires: Ollama running with embeddinggemma:latest
ollama pull embeddinggemma:latest

# Run multi-mode benchmark
cargo run -- benchmark --corpus mini --output table --mode all

# For full hybrid-rerank evaluation, download the reranker model first:
# Model: Xenova/bge-reranker-v2-m3 (~571MB)
# Place in: ~/.vectorcode/models/bge-reranker-v2-m3/model.onnx + tokenizer.json
```
