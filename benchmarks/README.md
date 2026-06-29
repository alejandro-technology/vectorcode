# Benchmarks — orientation

This directory is the canonical home of the VectorCode benchmark suite.
The published baseline numbers live in `BASELINE.md` (one level up from
this directory, in the repo root) and the regression-gate baseline JSON
files live in `baseline/`. The verification path is a single command:

```bash
bash scripts/verify-baseline.sh
```

For a deeper walkthrough of how to read the numbers and what they mean,
see [`../docs/benchmarks.md`](../docs/benchmarks.md).

## Layout

```
benchmarks/
├── README.md            ← you are here
├── CONTRIBUTING.md      ← how to add / change golden queries
├── corpus.toml          ← corpus definitions (mock-mini, mini, vscode)
├── baseline/            ← committed regression-gate JSON files
│   ├── SCHEMA.md        ← contract between baselines and the comparator
│   ├── baseline-mock-mini.json
│   ├── baseline-mock-mini-structural.json
│   └── baseline-store-mock-mini.json
└── queries/             ← golden query sets, one per baseline
    ├── mock-mini.toml
    ├── mock-mini-structural.toml
    ├── mini.toml
    └── mini_structural.toml
```

## Corpora

| Corpus | Source | Purpose |
|--------|--------|---------|
| `mock-mini` | `tests/fixtures/mini/` (4 small files) | Public verify path (phase 4.1). No network, no Ollama, no model download. |
| `mini` | 3 small GitHub repos (thiserror, defu, itsdangerous) | Larger integration smoke test. Requires network on first run. |
| `vscode` | `microsoft/vscode` sparse checkout | Scale benchmark (~15K files). Requires network. |

The `vscode.toml` placeholder query file that used to live under
`queries/` has been removed (its `grade = 0` entries could not gate any
regression). The real vscode corpus + query set arrives in phase 4.4.

## Quick path

```bash
# Verify against the committed baselines (CI uses this).
bash scripts/verify-baseline.sh

# Run the benchmarks without comparing, capturing JSON for inspection.
bash scripts/run-benchmarks.sh
# → benchmarks/results/benchmark-mock-mini-ir-dense.json
# → benchmarks/results/benchmark-mock-mini-structural-dense.json
# → benchmarks/results/bench-store-mock-mini.json
```

## Mock vs real

The mock-mini baselines are a **smoke test**, not a measure of real
retrieval quality. The mock embedder produces deterministic, but
semantically random, vectors. The mock-mini baselines are useful to
catch:

- Indexing pipeline regressions (a chunk that should appear in the
  index doesn't).
- Store performance regressions (indexing time, RSS, disk size).
- Schema drift in the comparator or the report format.

For real IR-quality numbers (thiserror, defu, itsdangerous, vscode with
a pinned model), see phase 4.4. The infrastructure added in 4.1 is
designed to be model-agnostic so 4.4 needs no code changes.

## Adding a baseline

1. Update or add a query file under `queries/`.
2. Run `bash scripts/run-benchmarks.sh` to capture the new JSON.
3. Edit the corresponding file under `baseline/` so it carries just the
   metric values (see `baseline/SCHEMA.md` for the shape).
4. Re-run `bash scripts/verify-baseline.sh` to confirm the new baseline
   passes against itself.
5. Open a PR. CI will gate on the new baseline from then on.

## Adding a query

See [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## Academic Taxonomy & Roadmap

Our benchmarking efforts align with formal research terminology for LLM agents and Augmented Retrieval Systems. Currently, the suite formally implements **Phase 1**, with subsequent phases under development:

| Fase | Nombre técnico aproximado | Qué mide | Estado |
|------|---------------------------|----------|--------|
| **1** | **Information Retrieval Benchmark** (Retrieval Evaluation) | Calidad del recuperador (Recall, Precision, etc.) | **Implementado** (Dense, Sparse, Hybrid, Graph) |
| **2** | **End-to-End Agent Benchmark** (Task-Oriented Agent Evaluation) | Eficiencia y capacidad del agente resolviendo problemas usando las herramientas. | **Implementado** (vectorcode corpus, SER=1.80, TER=1.29) |
| **3** | **Context Efficiency Benchmark** (Token Efficiency Evaluation) | Coste de contexto y escalabilidad del sistema RAG. | **Implementado** (vectorcode corpus, SER=1.80, TER=1.29) |
