# Exhaustive Audit Report: VectorCode — Bugs, Errors & Issues

**Date**: 2026-06-11  
**Scope**: Full Rust codebase plus CI/release/config paths  
**Baseline checked**:

- `cargo fmt --check` ✅
- `cargo clippy --all-targets --all-features -- -D warnings` ✅
- `cargo test --all-targets` ✅ 483/483 tests pass when run outside the sandbox
- In-sandbox `cargo test --all-targets` ❌ 3 tests fail because localhost `TcpListener::bind("127.0.0.1:0")` is blocked by sandbox permissions, not by application logic.

**Prior report compared**: `docs/audit-2026-06-11.md`  
**Important note**: several prior findings are already fixed in current code, but some fixes are partial or ineffective.

---

## Summary

Current code is much healthier than the earlier audit baseline, but there are still production-grade issues. The most dangerous cluster is the indexing pipeline: it mutates DB state before embedding succeeds, so a transient provider failure can permanently mark changed files as indexed while leaving stale or missing chunks behind. That is the architectural equivalent of pouring concrete before checking the foundation — it may look fine until the first real load hits it.

| Severity | Count |
|---|---:|
| CRITICAL | 4 |
| HIGH | 11 |
| MEDIUM | 7 |
| LOW | 4 |

---

## ✅ Validation Results

```bash
cargo fmt --check
# pass

cargo clippy --all-targets --all-features -- -D warnings
# pass

cargo test --all-targets
# pass outside sandbox: 461 unit + 14 chunker integration + 8 MCP integration
```

Sandbox-only failure observed:

```text
embedder::model_manager::* panicked at src/embedder/model_manager.rs:360
TcpListener::bind("127.0.0.1:0") -> PermissionDenied
```

This is an environment restriction, not a code failure.

---

## 🚨 CRITICAL

---

### C1 — Indexer commits file/chunk metadata before embeddings succeed, causing permanent stale or missing index data

**Files**: `src/engine/indexer.rs:106-144`, `src/engine/indexer.rs:183-210`, `src/engine/indexer.rs:244-343`  
**Severity**: CRITICAL — data integrity / silent corruption

**What's wrong**:

`process_file_entries()` starts a DB transaction, deletes removed chunks, updates the `files` table, commits, and only then `index_project()` / `index_files()` calls `embed_batch()` and inserts vectors.

Relevant flow:

1. `process_file_entries()` reads changed files.
2. It deletes old chunks whose IDs no longer exist: `chunks::delete_chunk(...)` at `src/engine/indexer.rs:313`.
3. It updates the file record with the new hash/mtime: `files::upsert_file(...)` at `src/engine/indexer.rs:338`.
4. It commits at `src/engine/indexer.rs:343`.
5. Only afterwards, embeddings are requested at `src/engine/indexer.rs:121` / `191`.
6. If the embedder fails, the file is already marked current.

That means a transient Gemini/OpenAI/Ollama/ONNX failure can leave the DB in this state:

- `files` says the changed file is indexed.
- Removed chunks are gone.
- Updated chunks may still contain old content if their byte range stayed stable.
- New chunks were never inserted.
- The next incremental run sees matching `mtime + size + hash` and skips the file forever.

**Fix**:

Make changed-file metadata updates, old chunk deletion, new chunk insertion, vector insertion, and file record update part of one atomic transaction that happens only after embeddings succeed.

Concrete approach:

1. Change `process_file_entries()` to be a pure planning phase:
   - read files
   - compute changed files
   - chunk content
   - return a plan: existing chunks, new chunks, file records to update
   - do **not** mutate DB except reads
2. Embed all new chunks.
3. Open one transaction.
4. Delete obsolete chunks.
5. Insert/replace new chunks and vectors.
6. Update `files` rows.
7. Commit.
8. Roll back automatically on any error.

**Verification**:

Add a mock embedder that fails after `process_file_entries()` identifies a changed file. Confirm:

- old chunks remain intact,
- `files.hash` remains old,
- a subsequent successful index updates the file correctly.

---

### C2 — Manual DB transactions can leave the connection stuck in an open transaction on error

**Files**: `src/engine/indexer.rs:134-140`, `src/engine/indexer.rs:204-210`, `src/engine/indexer.rs:244-343`, `src/store/db.rs:190-235`  
**Severity**: CRITICAL — data integrity / cascading failures

**What's wrong**:

The code uses raw SQL transaction control:

```rust
conn.execute("BEGIN", [])?;
// multiple fallible operations
conn.execute("COMMIT", [])?;
```

If any `?` returns before `COMMIT`, there is no `ROLLBACK`. The same SQLite connection can remain inside an open transaction. Later calls can fail with nested transaction errors or observe partially mutated state.

This appears in:

- `Indexer::index_project()` vector insert transaction.
- `Indexer::index_files()` vector insert transaction.
- `Indexer::process_file_entries()` metadata transaction.
- `Database::migrate_v1_to_v2()` migration transaction.

**Fix**:

Use `rusqlite::Transaction` / `conn.transaction()` everywhere. It rolls back on drop unless committed.

```rust
let tx = db.conn_mut().transaction()?;
// mutations

tx.commit()?;
```

This requires passing mutable connections (`conn_mut()`) rather than using raw `execute("BEGIN")` on `&Connection`.

**Verification**:

Inject a failure mid-transaction and assert:

- no partial writes remain,
- subsequent indexing can run successfully on the same connection.

---

### C3 — `vectorcode index` and `vectorcode search` silently fall back to `MockEmbedder`

**Files**: `src/cli/index.rs:96-110`, `src/cli/search.rs:62-75`  
**Severity**: CRITICAL — silent wrong results / index corruption

**What's wrong**:

If the configured provider cannot be created, production CLI commands silently use `MockEmbedder`:

```rust
Err(_) => {
    eprintln!("Warning: Could not create ... using mock embedder for testing");
    Arc::new(MockEmbedder::new(index_meta.dimensions))
}
```

That is catastrophic outside tests:

- `vectorcode index` can write deterministic fake vectors into a real project index.
- `vectorcode search` can compare a fake query vector against real provider vectors.
- Results become meaningless while the command still exits successfully.

A warning is not enough. This is data corruption wearing a friendly hat.

**Fix**:

Fail closed in production. Only allow `MockEmbedder` when `config.provider.name == "mock"` or behind `#[cfg(test)]`.

```rust
let embedder = crate::cli::create_embedder_from_config(&config)?;
```

If tests need fallback, make it explicit in test config.

**Verification**:

- Set provider to `gemini` with no API key.
- Run `vectorcode index`.
- Expected: non-zero exit, no index mutation.

---

### C4 — MCP `vec_reindex full=true` is not a full rebuild

**File**: `src/mcp/handler.rs:147-162`  
**Severity**: CRITICAL — operator command lies about behavior

**What's wrong**:

The MCP handler says `full` should force a full rebuild, but implementation only calls:

```rust
db.init_schema(state.embedder.dimensions())
```

`init_schema()` is idempotent. It does not drop existing chunks, file records, vectors, or stale state. Then `index_project()` runs incremental logic and skips unchanged files.

Result: a user can request a full rebuild through MCP and still keep stale/corrupt vectors.

**Fix**:

Mirror CLI full-reindex behavior safely:

1. Add a library function like `reset_index(db_path, meta)` or `Database::reset_schema(dims)`.
2. Delete/recreate all index tables in a transaction or replace the DB file with correct WAL handling.
3. Re-write metadata.
4. Run `index_project()` from a clean DB.

**Verification**:

Add MCP integration test:

1. Insert a bogus chunk/vector.
2. Call `vec_reindex` with `{ "full": true }`.
3. Assert bogus data is gone and all files were re-embedded.

---

## 🔴 HIGH

---

### H1 — MCP `vec_reindex.path` parameter is ignored

**File**: `src/mcp/handler.rs:140-162`  
**Severity**: HIGH — tool contract violation / unnecessary full-project work

**What's wrong**:

`VecReindexParams` contains `path`, and the tool schema says it can reindex a specific file or directory. The handler parses it but never uses it:

```rust
let params: VecReindexParams = ...
// params.path ignored
indexer.index_project(&state.project_path).await
```

**Fix**:

If `path` is present:

- resolve it under `state.project_path`,
- reject paths outside the project,
- if file: call `index_files(&[path])`,
- if directory: discover files under that directory and call `index_files()`.

**Verification**:

MCP integration test where `path` points to one file and only that file is reindexed.

---

### H2 — MCP `vec_status.projectPath` parameter is ignored

**File**: `src/mcp/handler.rs:119-124`, `src/mcp/schema.rs:193-206`  
**Severity**: HIGH — misleading API surface

**What's wrong**:

The tool schema advertises `projectPath`, but handler discards it:

```rust
let _params: VecStatusParams = ...
let db = state.db.lock().unwrap();
```

Users may ask for another initialized project and receive the current server project's status instead.

**Fix**:

Either remove `projectPath` from the schema or implement it by opening that project's `.vectorcode/index.db` after validating the path.

**Verification**:

Test two temp projects with different metadata; querying `projectPath` should return the requested one.

---

### H3 — MCP transport still reads unbounded lines before enforcing `MAX_LINE_BYTES`

**File**: `src/mcp/transport.rs:35-48`  
**Severity**: HIGH — DoS / memory exhaustion

**What's wrong**:

The current code checks size after this call completes:

```rust
let bytes_read = stdin.read_line(&mut line).await?;
if bytes_read as u64 > MAX_LINE_BYTES { ... }
```

`read_line()` appends until newline or EOF. A malicious client can send a huge line and force allocation before the limit is checked. The limit exists, but it is not protecting memory.

**Fix**:

Read incrementally with `read_until(b'\n', &mut Vec<u8>)` plus a bounded buffer, or wrap the reader with `take(MAX_LINE_BYTES + 1)` per message and close on overflow.

**Verification**:

Send a line larger than 1 MiB without newline and confirm memory stays bounded and the server exits/errors cleanly.

---

### H4 — Gemini still creates an unbounded `reqwest::Client`

**File**: `src/embedder/gemini.rs:44-50`  
**Severity**: HIGH — hung requests

**What's wrong**:

OpenAI and Ollama use `build_http_client()`, but Gemini still does:

```rust
Self::with_client(api_key, dimensions, reqwest::Client::new())
```

So Gemini requests have no default request/connect timeout.

**Fix**:

```rust
Self::with_client(api_key, dimensions, crate::embedder::http::build_http_client())
```

**Verification**:

Unit test or code review verifying Gemini uses shared timeout client.

---

### H5 — Configured Gemini/OpenAI model names are ignored

**Files**: `src/cli/mod.rs:130-156`, `src/embedder/gemini.rs:49-77`, `src/embedder/openai.rs:44-59`  
**Severity**: HIGH — config lies / wrong provider behavior

**What's wrong**:

`GeminiConfig.model` and `OpenAiConfig.model` exist and TOML tests parse them, but `create_embedder_from_config()` does not pass those values into the embedders. Constructors always use hardcoded defaults:

- Gemini: `gemini-embedding-2`
- OpenAI: `text-embedding-3-small`

**Fix**:

Add constructors that accept model:

```rust
GeminiEmbedder::with_model(api_key, model, dimensions)
OpenAiEmbedder::with_model(api_key, model)
```

Then pass config values through.

**Verification**:

Config with non-default model should result in requests using that model/URL/body.

---

### H6 — Ollama model dimensions are hardcoded to 768 and embeddings are silently padded/truncated

**Files**: `src/embedder/ollama.rs:30-33`, `src/embedder/ollama.rs:195-197`, `src/store/vectors.rs:18-27`, `src/store/vectors.rs:51-55`  
**Severity**: HIGH — silent search quality corruption

**What's wrong**:

Ollama supports multiple embedding models with different dimensions, but `OllamaEmbedder::dimensions()` always returns `768`. `insert_vector()` then normalizes any returned embedding to DB dimensions by padding/truncating.

If the configured Ollama model returns 1024 or 384 dimensions, VectorCode silently damages vectors instead of rejecting the mismatch.

**Fix**:

Add `dimensions` to `OllamaConfig` or validate the first response length against index metadata and fail on mismatch.

**Verification**:

Mock an Ollama response with wrong vector length; indexing should fail with a provider mismatch, not normalize silently.

---

### H7 — Search/index vector dimension normalization hides provider mismatch

**Files**: `src/store/vectors.rs:18-27`, `src/store/vectors.rs:51-55`, `src/store/vectors.rs:144-146`  
**Severity**: HIGH — silent relevance degradation

**What's wrong**:

Both stored embeddings and query embeddings are padded/truncated to DB dimensions. That avoids crashes, but it masks serious provider/model mismatches.

A vector DB should not quietly reshape semantic vectors from another model. That produces mathematically valid but semantically invalid search.

**Fix**:

Validate exact dimensions before insertion/search. Return `VectorCodeError::ProviderMismatch` or a new `EmbeddingDimensionMismatch` error.

**Verification**:

- DB initialized at 768 dims.
- Attempt to insert/search with 1536-dim vector.
- Expected: explicit error.

---

### H8 — Release checksum verification fails open

**File**: `src/cli/upgrade.rs:121-187`  
**Severity**: HIGH — supply-chain security

**What's wrong**:

The upgrade flow verifies SHA256 only if `SHA256SUMS` exists and contains the tarball. If the sums file is missing, incomplete, or returns non-success other than 404, it logs a warning and proceeds.

For self-updating binaries, checksum verification should fail closed unless the user explicitly opts into insecure upgrade.

**Fix**:

- Require `SHA256SUMS` by default.
- Add `--allow-unverified` only if you really want an escape hatch.
- Consider signing `SHA256SUMS` with minisign/cosign/GPG.

**Verification**:

Mock release without `SHA256SUMS`; upgrade should abort.

---

### H9 — `indexing.concurrency` is still dead code

**Files**: `src/config/schema.rs:159-201`, `src/cli/index.rs:25-49`, `src/engine/indexer.rs:232-347`  
**Severity**: HIGH — misleading config / performance expectation bug

**What's wrong**:

The config and CLI expose concurrency, but `process_file_entries()` processes files in a sequential `for` loop. Setting `--concurrency 16` does nothing.

**Fix**:

Either implement bounded concurrency with `buffer_unordered(config.concurrency)` or remove the option until real parallelism exists. If implementing, be careful: SQLite writes must still be serialized.

**Verification**:

Benchmark with 1 vs 16 concurrency on a large project and assert different scheduling/throughput.

---

### H10 — `discover_files()` still uses suffix matching, not semantic extension matching

**File**: `src/engine/indexer.rs:384-389`  
**Severity**: HIGH — incorrect exclusions

**What's wrong**:

The comment says extension exclusion, but code checks filename suffix:

```rust
file_name.ends_with(ex)
```

This is okay for `.min.js`, but semantically wrong for plain extensions and inconsistent with the field name.

**Fix**:

Split config into two concepts:

- `exclude_extensions`: exact extension match via `Path::extension()`.
- `exclude_suffixes`: suffixes such as `.min.js`.

Or document that the current field is suffix-based and rename it.

**Verification**:

Tests for `.min.js`, `.js.map`, `.png`, and edge-case filenames.

---

### H11 — Model download has no retry and can leave first file installed when second file fails

**File**: `src/embedder/model_manager.rs:96-144`, `src/embedder/model_manager.rs:147-203`  
**Severity**: HIGH — partial install / reliability

**What's wrong**:

The model manager downloads and renames `model.onnx`, then downloads `tokenizer.json`. If tokenizer download fails, the final model remains installed but tokenizer is missing. `is_downloaded()` correctly returns false, but users are left with partial cache state.

It also has no retry for transient CDN failures.

**Fix**:

- Download both files to temp paths first.
- Verify sizes/checksums if available.
- Rename both into place only after both downloads succeed.
- Retry transient HTTP/network failures.

**Verification**:

Mock model success + tokenizer failure and confirm no final `model.onnx` remains.

---

## 🟡 MEDIUM

---

### M1 — Large-node line splitting computes wrong byte ranges

**File**: `src/engine/chunker.rs:254-283`  
**Severity**: MEDIUM — inaccurate metadata / unstable chunk IDs

**What's wrong**:

When splitting a large AST node into line chunks, every generated chunk uses the same `byte_start`:

```rust
let byte_start = node.start_byte() as u32;
let byte_end = (byte_start as usize + chunk_content.len()) as u32;
```

For the second and later chunks, byte ranges point to the beginning of the parent node, not to the chunk's actual content.

**Fix**:

Track cumulative byte offset within `node_source`, including newline bytes, or reuse a helper that maps line indexes to byte offsets in the original source.

**Verification**:

Create a large function split into multiple chunks and assert each chunk's `source[byte_start..byte_end]` equals `chunk.content`.

---

### M2 — `Retry-After` can force arbitrarily long sleeps

**Files**: `src/embedder/gemini.rs:155-163`, `src/embedder/gemini.rs:204-210`, `src/embedder/openai.rs:109-117`, `src/embedder/openai.rs:164-169`  
**Severity**: MEDIUM — availability / bad server behavior

**What's wrong**:

The code respects `Retry-After` seconds but applies no maximum. A server or proxy can return a huge value and park indexing for hours/days.

**Fix**:

Cap respected retry delay, e.g. 120 seconds, and return `RateLimited` when retry-after is larger.

**Verification**:

Mock `429 Retry-After: 999999`; command should fail fast with actionable error.

---

### M3 — `VectorCodeError::RateLimited` is effectively dead

**File**: `src/error.rs` plus retry handlers in `src/embedder/gemini.rs` and `src/embedder/openai.rs`  
**Severity**: MEDIUM — error semantics

**What's wrong**:

The code defines a rate-limit-specific error but retry exhaustion returns generic `EmbedderError`.

**Fix**:

Return `VectorCodeError::RateLimited { retry_after_secs }` for exhausted 429 cases.

**Verification**:

Mock repeated 429 and assert the specific error variant.

---

### M4 — Search options accept invalid thresholds and pathological limits

**Files**: `src/mcp/schema.rs:120-130`, `src/engine/searcher.rs:84-113`, `src/cli/search.rs:20-34`  
**Severity**: MEDIUM — bad input handling

**What's wrong**:

`threshold` is documented as `0.0–1.0`, but not validated. `limit` is also unbounded.

Bad inputs can produce confusing behavior or excessive DB work.

**Fix**:

Validate:

- `0.0 <= threshold <= 1.0`
- `1 <= limit <= reasonable_max` (for example 100 or 1000)

Also add JSON schema `minimum` / `maximum`.

**Verification**:

MCP and CLI tests for invalid threshold/limit.

---

### M5 — `init` lock file is never removed on success

**File**: `src/cli/init.rs:55-68`  
**Severity**: MEDIUM — stale lock / confusing recovery

**What's wrong**:

`init` creates `.vectorcode.init.lock`, holds the file handle, but never removes the file. A successful init leaves a permanent lock file in the project root. Because `.vectorcode/` now exists, most reruns fail earlier with “already initialized”, but the stale lock is still bad hygiene and can confuse manual recovery.

**Fix**:

Use a guard that removes the lock path on drop, or create the lock inside `.vectorcode/` after `create_dir` succeeds using atomic directory creation.

**Verification**:

Run init and assert `.vectorcode.init.lock` does not remain.

---

### M6 — CLI debug output leaks into normal `vectorcode index` UX

**File**: `src/cli/index.rs:63`, `src/cli/index.rs:78-80`, `src/cli/index.rs:96`, `src/cli/index.rs:113`, `src/cli/index.rs:156`, `src/cli/index.rs:160`, `src/cli/index.rs:163`  
**Severity**: MEDIUM — production polish / script noise

**What's wrong**:

The command prints unconditional debug lines with `eprintln!`, regardless of `--verbose`:

```text
DEBUG: meta loaded
DEBUG: creating embedder...
DEBUG: embedder created.
```

**Fix**:

Replace with `tracing::debug!` or remove.

**Verification**:

Run `vectorcode index --quiet`; no debug output should appear.

---

### M7 — Full CLI reindex removes DB file but ignores failure removing WAL/SHM sidecars

**File**: `src/cli/index.rs:70-80`  
**Severity**: MEDIUM — reset correctness

**What's wrong**:

`index --full` removes `index.db`, then ignores errors removing `index.db-wal` and `index.db-shm`. On some platforms/filesystems, stale sidecars can produce confusing state.

**Fix**:

Use SQLite-level table reset in a transaction or handle sidecar deletion errors explicitly after closing all DB handles.

**Verification**:

Integration test with WAL sidecars present.

---

## 🔵 LOW

---

### L1 — MCP method `notifications/initialized` with an `id` is silently ignored

**File**: `src/mcp/mod.rs:151-155`  
**Severity**: LOW — protocol edge case

If a client sends `notifications/initialized` with an `id`, it is technically a request, not a notification. The current handler returns no response anyway.

Fix: only suppress responses when `id` is absent. If `id` is present, return method-not-found or a normal response depending on protocol intent.

---

### L2 — `serde_json::to_value(...).unwrap_or_default()` can hide serialization bugs

**File**: `src/mcp/mod.rs:103`, `src/mcp/mod.rs:118-130`, `src/mcp/mod.rs:144-164`  
**Severity**: LOW — observability

Serialization should not fail for these types, but if it does, returning `{}` hides the root cause and emits invalid JSON-RPC shape.

Fix: construct an explicit `-32603` error or log serialization failures.

---

### L3 — `ModelManager::new()` uses raw env vars for home directory

**File**: `src/embedder/model_manager.rs:50-58`  
**Severity**: LOW — portability

Uses `HOME` / `USERPROFILE` manually instead of `dirs::home_dir()`, even though the project already depends on `dirs`.

Fix: use `dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))`.

---

### L4 — Some prior audit fixes lack regression tests

**Files**: multiple  
**Severity**: LOW — maintenance risk

Examples:

- Gemini timeout fix is incomplete and should have a test/code assertion.
- MCP notification/null-id fixes need integration tests.
- Watcher deletion behavior should have an integration test covering DB cleanup.

Fix: add targeted regression tests for every previously critical audit item.

---

## Prior Audit Status Snapshot

From `docs/audit-2026-06-11.md`:

- Fixed or mostly fixed: Gemini key in URL, ONNX output panic, JSON-RPC null id handling, JSON-RPC version validation, watcher pending channel, watcher deletion handling, vec orphan cleanup, migration transaction, serve provider mismatch, install invalid JSON backup, install path fixes, macOS upgrade note.
- Still present / partial: Gemini HTTP timeout, ineffective MCP line-size limit, dead concurrency setting, suffix-based extension exclusion, model download retry/partial cache, checksum verification fails open.

---

## Recommended Fix Order

1. C1 + C2 together: make indexing transactional and atomic.
2. C3: remove production `MockEmbedder` fallback.
3. C4 + H1 + H2: make MCP tool contracts truthful.
4. H3 + H4: close availability/DoS gaps.
5. H5 + H6 + H7: enforce model/dimension correctness.
6. H8: fail closed for self-update verification.
7. Remaining medium/low polish and regression tests.

