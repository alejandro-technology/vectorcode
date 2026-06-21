# P7 — Honestidad sobre el estado

> Verdict: **40% (pre-commit) → measured again after this commit** — this document is the act that raises P7's verdict. Writing per-pilar docs with `file:line` citations and explicit limits IS the honesty work; before this commit P7 was ~40% because the visible docs (`README.md`, `roadmap_vectorcode.md`) contradicted the code in five places.

## Verdict

This document is meta: it measures how honest the project is about its own state. The honest answer before this commit was "less honest than the ADRs and baselines would suggest, because the most-visible docs (README, roadmap) are out of date". Five stale claims were live in the repo:

1. `README.md` "Supported Languages" listed 7 languages; the code supports 14 (`src/engine/languages.rs:6-22`).
2. `README.md` "MCP Tools" listed 5 tools; the code exposes 8 (`src/mcp/handler.rs` has `vec_search`, `vec_status`, `vec_reindex`, `vec_read_lines`, `vec_outline`, `vec_find_callers`, `vec_find_dependents`, `vec_trace_imports`).
3. `README.md:336` referenced `benchmarks/results/phase1_report.json`; the directory is `.gitignore`d.
4. `roadmap_vectorcode.md:29-41` (the live "Estado actual" block after the C2 rewrite) had listed 4 features as `❌` that are all `✅` today (BM25/FTS, knowledge graph, multi-repo, formal benchmark); the original pre-Fase-1 snapshot is preserved verbatim at `roadmap_vectorcode.md:156-159` for the audit trail.
5. `roadmap_vectorcode.md:92` (row 3.3) said "Hashing de archivos (SHA-256)"; the code uses `blake3` (`src/types.rs:127`, `src/types.rs:134`) — fixed by the same C2 edit, which now reads "Hashing de archivos (BLAKE3 — `src/types.rs:127` y `:134`)".

This commit fixes all five. Reading them in the new `docs/STATUS.md` plus the seven `docs/pilar-status/P{1..7}.md` should now give a new contributor an honest answer to "how complete is this project?" within 60 seconds.

## Evidence (what the project got right before this commit)

- **ADR-0001 is exemplary** — `docs/adr/0001-store-choice.md` carries a `Status: proposed` header (line 3), the "LanceDB honesty" section admits the shim (line 47-50), the bench table notes "Not measured (shim)" for LanceDB (line 100), and "Re-evaluation Conditions" sets a concrete re-evaluation date. This is the template every other doc should follow.
- **BASELINE.md admits limits plainly** — `BASELINE.md:31-36` shows TypeScript at 0% and Python at 20% on the mini corpus, with a one-line note that "the embedding model struggles with TS semantic search or the defu corpus is too small". No marketing-speak hedging.
- **`bench_store.rs` refuses to lie about the shim** — `src/bench/store_bench.rs:172-178` prints a warning and returns early when the harness would otherwise publish shim numbers as LanceDB numbers. The shim's own module header at `src/store/lancedb.rs:1-32` is unambiguous about the in-memory nature of the implementation.
- **Per-pilar template forces honesty** — every `P{n}.md` has a non-empty "Known limits" section. If a pilar has no known limit, that's the limit.

## Evidence (what was dishonest or stale before this commit — now fixed)

| Stale claim | Location (pre-commit) | Reality | Fixed in |
|-------------|-----------------------|---------|----------|
| 7 supported languages | `README.md:230-240` | 14 languages (`src/engine/languages.rs:6-22`) | [phase-4.3 commit C2](../../README.md) |
| 5 MCP tools | `README.md:242-296` | 8 tools (`src/mcp/handler.rs` 179, 367, 409, 480, 540, 600, 643, 690) | [phase-4.3 commit C2](../../README.md) |
| Reference to `benchmarks/results/phase1_report.json` | `README.md:336` | Directory is `.gitignore`d; results go to the regression gate at `benchmarks/baseline/` | [phase-4.3 commit C2](../../README.md) |
| 4 features marked `❌` in "Estado actual" | pre-C2 snapshot, now at `roadmap_vectorcode.md:156-159` (Historical snapshots appendix) | All 4 are `✅` (Fase 1 + 2 + 3 shipped); the live `Estado actual` block at `roadmap_vectorcode.md:29-41` reflects this | [phase-4.3 commit C2](../../roadmap_vectorcode.md) |
| "Hashing de archivos (SHA-256)" | pre-C2 `roadmap_vectorcode.md` row 3.3 | `blake3` (`src/types.rs:127`, `src/types.rs:134`); row 3.3 at `roadmap_vectorcode.md:92` now reads "BLAKE3 — `src/types.rs:127` y `:134`" | [phase-4.3 commit C2](../../roadmap_vectorcode.md) |

The original pre-Fase-1 "Estado actual" block is preserved verbatim as a [Historical snapshot](../../roadmap_vectorcode.md#historical-snapshots) appendix at the bottom of the roadmap. The audit trail stays; the in-place lie is removed.

## Known limits (the pilar where the limit IS the pilar)

- **No canonical status doc before this commit** — a new contributor had to read `README.md`, `roadmap_vectorcode.md`, `BASELINE.md`, `docs/adr/0001-store-choice.md`, and `docs/benchmarks.md` to reconstruct a per-pilar picture. `docs/STATUS.md` is the new index; the seven `P{n}.md` are the deep dives.
- **Stale-by-construction risk** — `docs/STATUS.md` will go stale the moment any pilar changes. Mitigation: link it from `README.md` and `roadmap_vectorcode.md`; review it in each release. The "How to update" callout at the bottom of `STATUS.md` is the procedure.
- **Per-pilar % invites bikeshedding** — the % is a 5-second guide, not a grade. The evidence and limits sections in each deep-dive carry the real weight. The `%` is a search-friendly signal, not a quality bar.
- **The "P7 is low" reading** — a reviewer who sees "P7 = 40%" might mistake the verdict for an admission of failure. It is the opposite: P7 measures honesty, and the honest answer at the start of phase-4.3 was "less honest than we'd like", because the visible docs contradicted the code. Writing this doc is the work that moves P7 upward; the next measurement (after this commit) is the real one.
- **No machine-checked link rot yet** — the `file:line` citations in the deep dives are verified manually during the commit; a CI grep step (`rg -n "(\\.\\./)+[\\w/]+\\.rs:\\d+" docs/pilar-status/`) would catch drift, but does not exist yet.
- **The roadmap appendix carries the old lie, on purpose** — keeping the pre-Fase-1 "Estado actual" verbatim in the "Historical snapshots" appendix is an audit-trail decision. Some readers will skim past `docs/STATUS.md` and read only the roadmap; for them, the appendix is the most honest thing we can offer. Others will read the appendix and wonder why the lie is still there. Trade-off accepted.

## Links

- [docs/STATUS.md](../STATUS.md) — the 7-row index this pilar feeds.
- [docs/adr/0001-store-choice.md](adr/0001-store-choice.md) — the model honest ADR; the pattern this pilar aims for across the whole project.
- [BASELINE.md](../../BASELINE.md) — exemplary honest-baseline doc; admits TypeScript 0% and Python 20% without hedging.
- [docs/benchmarks.md](../benchmarks.md) — the public verification guide.
- Related: [P1](P1-local-first.md) · [P2](P2-retrieval-medido.md) · [P3](P3-estructura-ast-grafo.md) · [P4](P4-escala-real.md) · [P5](P5-arquitectura-contrato.md) · [P6](P6-mcp-interfaz.md).
