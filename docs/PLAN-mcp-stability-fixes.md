# MCP Stability Fixes — Implementation Plan

**Date**: 2026-06-11
**For**: vectorcode v0.1.0
**Goal**: Fix MCP server startup failures, ONNX hang, Ollama batch errors, and transport-level blocking issues.

---

## Root Cause Summary

1. **ONNX Runtime hangs on macOS** — `Session::builder().commit_from_memory()` blocks indefinitely when CoreML EP tries to compile the model. This blocks the calling thread (sync call inside async context), preventing the MCP message loop from ever starting.
2. **Ollama batch HTTP errors** — remote Ollama instance (`192.168.0.14:11434`) fails intermittently, causing index failures and 0-chunk results.
3. **`std::sync::Mutex` blocking inside async handlers** — `handle_vec_status` and `handle_vec_reindex` use `state.db.lock().unwrap()` which blocks the tokio worker thread.
4. **No startup resilience** — `serve --mcp` has no fallback, timeout, or readiness signal for slow embedder initialization.
5. **Missing `ping` handler** — MCP spec utility; clients may disconnect if pings go unanswered.

---

## Changes (ordered by priority)

### P1 — Fix ONNX session creation hang

**Files**: `src/embedder/onnx.rs`, `src/cli/mod.rs`

**Problem**: `OnnxEmbedder::from_cache()` calls `Session::builder().commit_from_memory()` synchronously. On macOS ARM64, ONNX Runtime's CoreML EP initialization can hang for minutes or forever.

**Solution**:
1. Make `create_embedder_from_config` async (or add an async variant).
2. Wrap ONNX session creation in `tokio::task::spawn_blocking` with a 60-second timeout.
3. On timeout, return a clear error instead of hanging.
4. In `serve.rs` and `index.rs`, handle the error gracefully.

**Implementation**:

In `src/embedder/onnx.rs`, add a timeout-aware constructor:
```rust
/// Create from cache with a timeout on session creation.
pub async fn from_cache_with_timeout() -> EmbedderResult<Self> {
    let manager = ModelManager::new();
    let (model_bytes, tokenizer_bytes) = manager.load_model()?;
    
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::task::spawn_blocking(move || Self::new(&model_bytes, &tokenizer_bytes))
    )
    .await
    .map_err(|_| VectorCodeError::EmbedderError {
        message: "ONNX model loading timed out after 60s. \
                  Try setting ORT_DISABLE_COREML=1 or switch provider.".to_string(),
    })?
    .map_err(|e| VectorCodeError::EmbedderError {
        message: format!("ONNX session creation failed: {e}"),
    })?
}
```

In `src/cli/mod.rs`, make `create_embedder_from_config` async:
```rust
pub async fn create_embedder_from_config(config: &Config) -> Result<Arc<dyn Embedder>> {
    match config.provider.name.as_str() {
        "onnx" => {
            let embedder = crate::embedder::onnx::OnnxEmbedder::from_cache_with_timeout()
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Arc::new(embedder))
        }
        // ... other providers unchanged ...
    }
}
```

Add environment variable override for CoreML:
```rust
// In onnx.rs, before Session::builder()
let mut builder = Session::builder()?;
if std::env::var("ORT_DISABLE_COREML").is_ok() {
    // Disable CoreML EP to avoid hang
    builder = builder.with_execution_providers([ort::execution_providers::CPUExecutionProvider::default()])?;
}
```

---

### P2 — Fix Ollama batch failures

**Files**: `src/embedder/ollama.rs`

**Problem**: `embed_batch` sends all texts in one HTTP request. If the batch is large or Ollama is remote, the request can fail with a connection error. Retry logic exists but only covers HTTP status codes (429, 500, 503), NOT connection errors.

**Solution**:
1. Retry on connection/timeout errors, not just HTTP status codes.
2. Split large batches into smaller chunks (max 50 texts per request).
3. Add better error messages including the URL.

**Implementation**:

In the `embed_batch` implementation, add chunking:
```rust
async fn embed_batch(&self, texts: &[&str]) -> EmbedderResult<Vec<Vec<f32>>> {
    const CHUNK_SIZE: usize = 50;
    let mut results = Vec::with_capacity(texts.len());
    
    for chunk in texts.chunks(CHUNK_SIZE) {
        let chunk_results = self.embed_chunk_with_retry(chunk).await?;
        results.extend(chunk_results);
    }
    Ok(results)
}
```

Retry on connection errors:
```rust
async fn embed_chunk_with_retry(&self, texts: &[&str]) -> EmbedderResult<Vec<Vec<f32>>> {
    let mut last_error = None;
    for attempt in 0..crate::embedder::http::MAX_RETRIES {
        match self.try_embed_chunk(texts).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = Some(e);
                if attempt < crate::embedder::http::MAX_RETRIES - 1 {
                    let backoff = crate::embedder::http::calculate_backoff(
                        attempt, 1000, 30000, crate::embedder::http::jitter_factor()
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
    Err(last_error.unwrap())
}
```

---

### P3 — Move `std::sync::Mutex` DB operations off async runtime

**Files**: `src/mcp/handler.rs`, `src/mcp/mod.rs`

**Problem**: `handle_vec_status` and `handle_vec_reindex` call `state.db.lock().unwrap()` which uses `std::sync::Mutex`. This blocks the tokio worker thread. Under load, this can stall the entire message loop.

**Solution**:
1. Change `AppState.db` from `Arc<std::sync::Mutex<Database>>` to `Arc<tokio::sync::Mutex<Database>>`.
2. Update all call sites to use `.lock().await` instead of `.lock().unwrap()`.
3. For CPU-heavy DB operations, use `spawn_blocking`.

**Implementation**:

In `src/mcp/mod.rs`, change `AppState`:
```rust
pub struct AppState {
    pub db: Arc<tokio::sync::Mutex<Database>>,  // changed from std::sync::Mutex
    pub embedder: Arc<dyn Embedder>,
    pub config: Config,
    pub project_path: PathBuf,
    pub watcher: Option<Arc<tokio::sync::RwLock<FileWatcher>>>,
}
```

In `src/mcp/handler.rs`, update handlers:
```rust
// handle_vec_status
fn handle_vec_status(state: &AppState, arguments: &serde_json::Value) -> ToolCallResult {
    let db = state.db.lock().await; // now async
    // ... rest unchanged
}

// handle_vec_reindex
async fn handle_vec_reindex(state: &AppState, arguments: &serde_json::Value) -> ToolCallResult {
    let db = state.db.lock().await; // now async
    // ... rest unchanged
}
```

Update `src/cli/serve.rs` AppState construction:
```rust
let state = AppState {
    db: Arc::new(tokio::sync::Mutex::new(db)),  // changed
    // ...
};
```

---

### P4 — Add ping handler

**Files**: `src/mcp/handler.rs`, `src/mcp/schema.rs`, `src/mcp/mod.rs`

**Problem**: MCP spec says clients MAY send `ping` requests. No handler exists, so pings get `-32601 Method not found`. Some clients interpret this as connection failure.

**Solution**: Add a `ping` method handler that returns an empty object `{}`.

**Implementation**:

In `src/mcp/handler.rs`:
```rust
/// Handle the "ping" method (spec § utilities/ping).
pub fn handle_ping() -> serde_json::Value {
    serde_json::json!({})
}
```

In `src/mcp/mod.rs` dispatch:
```rust
"ping" => {
    let result = handler::handle_ping();
    serde_json::to_value(make_response(id, result)).unwrap_or_default()
}
```

---

### P5 — Improve MCP server startup resilience

**Files**: `src/cli/serve.rs`, `src/mcp/transport.rs`

**Problem**: If embedder creation fails or hangs, the MCP server never starts its message loop. OpenCode sees `-32000: Connection closed`. There's no fallback or timeout.

**Solution**:
1. Make `create_embedder_from_config` async with timeout (done in P1).
2. Add a readiness signal: write a known message to stderr after the message loop starts.
3. Add a startup timeout: if initialization takes >30s, log a warning.

**Implementation**:

In `serve.rs`, after `server.run().await` starts:
```rust
eprintln!("MCP_READY"); // Signal to clients that the server is ready
```

Add startup timeout wrapper:
```rust
let init_result = tokio::time::timeout(
    std::time::Duration::from_secs(90),
    async {
        // ... all initialization ...
        server.run().await
    }
).await;
```

---

### P6 — Add `listChanged: true` to tools capability

**Files**: `src/mcp/handler.rs`

**Problem**: `InitializeResult.capabilities.tools` is `json!({})` — empty object. MCP spec defines `listChanged` as a boolean sub-capability.

**Solution**: Set `listChanged: true` since the file watcher can detect when indexed files change (which may affect available tool behavior).

**Implementation**:
```rust
pub fn handle_initialize() -> InitializeResult {
    InitializeResult {
        protocol_version: "2024-11-05".to_string(),
        capabilities: ServerCapabilities {
            tools: serde_json::json!({"listChanged": true}),
        },
        server_info: ServerInfo {
            name: "vectorcode".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    }
}
```

---

### P7 — Fix fragile parameter parsing

**Files**: `src/mcp/mod.rs`, `src/mcp/handler.rs`

**Problem**: 
- `request.params["name"].as_str().unwrap_or("")` — if name is missing, error is confusing.
- `handle_vec_status` uses `unwrap_or` silently swallowing parse errors.

**Solution**: Validate explicitly and return proper error messages.

**Implementation**:

In `mod.rs` dispatch for `tools/call`:
```rust
"tools/call" => {
    let tool_name = request.params["name"].as_str().unwrap_or("");
    if tool_name.is_empty() {
        return Some(serde_json::to_value(make_error(
            id, -32602, "Missing required parameter: 'name'".to_string()
        )).unwrap_or_default());
    }
    let arguments = &request.params["arguments"];
    // ... rest unchanged
}
```

In `handler.rs` `handle_vec_status`:
```rust
let params: VecStatusParams = match serde_json::from_value(arguments.clone()) {
    Ok(p) => p,
    Err(e) => {
        return make_error_result(format!("Invalid vec_status parameters: {e}"));
    }
};
```

---

### P8 — Clean up dead code

**Files**: `src/mcp/mod.rs`

**Problem**: The `notifications/initialized` match arm (lines 151-156) is unreachable because notifications without `id` are filtered at line 107.

**Solution**: Remove the dead arm and add a comment explaining that `notifications/initialized` is handled by the notification filter.

**Implementation**: Remove lines 151-156 from `src/mcp/mod.rs`.

---

## Implementation Order

```
P1 (ONNX hang) ────────► P5 (startup resilience)
                              │
P2 (Ollama batch) ───────────┤
                              │
P3 (Mutex blocking) ─────────┤
                              ▼
P4 (ping handler) ──────► Integration test
                              
P6 (listChanged) ───────► P7 (fragile parsing) ──► P8 (dead code)
```

## Verification Checklist

After implementation, verify:

- [ ] `cargo run -- index` completes with ONNX provider (no hang, or clear timeout error)
- [ ] `cargo run -- index` completes with Ollama provider (batches succeed, chunks > 0)
- [ ] `cargo run -- serve --mcp` starts and prints `MCP_READY` within 90 seconds
- [ ] `cargo test --all-targets` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] Manual MCP connection test with OpenCode: server responds to `initialize` and `tools/list`

## Files Touched (estimated)

| File | Changes | Risk |
|------|---------|------|
| `src/embedder/onnx.rs` | Add timeout, CoreML env var | Medium |
| `src/cli/mod.rs` | Make create_embedder async | Medium |
| `src/cli/serve.rs` | Async embedder, readiness signal | Low |
| `src/cli/index.rs` | Await async embedder creation | Low |
| `src/embedder/ollama.rs` | Batch chunking, retry on conn errors | Medium |
| `src/mcp/handler.rs` | Async mutex, ping, fix parsing | Medium |
| `src/mcp/mod.rs` | Async mutex type, ping dispatch, dead code | Medium |
| `src/mcp/transport.rs` | Readiness signal | Low |
| `src/engine/indexer.rs` | Update Mutex type if needed | Low |
| `src/engine/searcher.rs` | Update Mutex type if needed | Low |

## Estimated Changed Lines

~200-250 lines across ~10 files.
