# P3 — Estructura del código como ciudadano de primera clase

> Verdict: **60%** — 14 languages are chunked via tree-sitter, the graph is built inline during indexing, and 3 of those 14 languages (Rust, TS/JS/TSX/JSX, Python) emit Call + Import edges. The remaining 40% is "11 languages are chunked but not graphed" and "Extends + Reference edges are typed in `EdgeType` but never emitted by the current queries".

## Verdict

VectorCode parses source code as an AST, not as text. The chunker uses one tree-sitter grammar per language, and the same parser feeds the graph extractor so symbols and edges land in the same database in one indexing pass. The chunker covers 14 languages; the graph extractor covers 3 (Rust, the TS/JS family, Python) and returns an empty edge list for everything else. Edge types `Call` and `Import` are the only ones currently emitted; `Extends` and `Reference` exist in the type system (`src/types.rs` `EdgeType` enum) and the SQL queries support them, but the tree-sitter queries never produce them. There is no cross-language graph benchmark (Fase 2.6 only ran on the structural mini corpus).

## Evidence

- **14 chunked languages** — `src/engine/languages.rs:6-22` defines the `SupportedLanguage` enum with 14 variants + `Unknown`: TypeScript, Tsx, JavaScript, Jsx, Python, Rust, Go, Java, CSharp, C, Cpp, Ruby, Swift, Kotlin. Extension dispatch is at `:26-45`.
- **Graph built inline during indexing** — `src/engine/indexer.rs:469-486` shows the fast path: `compute_content_hash` then a `(mtime, size, hash)` short-circuit; only on change does the chunker run, and the graph extraction is in the same pass.
- **Graph extraction limited to 3 languages** — `src/engine/graph_extractor.rs:6-24` dispatches on `SupportedLanguage`; the match returns `(Vec::new(), Vec::new())` (empty nodes/edges) for everything outside Rust, the TS/JS family, and Python. The line `:23` is the explicit `_ => return (Vec::new(), Vec::new()), // Unsupported for now` arm.
- **3 MCP graph tools** — `src/mcp/handler.rs:600` `vec_find_callers`, `:643` `vec_find_dependents`, `:690` `vec_trace_imports`. The graph port itself is `src/store/graph.rs:6-14` (`GraphStore` trait, 7 methods).
- **Call + Import are the only emitted edge types in practice** — the queries in `src/engine/graph_extractor.rs` (constants `RUST_EDGES_QUERY`, `TS_EDGES_QUERY`, `PYTHON_EDGES_QUERY` lower in the same file) only emit `EdgeType::Call` and `EdgeType::Import`. `EdgeType::Extends` and `EdgeType::Reference` are defined in `src/types.rs` (the `EdgeType` enum) and the SQL at `src/store/graph.rs` (and `src/store/db.rs` migration) accepts them, but no tree-sitter query produces them today.
- **Cross-language mini benchmark** — `benchmarks/queries/mini_structural.toml` exercises 12 structural queries against 3 repos (thiserror, defu, itsdangerous). Fase 2.6 reported Symbol R@5 = 1.0 and P@5 = 0.65 on this corpus.

## Known limits

- **11 of 14 languages are chunked but not graphed** — Java, Go, C, C++, C#, Ruby, Swift, Kotlin, plus the `_` catch-all, all return empty edges at `src/engine/graph_extractor.rs:23`. A user indexing a Java project gets chunks but no graph.
- **`Extends` and `Reference` edges never produced** — the SQL inserts them fine; the tree-sitter queries just never emit them. Implementing these for Rust (impl / use paths) and TS/JS (extends / type references) is straightforward and would close a chunk of the gap.
- **No cross-file resolution of late-bound symbols** — graph nodes are stored by `blake3(file_path)` plus the symbol name; identical symbol names in different files produce different node IDs. `get_callers` does a late-join by `target_symbol`, but cross-file resolution depends on string match, not on resolved binding.
- **No cross-language graph benchmark** — the structural mini corpus has 3 repos × 3 languages. A Java or Go graph benchmark does not exist; claims about graph quality on those languages are unverified.
- **No metric for "false positive callers"** — the structural mini reports Symbol R@5 and P@5 but does not penalize symbolic noise (e.g. `unwrap` showing up as a caller of everything). A precision-with-graded-judgments metric would help.

## Links

- [BASELINE.md](../../BASELINE.md) — Fase 1.2 baseline; Fase 2 structural numbers are summarized in the git history (`feat(graph)` commit).
- [ADR-0001](adr/0001-store-choice.md) — store choice; the graph is stored in SQLite today.
- Related: [P4](P4-escala-real.md) (the graph scales linearly with chunks; see O(N) KNN limit) · [P5](P5-arquitectura-contrato.md) (the `GraphStore` port) · [P6](P6-mcp-interfaz.md) (the graph tools are MCP-only, no CLI yet).
