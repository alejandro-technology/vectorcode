# P5 — Arquitectura como contrato, no como decoración

> Verdict: **90%** — 7 ports are defined with at least one impl each: `Embedder` (6 prod + 1 test), `SearchStrategy` (4), `Store` + `StoreFactory` (2 — `SqliteStore` production, `LanceStore` in-memory SHIM), `Reranker` (1), `GraphStore` (1), `Corpus` (3). The contract test suite in `tests/store_contract.rs` verifies both backends. The remaining 10% is the `LanceStore` being an in-memory SHIM, not real LanceDB, and `GraphStore` being composed on `Database` rather than on `Store` (so the LanceStore-shim path has no graph).

## Verdict

VectorCode defines its ports before its implementations. Every external concern (embedding, search strategy, persistence, reranking, graph, corpus) is a Rust trait with at least one production impl, and the engine talks to the trait, not the concrete type. The phase-3 store evaluation completed the `Store` port refactor; ADR-0001 records the call-site reduction as a side benefit ("cascading changes through 53+ call sites", per `docs/adr/0001-store-choice.md:76`). The honest limits are: `LanceStore` is an in-memory shim that satisfies the trait contract for the eval harness but does not exercise the real `lancedb` crate, the engine still has raw `db.conn()` reach-throughs in the free functions under `src/store/{chunks,files,fts,vectors,meta,graph}.rs`, and `GraphStore` is composed from `Store` (via `Store::graph()` at `src/store/store.rs:84`) but its concrete impl lives on `Database`, so swapping `Store` to LanceDB-shim also means losing graph queries.

## Evidence

| Port | Trait location | Impl count | Impl locations |
|------|----------------|------------|----------------|
| `Embedder` | `src/embedder/mod.rs:26` | 6 prod + 1 test | `OnnxEmbedder` `src/embedder/onnx.rs:242` · `OllamaEmbedder` `src/embedder/ollama.rs:243` · `OpenAiEmbedder` `src/embedder/openai.rs:112` · `GeminiEmbedder` `src/embedder/gemini.rs:169` · `OpenRouterEmbedder` `src/embedder/openrouter.rs:127` · `MockEmbedder` `src/embedder/mock.rs:53` (+ `MockDeterministicEmbedder` `src/embedder/mock.rs:97` for the bench harness) |
| `SearchStrategy` | `src/engine/searcher.rs:71` | 4 | `DenseSearcher` `src/engine/searcher.rs:202` · `SparseSearcher` `src/engine/sparse_searcher.rs:36` · `HybridSearcher` `src/engine/fusion.rs:134` · `GraphRetriever` `src/engine/graph_retriever.rs:64` |
| `Store` + `StoreFactory` | `src/store/store.rs:32` + `:109` | 2 | `SqliteStore` (production) · `LanceStore` (in-memory SHIM, feature-gated behind `--features lancedb-store`) |
| `Reranker` | `src/reranker/mod.rs:27` | 1 | `OnnxReranker` `src/reranker/onnx.rs` (BGE-Reranker-v2-m3, ~571 MB) |
| `GraphStore` | `src/store/graph.rs:6` | 1 | `Database` (impl on the concrete database, not on `Store`) |
| `Corpus` | `src/bench/corpus.rs:25,115,243` | 3 | `LocalCorpus` · `GitCorpus` · `MultiCorpus` |

- **Contract test suite** — `tests/store_contract.rs` verifies the 14-method `Store` contract for both `SqliteStore` and `LanceStore`. Run: `cargo test --test store_contract`.
- **`LanceStore` is an in-memory SHIM** — `src/store/lancedb.rs:1-32` module header is explicit: the real LanceDB dep tree is not pulled in the default build; the shim satisfies the trait contract for the eval harness. `src/bench/store_bench.rs:172-178` prints a warning and refuses to publish shim numbers as LanceDB numbers.
- **Raw `db.conn()` reach-throughs** — the ADR notes "53+ call sites" (`docs/adr/0001-store-choice.md:22` and `:76`). The engine hot path now goes through `Store`, but the free functions in `src/store/{chunks,files,fts,vectors,meta,graph}.rs` still take `&rusqlite::Connection` directly. A full migration would touch those free functions, not the engine.
- **`GraphStore` is composed, not on `Store`** — `src/store/store.rs:84` is `fn graph(&self) -> &dyn GraphStore;`. The concrete `GraphStore` impl is on `Database` (in `src/store/graph.rs`), so the `LanceStore`-shim path has no graph until a separate `LanceGraphStore` is built.

## Known limits

- **`LanceStore` is a shim, not real LanceDB** — anyone running `cargo run -- bench-store --backend lancedb` sees fake numbers on a fake backend. The shim is the honest eval path; the production path is sqlite-vec. The CLI's bench-store path explicitly warns (per `src/bench/store_bench.rs:172-178`) and `LanceStore`'s module header is unambiguous.
- **Raw `db.conn()` reach-throughs in free functions** — `Store` is the engine's port; `chunks::insert_chunk`, `files::upsert_file`, `fts::insert_fts_entry`, `vectors::search_similar`, `meta::write_meta`, `graph::insert_nodes` all still take `&Connection`. Migrating them to take `&dyn Store` is a pure refactor; not done in this phase.
- **`GraphStore` only on `Database`** — a LanceDB migration would lose graph queries until a `LanceGraphStore` exists. ADR-0001 says so.
- **`RerankDocument` is a value type, not a port** — there is only one reranker impl, so the trait is forward-looking. Not a defect, just a note for whoever adds a second reranker.
- **No plugin / dynamic loading** — adding a new embedder requires editing `src/cli/mod.rs:142-199` and `src/embedder/mod.rs`. A `libloading`-based plugin story would be additive, not in scope here.

## Links

- [ADR-0001](adr/0001-store-choice.md) — the "Store port refactor pays off" decision; cites the 53+ call-site reduction.
- [docs/benchmarks.md](../benchmarks.md) — how to run the contract tests + the bench-store harness.
- Related: [P1](P1-local-first.md) (the `Embedder` port) · [P3](P3-estructura-ast-grafo.md) (the `GraphStore` port) · [P4](P4-escala-real.md) (the `Store` port at vscode scale).
