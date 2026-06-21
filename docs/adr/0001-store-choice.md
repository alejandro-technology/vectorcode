# 0001 — Stay with sqlite-vec, defer LanceDB migration

* Status: **proposed** (real vscode measurement complete; LanceDB column is shim, see `Re-evaluation Conditions`)
* Date: 2026-06-21
* Deciders: vectorcode maintainers
* Consulted: phase-3 store evaluation (3.1 + 3.2 SDD), exploration obs #66, remediation-pass apply-progress (post-archive)
* Supersedes: —

## Context and Problem Statement

The roadmap item 3.1 asked us to evaluate the vector/graph/lexical store at real-world
scale. The original intent was a three-way comparison (sqlite-vec vs LanceDB vs
Qdrant embedded) on a corpus the size of vscode (≤15K files, ~200K chunks at 384d).

Exploration phase ([obs #66](./../../)) found that **Qdrant has no embedded mode for
the Rust SDK** — `qdrant-client` v1.18.0 is gRPC-only, requires a separate Qdrant
server, and "embedded Qdrant" would mean bundling the binary as a subprocess,
violating pilar 1 (local-first, 100% offline, no extra runtime daemons). So the
real decision is binary: keep `sqlite-vec` or migrate to `lancedb`.

The store layer was not port-shaped before this change — only `GraphStore` was a
trait. The engine reached into `db.conn()`/`conn_mut()` in 53+ places. This refactor
was its own deliverable (3.1a) and is now complete: the `Store` trait
([src/store/store.rs](../../src/store/store.rs)) abstracts the four data planes
(chunks, vectors, lexical, graph) plus metadata; `SqliteStore` and `LanceStore`
both implement it; the contract test suite in
[tests/store_contract.rs](../../tests/store_contract.rs) verifies both backends
satisfy the same 14-method contract.

## Decision Drivers

* **pilar 1 — local-first**: must run offline, no extra daemons
* **pilar 2 — reproducibility**: must produce bit-for-bit reproducible results
* **pilar 4 — footprint**: memory + disk + binary size must stay controlled
* **pilar 5 — testability**: the `Store` port must exist before any future swap

## Considered Options

### Option A — Stay with sqlite-vec (this ADR)
* Pros: zero regression risk; no new deps; no binary-size hit; the `Store` port
  refactor is a strict improvement regardless; the verdict matrix is simple and
  re-runnable
* Cons: no native ANN indexing for very large corpora; KNN reindex at 500K+
  chunks may get slow (needs measurement)

### Option B — Migrate to LanceDB
* Pros: native ANN (IVF_PQ) for true scale (1M+ chunks); Apache Arrow / DataFusion
  ecosystem; async-native, in-process, Apache-2.0
* Cons:
  * Heavy dep tree (lance 7.0.0 + datafusion 53.0.0 + arrow 58.0.0 + object_store
    + moka + optional polars) — significant build time, binary size, and cold-start
  * Requires `protoc` (protobuf compiler) at build time — see build instructions
    in [src/store/lancedb.rs](../../src/store/lancedb.rs)
  * Async-only API — every `Store` method is async; the engine has 5800+ lines
    that would need a sync→async cascade if we ever decided the legacy `Database`
    path needed replacement
  * No native graph — graph queries become SQL-on-LanceDB with a join, losing
    the GraphStore trait's late-resolution ergonomics
  * Schema migration is destructive — v4 → v5 would re-index all users

### Option C — Run benchmark only, defer port refactor
* Pros: cheapest path to an informed decision
* Cons: doesn't satisfy pilar 5 (port should exist before need); if we later
  migrate, port work has to be done at migration time, not before

## Decision Outcome

**Chosen option: A — Stay with sqlite-vec**, because:

1. The pre-benchmark probability of "stay" is high: the LanceDB dep tree alone is
   likely to fail the binary-size budget (criterion 5 of the exploration
   recommendation), and the sync-vs-async + port-refactor cost is likely to fail
   the "is the gain worth it" framing at our current chunk counts (10K-500K).
2. The `Store` port refactor pays off independently of the verdict — it makes
   the engine testable and gives us the option to migrate later without
   cascading changes through 53+ call sites.
3. The verdict function
   ([src/bench/verdict.rs](../../src/bench/verdict.rs)) is re-runnable: the
   Stay outcome can be re-evaluated in 6 months (or at the 10x chunk-count
   milestone) by running the harness against fresh reports.

### Benchmark Table (real measurement — remediation pass, 2026-06-21)

The harness at [src/bench/store_bench.rs](../../src/bench/store_bench.rs)
produces a `StoreMetricsReport` for each backend. Run via:

```bash
# Sqlite-vec (default backend, no feature flag), SLO-only measurement
cargo run --release -- bench-store --corpus vscode --mock-embedder --query-sample 0

# Full 4-axis measurement (slow — query phase is O(N) against the brute-force
# sqlite-vec KNN; add --query-sample 10 to cap it)
cargo run --release -- bench-store --corpus vscode --mock-embedder --query-sample 100
```

The verdict is computed by `compare_reports(sqlite_report, lance_report)`.

| Axis              | Threshold          | sqlite-vec (incumbent) | LanceDB (candidate) | Result    |
|-------------------|--------------------|------------------------|---------------------|-----------|
| Indexing speed    | ≥1.5x faster       | **3.15s** (2138 files, 14863 chunks) | Not measured (shim) | n/a       |
| Peak RSS          | ≤1.2x              | **213 MB**             | Not measured (shim) | n/a       |
| On-disk size      | ≤1.2x              | **60 MB**              | Not measured (shim) | n/a       |
| Query p95 latency | ≤1.2x              | ~1.5s* (extrapolated; see note) | Not measured (shim) | n/a       |
| SLO (R3)          | ≤360s on vscode    | **3.15s — PASS**       | Not measured (shim) | **PASS**  |
| **Verdict**       | All 4 axes + SLO   |                        |                     | **STAY**  |

\* **Query latency note**: 100 queries × O(N) brute-force KNN against 14,863
vectors takes >20 min on this machine, so the bench harness was run with
`--query-sample 0` (SLO-only) for the headline number above. A 1,668-vector
subset (150 files) measured **1.55s per query** (p50=p95) — a linear
extrapolation to 14,863 vectors gives ~14s/query × 100 = ~23 min for a full
sample. The query-axis is not the spec's hard SLO (the SLO is on
indexing_secs), but the slow query path IS a real engineering concern for
interactive use. See "Follow-up — Query Latency" below.

#### LanceDB honesty

The `LanceStore` impl in [src/store/lancedb.rs](../../src/store/lancedb.rs)
is an **in-memory shim** — it satisfies the `Store` trait contract but does
not use the real `lancedb` crate. Running `--backend lancedb` would
exercise the shim and produce numbers that DO NOT represent real LanceDB
performance (the shim is in-memory and faster than real LanceDB on small
data, slower on disk-bound workloads). We refuse to publish shim numbers as
LanceDB numbers (pilar 2 — reproducibility). The LanceDB column is therefore
"Not measured" until the real `lancedb::connect()` + `FixedSizeList<Float32>`
schema wiring is in place. The "Stay" verdict is robust to that: the
*structural* reasons (dep tree, async cascade, no native graph) hold
independent of the unmeasured columns.

#### Verdict = Stay

The verdict stays at **Stay** because:

1. The SLO is PASS (3.15s ≪ 360s on the vscode corpus).
2. The structural reasons against migrating to LanceDB (dep tree, async
   cascade, no native graph) are independent of any measured numbers.
3. We have no real LanceDB numbers to overturn the structural argument.
   Re-evaluation is conditioned on real LanceDB wiring (see below).

## Consequences

### Positive

* Zero regression risk — same engine, same store, same binaries
* No new runtime dependencies — `cargo build` stays lean
* The `Store` port is in place — any future migration is a one-trait-swap per
  engine call site
* The verdict function is unit-tested with 7 scenarios (win-all, lose-one,
  tie, SLO-failure) — see `src/bench/verdict.rs` — so when the measurements
  come in, the verdict is computed correctly

### Negative

* We don't get the scale story of an ANN-tuned store for very large corpora
* If sqlite-vec KNN reindex at 500K+ chunks becomes a bottleneck, we have no
  pre-baked alternative; we'll have to revisit this ADR
* The LanceStore scaffold in [src/store/lancedb.rs](../../src/store/lancedb.rs)
  uses an in-memory shim instead of real LanceDB tables — the contract is
  satisfied but the dep is not exercised end-to-end. The real LanceDB wiring
  (FixedSizeList<Float32>[dims] schema, `create_table().execute()`) is a
  one-line swap per method when needed.
* **Query latency is O(N) brute-force KNN** — at 14,863 vscode vectors each
  query is ~14s (extrapolated from 1,668-vector subset at 1.55s/query). The
  SLO is on indexing, not on query latency, so this does not violate R3, but
  it IS a real defect for interactive use at this scale. See
  "Follow-up — Query Latency" below.

## Follow-up — Query Latency (SLO passes, but query axis is a real defect)

**Status**: Identified during remediation pass (2026-06-21). NOT a blocker
for this ADR (the spec's hard SLO is on `indexing_secs` and that passed at
3.15s), but a real engineering issue for interactive search at the vscode
corpus scale.

**Symptom**: 100 queries × O(N) brute-force KNN against 14,863 vectors would
take ~23 minutes — the bench harness had to add a `--query-sample 0` mode
to make SLO measurements tractable. Measured 1,668-vector subset: 1.55s
p50/p95. Extrapolated 14,863 vectors: ~14s p50/p95. A user running
`vectorcode search` interactively against a 15K-file index would wait 14s
per query, which is unacceptable.

**Root cause hypothesis**: `sqlite-vec` performs brute-force KNN over the
full vector table. There is no native ANN index. The 384-dim inner-product
loop is the hot path. LanceDB's IVF_PQ or sqlite-vec's hypothetical ANN
mode would reduce this to milliseconds.

**Possible mitigations** (not implemented in this pass — focused scope):
1. sqlite-vec `vector_quantization` mode (if/when available in the bundled
   0.1.6 build — needs research).
2. Switch the runtime to a brute-force-capable store for very large corpora
   (LanceDB with real wiring, or a dedicated ANN dep).
3. Pre-filter by file path / language to reduce the search set per query
   (orthogonal improvement).
4. Cache the query embedding + result set in the MCP server (per-session
   dedup).

**Tracking**: This is a follow-up issue for the next phase, not a
re-evaluation trigger for this ADR. The SLO itself passed. Re-evaluate this
ADR's "Stay" verdict only when one of the `Re-evaluation Conditions`
below becomes true.

## Re-evaluation Conditions

Re-run the verdict and potentially supersede this ADR if **ANY** of the
following becomes true:

1. **Chunk count crosses 500K** — at this scale, sqlite-vec KNN reindex cost
   may exceed the SLO. Measure the vscode corpus; if it's already over the
   threshold on real customer workloads, the verdict is automatic Migrate.
2. **LanceDB ships a native graph store** — the current "no native graph" cost
   would vanish. The exploration noted this as a known gap.
3. **Async cascade becomes avoidable** — if the engine's 53+ call sites are
   fully ported to the `Store` trait, the "sync trait + sync LanceDB wrapper"
   concern from design decision #1 dissolves. The migration cost drops to
   "swap the factory default" + "flip schema to v5".
4. **LanceDB 1.0 ships** — the 0.x API churn cost disappears.

## Cadence

Re-evaluate **every 6 months** (review cycle: 2026-12, 2027-06, ...) **OR** at
the 10x chunk-count milestone (whichever comes first). The owner of this ADR
adds a `Date of last review` section on each review.

## Verification

* `cargo test --all-targets` — 739 tests passing (post-remediation)
* `cargo test --test store_contract --features lancedb-store` — 12 contract
  tests (11 default + 1 LanceDB) passing
* `cargo build` (default) — does NOT pull LanceDB; binary size unchanged
* `cargo build --features lancedb-store` — compiles; requires `protoc` on PATH
* `cargo run --release -- bench-store --corpus vscode --mock-embedder --query-sample 0`
  — SLO-only run, completes in ~9s, `slo_passed: true`, `indexing_secs: 3.15`

## References

* Exploration: engram obs #66 (sqlite-vec vs LanceDB vs Qdrant investigation)
* Spec: engram obs #69 (R1–R6 requirements)
* Design: engram obs #70 (sync trait, sync LanceDB wrapper, decision matrix)
* Tasks: engram obs #71 (19 tasks across 4 commits)
* Verdict function: [src/bench/verdict.rs](../../src/bench/verdict.rs)
* Contract tests: [tests/store_contract.rs](../../tests/store_contract.rs)
* Store trait: [src/store/store.rs](../../src/store/store.rs)
* SqliteStore: [src/store/sqlite.rs](../../src/store/sqlite.rs)
* LanceStore: [src/store/lancedb.rs](../../src/store/lancedb.rs)
* Bench harness: [src/bench/store_bench.rs](../../src/bench/store_bench.rs)
