# P4 — Escala real, no escala de demo

> Verdict: **80%** — incremental indexing with `blake3` content hashes is real, multi-repo serve uses a `BTreeMap<PathBuf, AppInnerState>` keyed by project root, and the footprint numbers from ADR-0001 (16-213 MB RSS, 60 MB on-disk) are measured on the vscode corpus. The remaining 20% is sqlite-vec KNN being O(N) brute-force (the ADR itself notes ~14 s/query extrapolated at 14 863 vectors) and the schema-migration story being non-backwards-compatible.

## Verdict

VectorCode scales beyond a single repo. Incremental re-indexing is real: every chunk has a `blake3` content hash (`src/types.rs:133-135`) and a derived chunk id (`src/types.rs:125-128`), and the indexer short-circuits on `(mtime, size, hash)` match before doing any work (`src/engine/indexer.rs:471-486`). Multi-repo serve is a `BTreeMap` of workspaces with per-result `repo_name` annotation (`src/mcp/mod.rs:37`, `src/types.rs:114`, `src/mcp/handler.rs` annotates each tool result). Footprint is measured, not estimated: ADR-0001 reports 16-17 MB RSS for ONNX/Ollama on the mini corpus, and 213 MB RSS / 60 MB on-disk for sqlite-vec on the full vscode corpus. The honest limits are sqlite-vec KNN being O(N) — the ADR itself labels the extrapolated latency "unacceptable" at vscode scale — and a non-backwards-compatible schema migration. **Hashing is BLAKE3, not SHA-256** as the roadmap line 85 claims; the code is correct, the roadmap is wrong, and this doc fixes the roadmap in [phase-4.3 commit C2](../../roadmap_vectorcode.md).

## Evidence

- **blake3 content hash** — `src/types.rs:133-135` `compute_content_hash` uses `blake3::hash(content.as_bytes())`. The chunk id at `:125-128` is `blake3("{file_path}:{byte_start}:{byte_end}")`. `Cargo.toml` has `blake3 = "1"` under `# Hashing`.
- **Incremental short-circuit** — `src/engine/indexer.rs:471-486`: after the canonical-path check, the indexer reads the existing file record and short-circuits when `mtime == mtime && size == size && hash == content_hash`. Only on change does it parse + chunk.
- **Multi-repo `BTreeMap`** — `src/mcp/mod.rs:37` `pub workspaces: Arc<tokio::sync::RwLock<BTreeMap<PathBuf, AppInnerState>>>`. `AppInnerState` (`:23-30`) carries `db`, `embedder`, `config`, `project_path`, and optional `watcher`. The `BTreeMap` (not `HashMap`) was the explicit fix from phase-4.2 to make iteration order deterministic.
- **Per-result `repo_name`** — `src/types.rs:114` adds `pub repo_name: Option<String>` to `SearchResult`; the MCP handlers annotate each tool result with the originating workspace.
- **Footprint numbers** — `BASELINE.md:13` records peak RSS at 16-17 MB for ONNX / Ollama on the mini corpus. ADR-0001 (table at `docs/adr/0001-store-choice.md:98-100`) records sqlite-vec at **3.15 s indexing for 2 138 files / 14 863 chunks**, peak RSS 213 MB, 60 MB on-disk.
- **LanceDB was not measured (shim)** — ADR-0001 explicitly states the LanceDB column is a shim. The verdict is "stay with sqlite-vec", conditional on re-evaluation at 10x chunks or 6 months.

## Known limits

- **Hashing is BLAKE3, not SHA-256** — `roadmap_vectorcode.md:92` (row 3.3) now reads "Hashing de archivos (BLAKE3 — `src/types.rs:127` y `:134`)". Phase-4.3 corrected the original "SHA-256" wording; the code was always `blake3` (`Cargo.toml` pins `blake3 = "1"`). See the historical snapshot appendix at `roadmap_vectorcode.md:154-160` for the pre-Fase-4.3 text.
- **sqlite-vec KNN is O(N) brute force** — `src/store/vectors.rs:193-220` shows the cosine SQL: `SELECT rowid, distance FROM vec_chunks WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2`. No ANN index. ADR-0001 measures **1.55 s/query at 1 668 vectors** and extrapolates **~14 s/query at 14 863 vectors** ("unacceptable" per the ADR's own words). The SLO is on indexing, not query, so the spec passes, but interactive search at vscode scale is not realistic.
- **One watcher per workspace** — multi-repo serve means N `notify` handles, N debounce timers. No cross-workspace coalescing. CPU cost grows linearly with the number of workspaces.
- **No published CI-runner profile** — Fase 3.5 said "validate that it runs on a standard CI runner", but the only measured platform is the local ARM Apple Silicon used for development. No GitHub Actions timing data is published.
- **Schema migration is destructive** — `docs/adr/0001-store-choice.md:59` notes v4 → v5 would re-index all users. The migration files in `src/store/db.rs` are not backwards-compatible; an old `.vectorcode/index.db` does not auto-upgrade.
- **Repo-name annotation is best-effort** — `SearchResult.repo_name` is `Option<String>` and the MCP path sets it; the CLI path does not always populate it. A consumer that filters by repo name from CLI may see `None`.

## Links

- [BASELINE.md](../../BASELINE.md) — Fase 1.2 footprint (16-17 MB RSS on mini).
- [ADR-0001](adr/0001-store-choice.md) — full store-choice table; sqlite-vec at 3.15 s / 14 863 chunks, O(N) caveat explicit.
- Related: [P3](P3-estructura-ast-grafo.md) (graph scales linearly with chunks) · [P5](P5-arquitectura-contrato.md) (the `Store` port + LanceDB shim).
