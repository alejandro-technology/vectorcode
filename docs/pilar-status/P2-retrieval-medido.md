# P2 — Retrieval que se mide, no que se siente

> Verdict: **70%** — the harness exists (`src/bench/` 8 modules), the CLI works (`cargo run -- benchmark --corpus mini`), ADR-0001 is exemplary, and `scripts/verify-baseline.sh` is the regression gate. The remaining 30% is "no public cross-tool comparison yet" (→ 4.4) and the silent mock fallback that warns the user but still produces numbers.

## Verdict

VectorCode treats retrieval quality as a measured property, not a vibe. The benchmark harness lives in `src/bench/` with 8 modules covering corpus loading, metric computation, reports, runners, schemas, verdicts, and a parameterized store benchmark. There are two runnable CLI commands (`benchmark` and `bench-store`), three golden query sets (`mini.toml`, `vscode.toml`, `mini_structural.toml`, plus the `mock-mini*` regression baselines), and an ADR (0001) that records the real-vs-shim store verdict with reproducible commands. The phase-4.1 commit added `scripts/verify-baseline.sh` as a <30s regression gate. The open work is honest: no comparison against external tools (deferred to 4.4) and a `MockEmbedder` fallback in the non-`--compare` path that still produces numbers (the CLI prints a warning, but a careless user can publish them as if real).

## Evidence

- **`src/bench/` has 8 modules** — `corpus.rs`, `metrics.rs`, `report.rs`, `runner.rs`, `schema.rs`, `verdict.rs`, `store_bench.rs`, and `mod.rs`. Run `ls src/bench/`.
- **Two runnable CLI commands** — `src/cli/benchmark.rs` (`vectorcode benchmark --corpus <mini|vscode|all>`) and `src/cli/bench_store.rs` (`vectorcode bench-store --corpus <name> --backend <sqlite-vec|lancedb>`). Both are registered in `src/cli/mod.rs:50-73` (the `Commands` enum).
- **Mock silent fallback** — `src/cli/benchmark.rs:121-139` (the `corpora_to_run` arm and the embedder construction): the non-`--compare` path falls back to `MockEmbedder` and prints a warning, but the benchmark still runs. The `--compare` path is strict (`src/cli/benchmark.rs:126-129`): it requires `--mock-embedder` to be set explicitly OR a real provider to load successfully.
- **Reproducible regression gate** — `scripts/verify-baseline.sh` (committed in phase-4.1) runs the mock-mini baselines against committed JSON under `benchmarks/baseline/` and exits non-zero on regression. CI gate lives in `.github/workflows/`.
- **Golden query sets** — `benchmarks/queries/{mini.toml, vscode.toml, mini_structural.toml, mock-mini.toml, mock-mini-structural.toml}`. Each query has hand-labeled relevance judgments (grades 0-3) per `benchmarks/CONTRIBUTING.md`.
- **ADR-0001 carries real numbers** — `docs/adr/0001-store-choice.md:82-94` shows the bench-store invocation, and the verdict table at `:98-...` records sqlite-vec at **3.15 s for 2 138 files / 14 863 chunks** and notes that LanceDB was not measured (shim). Honest disclosure lives next to the decision.
- **`BASELINE.md` admits limits** — Fase 1.2 baseline (`BASELINE.md:21-27`) shows R@5 = 0.30 with TypeScript at 0% and Python at 20%. The doc says so plainly, with no empty superlatives.

## Known limits

- **No committed baseline JSON under `benchmarks/results/`** — that directory is `.gitignore`d (`/benchmarks/results/` in `.gitignore`). Reproducers must re-run. The committed regression gate is `benchmarks/baseline/`, which is a different (faster, mock-based) artifact.
- **Mock fallback still produces numbers** — the warning at `src/cli/benchmark.rs:121` is easy to miss in CI logs. A `--strict` mode that hard-fails when a real provider is missing would close this.
- **Mini corpus is small** — 3 repos, 15 queries, 83 chunks (`BASELINE.md:11-17`). Easy to over-claim from this. The structural mini (12 queries) and the mock-mini baselines are also tiny.
- **No public cross-tool comparison** — the claim "hybrid + reranker improves nDCG by 47%" is measured against our own `hybrid` baseline, not against `codesearch` / `mcp-vector-search` / etc. This is phase-4.4 work, deliberately out of scope here.
- **No CI gate on search-quality regression** — the phase-3.6 CI gate covers latency, not nDCG/Recall. A drop in nDCG from 0.2415 → 0.18 would not fail CI today.
- **`vscode` corpus run is slow** — bench-store on vscode at full `--query-sample 100` is O(N) brute-force KNN; the ADR itself notes the operation is slow. `--query-sample 10` is the practical cap.

## Links

- [BASELINE.md](../../BASELINE.md) — the canonical Phase 1.2 numbers (R@5 = 0.30, nDCG = 0.24).
- [benchmarks/baseline/SCHEMA.md](../../benchmarks/baseline/SCHEMA.md) — the per-metric tolerance policy for the regression gate.
- [ADR-0001](adr/0001-store-choice.md) — store-choice verdict with sqlite-vec at 3.15 s for 14 863 chunks.
- [docs/benchmarks.md](../benchmarks.md) — the public verification guide (how to re-run).
- Related: [P5](P5-arquitectura-contrato.md) (the `Store` port makes the bench possible) · [P7](P7-honestidad.md).
