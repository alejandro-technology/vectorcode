# Project Status

VectorCode ships seven pillars (local-first retrieval, measured IR quality,
AST + code graph, real scale, port-shaped architecture, MCP as interface,
honest state). This page is the single index for per-pilar verdicts: each
row points to a deep-dive that lists evidence with `file:line` citations and
explicit known limits. The `Verdict %` is a 5-second summary, not a grade —
the evidence and limits sections in each deep-dive carry the real weight.

Last reviewed: 2026-06-21 (phase-4.3 commit, build `fbf07e0`).

| #  | Pilar                        | Verdict % | Summary                                                              | One known limit                                                              | Deep dive                                          |
| -- | ---------------------------- | --------: | -------------------------------------------------------------------- | ----------------------------------------------------------------------------- | -------------------------------------------------- |
| P1 | Local-first, sin excepciones |     85 %  | 6 embedder providers; default = ONNX; `ApiKeyMissing` hard-fail path  | First-run ONNX + reranker fetch from HF CDN; no `--list-providers`            | [P1](pilar-status/P1-local-first.md)              |
| P2 | Retrieval que se mide        |     70 %  | `src/bench/` harness; `cargo run -- benchmark --corpus mini`         | Mock silent fallback; no public cross-tool comparison (→ 4.4)                 | [P2](pilar-status/P2-retrieval-medido.md)          |
| P3 | Estructura AST + grafo       |     60 %  | 14 chunked languages; 3 graphed (Rust/TS-JS/Python); Call+Import edges | 11 langs chunked-not-graphed; Extends+Reference not emitted                   | [P3](pilar-status/P3-estructura-ast-grafo.md)      |
| P4 | Escala real                  |     80 %  | Incremental indexing with `blake3`; multi-repo `BTreeMap` workspaces  | sqlite-vec KNN is O(N); ~14 s/query extrapolated at 14 863 vectors (ADR-0001)  | [P4](pilar-status/P4-escala-real.md)              |
| P5 | Arquitectura como contrato   |     90 %  | 7 ports (Embedder 7, SearchStrategy 4, Store 2, Reranker 1, etc.)     | `LanceStore` is an in-memory SHIM; GraphStore lives on `Database`, not Store  | [P5](pilar-status/P5-arquitectura-contrato.md)    |
| P6 | MCP como interfaz            |     55 %  | 8 MCP tools × 11 CLI commands; `routing=auto` heuristic MCP-only     | 4 MCP tools have no CLI equivalent (`vec_read_lines`, `vec_find_*`, etc.)     | [P6](pilar-status/P6-mcp-interfaz.md)              |
| P7 | Honestidad sobre el estado   |     40 %  | ADR-0001 exemplary; `bench_store.rs:172-178` shim warning            | This index is the work that raises P7 — measure again after this commit        | [P7](pilar-status/P7-honestidad.md)                |

> **How to update this page.** Bump `Verdict %` when a pilar's deep-dive
> changes. Each `P{n}.md` is independently editable; re-run the
> anti-marketing grep over `docs/pilar-status/` before committing to keep
> the tone honest.
