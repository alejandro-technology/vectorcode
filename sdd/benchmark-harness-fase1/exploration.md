# Exploration: Benchmark Harness — Fase 1.1 + 1.2

> **Status**: ready for proposal
> **Scope**: in-tree benchmark harness + baseline measurement of current dense search
> **Roadmap reference**: `roadmap_vectorcode.md` Fase 1, items 1.1 and 1.2
> **Out of scope**: sparse search (1.3), RRF (1.4), reranker (1.5) — those are 1.3+ and will reuse this harness

---

## 1. Current State — Search Pipeline (end-to-end)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  ENTRY POINTS                                                               │
├─────────────────────────────────────────────────────────────────────────────┤
│  CLI:  src/main.rs:23 → src/cli/search.rs:execute()                         │
│        args: query, limit, threshold, language, path, json                  │
│                                                                            │
│  MCP:  src/mcp/handler.rs:106 → vec_search tool                             │
│        params: VecSearchParams { query, limit, threshold, language, path }  │
│        cap: limit capped at 100                                            │
└──────────┬──────────────────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  SEARCHER  src/engine/searcher.rs:78  Searcher::search()                    │
├─────────────────────────────────────────────────────────────────────────────┤
│  1. enrich_query(query)              — prepend "code that " if <3 words   │
│  2. embedder.embed(enriched).await   — produces Vec<f32> of dims() length │
│  3. fetch_limit = if filters: limit*5 else limit (min 50)                 │
│  4. vectors::search_similar(conn, q, fetch_limit, threshold, path_pattern) │
│  5. results.retain(|r| r.language == lang)  (post-filter)                  │
│  6. results.truncate(options.limit)                                        │
└──────────┬──────────────────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  STORE  src/store/vectors.rs:149  vectors::search_similar()                 │
├─────────────────────────────────────────────────────────────────────────────┤
│  IF sqlite-vec available (the default path):                                │
│    L2-normalize query → f32 le blob                                        │
│    SELECT ... FROM vec_chunks WHERE embedding MATCH ?1 ORDER BY distance   │
│    JOIN chunk_vec_map ON vec_rowid JOIN chunks ON chunk_id                 │
│    WHERE c.file_path LIKE ?3 ESCAPE '\' (if path filter)                   │
│    score = 1.0 - distance (cosine → similarity in [0,1])                   │
│    filter by threshold                                                      │
│                                                                            │
│  ELSE fallback (brute-force):                                               │
│    SELECT chunk_id, embedding FROM vectors_data WHERE file_path LIKE ?1   │
│    cosine_similarity() per row  (src/store/vectors.rs:318)                  │
│    sort desc, truncate, hydrate chunk metadata                              │
└──────────┬──────────────────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  RESULT  Vec<SearchResult>  (src/types.rs:80)                                │
├─────────────────────────────────────────────────────────────────────────────┤
│  file_path: String                                                         │
│  start_line, end_line: u32                                                 │
│  symbol: Option<String>     (e.g. "Calculator.add")                         │
│  kind: String                (AST node kind, e.g. "function_declaration")  │
│  language: String            (e.g. "typescript")                            │
│  parent_context: Option<String>  (e.g. "class Calculator")                  │
│  content: String             (raw chunk text)                              │
│  score: f32                  (cosine similarity, higher = better)          │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Key observations for the benchmark:**
- Score is already normalized to `[0, 1]` (cosine sim) — no conversion needed.
- The pipeline is fully async (tokio); a benchmark runner can use `tokio::runtime::Runtime` or `#[tokio::test]`.
- The `path` filter in `SearchOptions` lets us narrow to a sub-corpus per query (useful for multi-project benchmarks later).
- The post-filter on `language` happens *after* KNN — so `limit` is not exact when language filter is active. For benchmark, pass `limit=10..20` and don't filter by language in the harness — just measure ranking quality.
- `enrich_query` adds "code that " to short queries — this is part of the system under test, so benchmarks must call `Searcher::search` (not skip it).

---

## 2. Store Schema Summary (src/store/db.rs)

| Table             | Purpose                                           | Key columns                                                                                                |
|-------------------|---------------------------------------------------|------------------------------------------------------------------------------------------------------------|
| `meta`            | Key-value store for index metadata                | `key`, `value` (provider, model, dimensions, created_at, last_sync_at, files_indexed, chunks_stored, vectorcode_version) |
| `chunks`          | One row per AST chunk                             | `id` (PK, blake3 hash), `file_path`, `start_line`, `end_line`, `byte_start`, `byte_end`, `symbol`, `kind`, `content`, `parent_context`, `language`, `file_mtime`, `content_hash` |
| `files`           | One row per indexed file (incremental sync)       | `path` (PK), `mtime`, `size`, `hash`, `indexed_at`                                                         |
| `vectors_data`    | Fallback vector storage (JSON)                    | `chunk_id` (PK, FK→chunks), `embedding` (JSON array string)                                                |
| `chunk_vec_map`   | Map chunk_id → sqlite-vec rowid                   | `chunk_id` (PK, FK→chunks), `vec_rowid`                                                                    |
| `vec_chunks`      | sqlite-vec virtual table (cosine distance)        | implicit `rowid`, `embedding float[N]`                                                                     |

Indexes on `chunks`: `file_path`, `symbol WHERE symbol IS NOT NULL`, `language`, `content_hash`.

**For relevance judgments** the benchmark needs to map a query's expected results to chunks/files. The natural key is `(file_path, start_line..end_line)` or simply `file_path` (file-level relevance). `symbol` is also available for fine-grained judgments.

**Schema version is 2** — no FTS5 table exists yet (Fase 1.3 will add it). For Fase 1.1/1.2 the dense-only path is exactly what we measure.

---

## 3. Test Infrastructure Assessment

### Existing patterns (use these as templates)

**Unit tests** live inline (`#[cfg(test)] mod tests`) in every src file. ~95%+ coverage of the existing code. They use:
- `tempfile::tempdir()` for ephemeral directories
- `Database::open_in_memory()` for the store
- `MockEmbedder::new(64)` (or 384) for deterministic embeddings
- `assert_cmd::Command::cargo_bin("vectorcode")` for CLI tests (dev-dep)
- `std::process::Command + Stdio::piped()` for MCP server tests

**Integration tests** in `tests/`:
- `chunker_integration_test.rs` — uses `tests/fixtures/sample_{ts,py,rs,cs,c,cpp,rb,swift,kt}/*.ext` (small hand-written samples, 30-50 lines each)
- `mcp_integration_test.rs` — boots the binary via `cargo run -- serve --mcp`, exchanges JSON-RPC over stdio

**`MockEmbedder` (`src/embedder/mock.rs`)** is the workhorse for tests:
- Deterministic (hash-of-text → f32 vector, L2-normalized)
- Configurable dimension
- `provider_name() = "mock"`, `model_name() = "mock-embedder"`
- **Limitation**: not semantic. For a benchmark we want real semantic similarity to be measurable, OR we can rely on the hash determinism to construct predictable top-k queries.

### Test/dev deps already in `Cargo.toml`
- `assert_cmd = "2"` — CLI testing
- `predicates = "3"` — assertions
- `serial_test = "3"` — env-var test isolation
- `http = "1"`
- `tempfile = "3"` (regular dep, available everywhere)

### What's missing for benchmarks
- **No metrics library** (no `ndcg`, no `recall` crate). Will implement inline (~60 lines).
- **No `goldenfile` / `insta`** for snapshot tests of metric values (could add, but inline `assert!` with explicit numbers is fine and more honest).
- **No `cargo bench` harness** (would need nightly). Integration tests via `cargo test --test benchmark_recall` are simpler and serve the same "this is the number" purpose for Fase 1.1/1.2. `benches/` (Criterion) makes sense later for latency.

### CodeGraph availability
`.codegraph/` exists with a populated `codegraph.db` (3.7MB) and active daemon (`daemon.pid`, `daemon.sock`). Used successfully in this exploration — gives huge speedup over grep+Read loops.

---

## 4. Indexing Flow (for foreign-corpus support)

The indexer (`src/engine/indexer.rs`) is path-agnostic — it can index any directory.

```rust
Indexer::new(db, embedder, IndexingConfig)
    .index_project(project_path)        // discovers + chunks + embeds + stores
    .index_files(&paths, project_path)  // incremental subset
```

Discovery uses `ignore::WalkBuilder` + extension filter (`has_supported_extension` → 12 supported languages).

**Important defaults** (`src/config/schema.rs:248`):
- `exclude_dirs` contains: `["benchmarks", "fixtures", "tests", ".vectorcode", ".git", "node_modules", "target", ...]`
- `exclude_extensions` contains: `[".min.js", ".map", ".lock", ".json", ".md", ".toml", ".txt", ...]`

**Implication for benchmark corpus location**: the corpus must NOT be under any of the excluded dirs. The cleanest options:
- Put corpus at `benchmarks/corpus/` and pass a custom `IndexingConfig::exclude_dirs` (the benchmark runner can build its own config and pass it to `Indexer::new` directly — no need to write to disk).
- OR put corpus at repo root in a non-excluded path (e.g., `benchmark_corpus/` — already in exclude_dirs! rename to `bench_corpus` or pass custom config).

**Recommendation**: build the benchmark runner so it constructs `Indexer` programmatically with a custom `IndexingConfig` (no excluded dirs), and uses an in-memory DB. This keeps the corpus on disk, indexable on demand, and never pollutes the user's actual project index.

---

## 5. Configuration Reuse for Benchmarks

`src/config/schema.rs` already accepts `name = "mock"` in the `valid_providers` list (line 68). However:
- The CLI `init` command's `ProviderArg` enum (`src/cli/mod.rs:71`) does NOT have a `Mock` variant — `vectorcode init --provider mock` doesn't exist.
- Workaround: write `config.toml` directly with `name = "mock"` (this is what `tests/mcp_integration_test.rs:77` does — `[provider]\nname = "mock"\n`).

**Cleanest approach for the benchmark harness**:
- Don't shell out to `vectorcode init` at all.
- Don't even shell out to `vectorcode search`.
- Build the harness as **library code** (a Rust binary in `src/bin/benchmark.rs` or a `tests/benchmark_*.rs` integration test) that uses `vectorcode::Database`, `vectorcode::engine::Indexer`, `vectorcode::engine::Searcher`, and `vectorcode::embedder::mock::MockEmbedder` directly.
- This is faster, deterministic, and doesn't require subprocess management.

**For Fase 1.2 baseline (with real embeddings)**:
- Use `OnnxEmbedder::from_cache_with_timeout()` — the model is bundled. Dimensions = 384. Deterministic for the same input.
- Or use `OllamaEmbedder` with a real Ollama instance (slower, requires setup, but measures the realistic "production" path).

---

## 6. Affected Areas (precise file list)

### Files to CREATE
| Path | Purpose |
|---|---|
| `benchmarks/corpus/**/*` | Mini multi-language corpus (hand-curated, ~20-30 files) |
| `benchmarks/queries.toml` | Golden set: queries + expected relevant files/chunks |
| `benchmarks/expected.toml` | Relevance judgments (separate for clarity) |
| `src/bench/mod.rs` | `pub mod metrics; pub mod runner;` |
| `src/bench/metrics.rs` | `recall_at_k`, `ndcg_at_k`, `mrr`, `precision_at_k` (~80 lines) |
| `src/bench/runner.rs` | Loads corpus, runs queries, aggregates metrics |
| `src/bench/golden.rs` | Loads/parses `benchmarks/queries.toml` + `expected.toml` |
| `tests/benchmark_recall.rs` | Integration test that runs the harness via library API |
| `docs/benchmarks.md` | How to run, how to add queries, what the numbers mean |
| `.github/workflows/benchmark.yml` | CI job: run benchmark on every PR |

### Files to MODIFY (minor)
| Path | Why |
|---|---|
| `src/lib.rs` | Add `pub mod bench;` (gate behind a feature flag if it pulls deps) |
| `Cargo.toml` | Add `[dev-dependencies] toml = "0.8"` is already in main deps; add `criterion = "0.5"` under `[[bench]]` if we go the Criterion route |
| `src/config/schema.rs` | No change needed (mock already valid) |
| `src/cli/init.rs` | Optional: add `ProviderArg::Mock` to enable `vectorcode init --provider mock` (nice-to-have, not required) |

### Files to READ but NOT modify
- `src/engine/searcher.rs` (consumer of benchmark)
- `src/store/vectors.rs` (returns `SearchResult`)
- `src/types.rs` (`SearchResult` shape)
- `src/embedder/mock.rs` (the test embedder)
- `src/embedder/onnx.rs` (real embedder for Fase 1.2 baseline)

---

## 7. Benchmark Harness — Design Options

### Option A — In-tree integration test (RECOMMENDED for Fase 1.1)
**Layout**: `tests/benchmark_recall.rs` + `src/bench/*.rs` modules
**Embedder**: `OnnxEmbedder` (bundled, offline, deterministic, 384d)
**Corpus**: hand-curated mini-repo under `benchmarks/corpus/` (20-30 files, 3-4 languages)
**Golden set**: `benchmarks/queries.toml` (query, expected_files, language_hint)
**Metrics**: `Recall@5`, `Recall@10`, `nDCG@10`, `MRR` (file-level relevance)
**CI**: runs on every PR via `cargo test --test benchmark_recall`
**Pros**: deterministic, fast (< 30s), zero external deps, version-controlled, easy to extend
**Cons**: small corpus = limited statistical power; ONNX-on-CPU is what we ship so it IS realistic
**Effort**: 2-3 days

### Option B — External public repo (NOT recommended for Fase 1.1)
**Layout**: same harness, corpus downloaded at runtime from github.com/microsoft/vscode or django/django
**Embedder**: Ollama or ONNX
**Corpus**: snapshot at a pinned commit (e.g., `vscode@v1.85.0`, ~150k files)
**Golden set**: hand-labeled 50-100 queries
**Pros**: realistic scale, recognizable for users
**Cons**:
- vscode is 150k+ files — indexing takes 10+ minutes even on a fast machine
- License/attribution concerns
- CI runner timeouts (GitHub free tier = 6h/job, but slow)
- Golden set labeling is days of work; not just "wrangle data"
- **The roadmap says "estilo RepoEval reducido"** = reduced/curated, not full
**Effort**: 1-2 weeks

### Option C — Hybrid (Fase 1.1 = Option A, Fase 4.1 = public release)
**Fase 1.1**: in-tree mini-corpus (Option A) — CI-gated, fast feedback
**Fase 4.1**: add public-corpus runner + comparison vs competitors (`flupkede/codesearch`, `mcp-vector-search`)
**Pros**: each phase stays scoped; the public release (Fase 4) is the one that "matters" for marketing; the in-tree one drives development
**Cons**: requires discipline to keep both paths maintained
**Effort**: A + ~1 week for the public runner

**Recommendation: Option A for Fase 1.1/1.2. Add Option C's public runner in Fase 4.1.**

---

## 8. Recommended Public-Repo / Test-Data Strategy

**The roadmap's "RepoEval reducido" hint is the key.** Don't try to replicate RepoEval (a 100GB+ dataset). Build a **fixed, version-controlled mini-corpus**:

**Proposal** (concrete, ship-ready):
- **Location**: `benchmarks/corpus/` (already gitignored-ish via `exclude_dirs`)
- **Size**: 20-30 files, ~50-100 chunks total
- **Languages**: TypeScript (10 files), Python (8 files), Rust (8 files) — covers the 3 most common MCP-target stacks
- **Domain**: A contrived but coherent "auth + payments + background jobs" sample app (e.g. `benchmarks/corpus/src/auth/login.ts`, `.../payments/charge.py`, `.../jobs/scheduler.rs`)
- **Golden set**: 30-50 queries
  - **Symbol queries** (easy): "authenticate user", "process charge", "retry failed job" → expect specific files/symbols
  - **Concept queries** (medium): "password hashing", "idempotent payment", "dead letter queue" → expect 1-3 files
  - **Cross-language queries** (hard): "background worker" → expect hits in all 3 languages
- **Relevance judgments** stored separately: `benchmarks/expected.toml` (not embedded in queries.toml) so a single query can have multiple "valid" relevant sets if we expand the corpus later

**Why not vscode/django as a "primary" test data**:
- They are excellent for **Fase 4 public release** (credibility, comparability with other tools).
- They are **terrible for CI** (size, time, determinism issues across git clones).
- The "queries with expected answers" problem is unsolved for them at the scale needed — RepoEval itself took a research team to build.

**Concrete next step**: design 3-5 sample files inline in the proposal to validate the chunker+embedder→top-k pipeline works as expected before investing in the full 30-file corpus.

---

## 9. Configuration Strategy

- The benchmark runner builds its own `IndexingConfig` and `SearchConfig` in code (no `config.toml` file needed for the library path).
- For `cargo test --test benchmark_recall`, no config files are touched on disk.
- If we later want a CLI `vectorcode benchmark` subcommand, it can read `benchmarks/queries.toml` and call the same library code.
- Embedder choice: hard-code `MockEmbedder` for unit tests, `OnnxEmbedder` for the "real" baseline, env-var switchable (`VECTORCODE_BENCH_EMBEDDER=onnx|mock|ollama`).

---

## 10. Risks & Blockers

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| 1 | No `ndcg`/`recall` crate in `Cargo.toml` | Low | Implement inline (~60 lines, no external dep needed) |
| 2 | `MockEmbedder` is hash-based, not semantic — top-k is not "semantically relevant" | Medium | Use `OnnxEmbedder` (bundled, 384d, deterministic) for the real baseline. Keep `MockEmbedder` only for unit tests of the harness itself (do the metrics code does the right thing) |
| 3 | `Indexer::index_project` excludes `benchmarks/`, `tests/`, `fixtures/` by default | Low | Build a custom `IndexingConfig { exclude_dirs: vec![] }` in the harness and pass it directly to `Indexer::new` |
| 4 | No `ProviderArg::Mock` variant in `init` CLI | Low | Don't use `init` for benchmarks — call library directly |
| 5 | `init_schema(dims)` is hard-fail on dimension mismatch (searcher) | Low | Pin dims in the corpus builder (always 384 for ONNX, always N for mock) |
| 6 | Golden set is hand-labeled → biased and small | Medium | Document the limitation; add 5-10 "obvious" queries (sanity) and 20+ "realistic" queries. The roadmap says "RepoEval reducido" — reduced = small, but should be principled |
| 7 | ONNX model download in CI | Low | The `model_manager` is in the user home (`~/.vectorcode/models/`). CI runners need to seed this once. Use a GitHub Actions cache, or commit a small mock model for tests |
| 8 | Benchmark value drift over time (chunking changes, embedder model changes) | Medium | Pin embedder model in `IndexMeta`; lock corpus file contents; record config hash in benchmark output |
| 9 | Path normalization across machines (CI vs local) | Low | Use relative paths everywhere in the corpus; canonicalize before comparison |
| 10 | No notion of "relevance grade" (binary vs graded) | Low | Start binary (relevant / not relevant). Add graded relevance in Fase 2.6 when needed |
| 11 | The `MockEmbedder` provider isn't in `ProviderArg` enum — must write config.toml manually | Low | Either (a) add `ProviderArg::Mock` (5-line change) or (b) call library directly bypassing CLI |

**No hard blockers** — all of these are addressable in-scope of Fase 1.1/1.2.

---

## 11. Ready for Proposal

**Yes** — the orchestrator can advance to the proposal phase (`sdd-propose`) with this exploration as input.

**What the proposal should answer**:
1. Concrete golden set (3-5 sample files + 10-15 queries to seed the corpus)
2. Exact `benchmarks/queries.toml` / `expected.toml` schemas
3. Metrics module API (`recall_at_k(&results, &expected, k) -> f64`)
4. How to run (`cargo test --test benchmark_recall -- --nocapture`)
5. How the baseline number (Fase 1.2) is reported (markdown table in CI output)
6. Definition of Done for Fase 1.1 (harness exists, all metrics unit-tested) and 1.2 (baseline numbers recorded, format documented)

**Skills the proposal should load**:
- `sdd-propose` (mandatory)
- `sdd-spec` (for writing the metrics requirements as scenarios)
- `sdd-tasks` (for the breakdown)
- `cognitive-doc-design` (for the `docs/benchmarks.md` doc that explains the harness)

---

## 12. Quick Sanity Check (one-shot experiment to run during proposal)

Before writing the full harness, do this 5-minute experiment:
1. Pick 2 small TS files (existing fixtures `tests/fixtures/sample_ts/calculator.ts` + a new `auth.ts`)
2. Build an in-memory DB with `init_schema(384)` + `OnnxEmbedder::from_cache_with_timeout()`
3. Index both files
4. Search for "authenticate user"
5. Confirm the top-3 results include `auth.ts`
6. Record the score and the chunk ID

If the experiment works, the harness is mechanical from here. If the score is weird (e.g. very low), we may need to tune the embedder choice or chunk enrichment (currently: `"{lang} | {file} | {parent} | {symbol}\n{content}"`).

---

*End of exploration. Topic key: `architecture/exploration-benchmark-harness-fase-1-1-1-2`*
