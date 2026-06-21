# P6 — MCP como interfaz, no como producto

> Verdict: **55%** — 8 MCP tools are exposed (3 of them graph-aware, added in phase 2) and 11 CLI commands are registered. The parity table below shows where each MCP tool maps to a CLI subcommand and where the gap is. The remaining 45% is the 4 MCP-only tools (`vec_read_lines`, `vec_find_callers`, `vec_find_dependents`, `vec_trace_imports`), the cross-workspace aggregation in `vec_status` (MCP-only), and the `routing=auto` heuristic that chooses dense vs. graph per query (MCP-only).

## Verdict

VectorCode ships an MCP server and a CLI; the two are not full peers. The MCP surface is the rich one: 8 tools (`vec_search`, `vec_status`, `vec_reindex`, `vec_read_lines`, `vec_outline`, `vec_find_callers`, `vec_find_dependents`, `vec_trace_imports`) defined in `src/mcp/handler.rs`. The CLI surface is broader on orchestration (init, install, uninstall, upgrade, benchmark, bench-store) and narrower on read operations. There is no `vec_read_lines` CLI subcommand, no `vec_find_*` CLI subcommand, and no `vec_trace_imports` CLI subcommand — graph-aware and snippet-aware reads are MCP-only. Multi-workspace users get a cross-workspace `vec_status` only via MCP (`src/mcp/handler.rs:367` aggregates `AppInnerState` across all `workspaces`); the CLI `vectorcode status` is per-project. The `routing=auto` heuristic that picks dense vs. graph per query lives in the MCP path (`src/engine/router.rs`); CLI users must force `--mode graph` explicitly.

## Evidence

- **8 MCP tools** — `src/mcp/handler.rs:179` `vec_search`, `:367` `vec_status`, `:409` `vec_reindex`, `:480` `vec_read_lines`, `:540` `vec_outline`, `:600` `vec_find_callers`, `:643` `vec_find_dependents`, `:690` `vec_trace_imports`.
- **11 CLI subcommands** — `src/cli/mod.rs:50-73` (the `Commands` enum): `init`, `index`, `search`, `outline`, `status`, `serve`, `install`, `uninstall`, `upgrade`, `benchmark`, `bench-store`.
- **MCP↔CLI parity table** — see below.
- **Cross-workspace `vec_status`** — `src/mcp/handler.rs:367` aggregates across `AppInnerState` for every entry in `state.workspaces`. The CLI `vectorcode status` resolves one project root via `src/cli/mod.rs:103-129` and never sees the multi-workspace map.
- **`routing=auto` heuristic** — `src/engine/router.rs` exists and is used by the MCP `vec_search` path. CLI users can `--mode graph | dense | sparse | hybrid | hybrid-rerank` (per `src/cli/mod.rs:111-114` in the README usage block) but cannot delegate the choice to the engine.

### Parity table

| MCP tool            | CLI equivalent                       | Notes                                                             |
|---------------------|--------------------------------------|-------------------------------------------------------------------|
| `vec_search`        | `vectorcode search`                  | Full parity (CLI supports `--mode`, `--limit`, `--threshold`, …). |
| `vec_status`        | `vectorcode status`                  | CLI is per-project; MCP aggregates across workspaces.             |
| `vec_reindex`       | `vectorcode index --full`            | Flag, not a subcommand. CLI also has `vectorcode index` (incremental). |
| `vec_outline`       | `vectorcode outline <file>`          | Full parity.                                                      |
| `vec_read_lines`    | **none**                             | MCP-only. CLI has no `read_lines` subcommand.                      |
| `vec_find_callers`  | **none**                             | MCP-only. CLI has no `find_callers` subcommand.                    |
| `vec_find_dependents` | **none**                           | MCP-only.                                                          |
| `vec_trace_imports` | **none**                             | MCP-only.                                                          |

## Known limits

- **4 MCP tools have no CLI equivalent** — `vec_read_lines`, `vec_find_callers`, `vec_find_dependents`, `vec_trace_imports`. A user who wants to script a graph traversal or read a 50-line snippet from the shell cannot do it without an MCP client.
- **Cross-workspace status is MCP-only** — multi-repo users (the fase-3 deliverable) get the cross-workspace view only via MCP. The CLI `vectorcode status` is single-root.
- **`routing=auto` is MCP-only** — the engine knows when a question is structural ("who calls X?") vs. conceptual ("where is auth handled?"), but the CLI forces the user to pick. The CLI does not have a `--mode auto` flag.
- **MCP `vec_status` schema is JSON-string-typed** — it returns a `String` (JSON-encoded) rather than a structured response. Agents parse it manually. The 8 tools have the same shape; this is a constraint of the `rmcp` macro shape.
- **No streaming in MCP** — long searches block until done. A streaming `vec_search` is a non-trivial addition.

## Links

- [docs/benchmarks.md](../benchmarks.md) — the bench harness exercises the same code path the CLI uses (`cargo run -- benchmark`), independent of MCP.
- [ADR-0001](adr/0001-store-choice.md) — the `Store` port makes both the CLI and the MCP server share the same hot path.
- Related: [P3](P3-estructura-ast-grafo.md) (the 3 graph tools come from P3's graph port) · [P5](P5-arquitectura-contrato.md) (the `SearchStrategy` port makes the CLI↔MCP parity trivial in principle).
