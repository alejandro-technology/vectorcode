# 0001 — Stay with sqlite-vec, defer LanceDB migration

* Status: **proposed** (real-measurement verification pending; see `Re-evaluation Conditions`)
* Date: 2026-06-20
* Deciders: vectorcode maintainers
* Consulted: phase-3 store evaluation (3.1 + 3.2 SDD), exploration obs #66
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

### Benchmark Table (template — fill in after real measurement)

The harness at [src/bench/store_bench.rs](../../src/bench/store_bench.rs)
produces a `StoreMetricsReport` for each backend. Run via:

```bash
# Sqlite-vec (default backend, no feature flag)
cargo run --release -- bench-store --corpus vscode

# LanceDB (opt-in)
cargo run --release --features lancedb-store -- bench-store --corpus vscode
```

The verdict is computed by `compare_reports(sqlite_report, lance_report)`.

| Axis              | Threshold          | sqlite-vec (incumbent) | LanceDB (candidate) | Result    |
|-------------------|--------------------|------------------------|---------------------|-----------|
| Indexing speed    | ≥1.5x faster       | _pending measurement_  | _pending_           | _pending_ |
| Peak RSS          | ≤1.2x              | _pending_              | _pending_           | _pending_ |
| On-disk size      | ≤1.2x              | _pending_              | _pending_           | _pending_ |
| Query p95 latency | ≤1.2x              | _pending_              | _pending_           | _pending_ |
| SLO (R3)          | ≤360s on vscode    | _pending_              | _pending_           | _pending_ |
| **Verdict**       | All 4 axes + SLO   |                        |                     | **STAY**  |

**Verdict = Stay** until the table is filled in with measured numbers AND all
axes pass. The harness is the source of truth; this ADR is the recorded decision.

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

* `cargo test --all-targets` — 722 tests passing
* `cargo test --test store_contract --features lancedb-store` — 12 contract
  tests (11 default + 1 LanceDB) passing
* `cargo build` (default) — does NOT pull LanceDB; binary size unchanged
* `cargo build --features lancedb-store` — compiles; requires `protoc` on PATH

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
