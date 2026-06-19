# Exploration: ONNX Cross-Encoder Reranker (Fase 1.5)

**Change**: `fase1-5-reranker` (proposed)
**Date**: 2026-06-19
**Mode**: Architecture pre-proposal
**Roadmap phases**: 1.5 (reranker) + 1.6 (re-measure)

---

## 1. Current Architecture — what's reusable

### 1.1 Existing ONNX embedder pattern (the closest reference)

`src/embedder/onnx.rs` (492 lines) gives us a battle-tested pattern we can clone almost verbatim:

- **`OnnxEmbedder` struct**: `tokio::sync::Mutex<Session>` + `Tokenizer`
- **Session construction** (lines 46-78):
  - `Session::builder()?` → optional `with_execution_providers([CPU::default().build()])` → `commit_from_memory(model_bytes)?`
  - Honors `ORT_DISABLE_COREML` env var to bypass the macOS CoreML EP hang
- **Async wrapper with timeout** (lines 107-154): `from_cache_with_timeout()` runs ONNX work on a **`std::thread::spawn` raw OS thread** (NOT tokio blocking pool) with 60s timeout via `tokio::time::timeout`. This is critical — same pattern needed for the reranker to avoid runtime-shutdown deadlocks.
- **Tokenization** (lines 160-189): `tokenizer.encode(text, true)` returns `Encoding` with `get_ids()`, `get_attention_mask()`, `get_type_ids()`. Truncated to `MAX_TOKENS` (512 for MiniLM).
- **Tensor build** (lines 249-267): shape `(1, seq_len)`, `Tensor::from_array((shape, boxed_slice.into_boxed_slice()))`.
- **Inference call** (lines 271-280): `session.run(ort::inputs!["input_ids" => tensor, "attention_mask" => tensor, "token_type_ids" => tensor])` — named inputs scoped inside a lock guard.
- **Output extraction** (lines 283-298): `outputs.get("last_hidden_state")?.try_extract_tensor::<f32>()` then `.to_vec()`.
- **Constants**: `MODEL_NAME`, `DIMENSIONS`, `MAX_TOKENS` as `pub const` items.

### 1.2 ModelManager (download + cache)

`src/embedder/model_manager.rs` (471 lines) provides the download/cache pattern:

- **Default cache path**: `~/.vectorcode/models/minilm-l6-v2-q8/`
- **Files stored as**: `model.onnx` + `tokenizer.json` (always the local filename; HF path is the variant).
- **Platform-specific selection** (lines 27-41): `cfg!(target_arch = "aarch64")` → `onnx/model_qint8_arm64.onnx`; x86_64 → `onnx/model_quint8_avx2.onnx`; fallback to full `onnx/model.onnx`.
- **Download** (lines 85-147): reqwest + indicatif progress bar, atomic temp-file rename to avoid partial files.
- **Tests use mock HTTP server** (lines 361-408) — pattern reusable for reranker tests.

### 1.3 Search pipeline — integration point

`src/engine/searcher.rs` and `src/engine/fusion.rs` define the search contract:

- **`SearchStrategy` trait** (`searcher.rs:52`): `async fn search(&self, query, options) -> Result<Vec<SearchResult>>` + `fn mode() -> SearchMode`. Object-safe via `async-trait`.
- **`SearchMode` enum** (`searcher.rs:22-30`): `Dense | Sparse | Hybrid`. `clap::ValueEnum`, `FromStr`, default `Dense`.
- **`SearchOptions`** (`searcher.rs:62-75`): `limit`, `threshold`, `language`, `path`, `mode`, `rrf_k`.
- **`HybridSearcher`** (`fusion.rs:71`): runs `DenseSearcher` + `SparseSearcher` in parallel via `tokio::join!`, fuses with `rrf_fuse()`, graceful degradation on partial failure (already designed to fall back when one side fails — perfect template for "reranker fails → fall back to RRF order").
- **`build_strategy()` factory** (`searcher.rs:198-214`): builds the right strategy from `SearchMode` + dependencies. This is where we'll wire in the optional reranker.

### 1.4 Benchmark harness — multi-mode extension point

`src/bench/runner.rs` (365 lines):

- `run_benchmark(corpus, queries, embedder) -> BenchmarkResult` — single-mode today.
- Builds `Searcher::new(db, embedder, SearchConfig::default())` (line 73) — always dense.
- For multi-mode, we need to accept a `SearchMode` parameter and build the right strategy (using `build_strategy()`).
- Current `BenchmarkResult.aggregate` aggregates over a single mode. For Fase 1.5/1.6 comparison, we want a single report with rows per mode.

`src/bench/schema.rs`:

- `QueryResult` has `recall_at_5`, `recall_at_10`, `ndcg_at_10`, `mrr` — no `mode` field today.
- `BenchmarkResult` has aggregate metrics — no per-mode breakdown.

### 1.5 Config — where reranker settings go

`src/config/schema.rs`:

- `Config` has `provider`, `indexing`, `watcher`, `search` (line 9-14).
- `SearchConfig` (line 386-397): `default_limit`, `default_threshold`, `default_mode`, `rrf_k`. We can either:
  - **(a)** Extend `SearchConfig` with `rerank_enabled`, `rerank_top_k`, `rerank_timeout_ms`, `rerank_model`.
  - **(b)** Add a sibling `RerankConfig` struct at the same level as `SearchConfig`.
  - **(c)** Add a `RerankConfig` section in `ProviderConfig` (parallel to `OnnxConfig`/`GeminiConfig`).
- Recommendation: **(b)** — `RerankConfig` as a peer of `SearchConfig`. It's a separate concern (reranking vs searching), and we want it controllable even when reranking uses a different model than the embedder.
- `validate()` (line 74-177) already enforces mode strings — add `["dense", "sparse", "hybrid", "hybrid-rerank"]` to `valid_modes`.

### 1.6 CLI — search command needs new mode

`src/cli/search.rs`:

- `SearchArgs.mode: String` with `value_parser(["dense", "sparse", "hybrid"])` (line 41) — needs `hybrid-rerank` added.
- `build_strategy(mode, db, embedder, config.search.clone())` (line 107) — signature must accept the reranker.

### 1.7 Error type — already has `EmbedderError`

`src/error.rs` has `EmbedderError { message }` variant (line 22-23). Reusable for reranker errors as-is, no new variant needed.

---

## 2. ONNX model availability research

### 2.1 BGE-Reranker-v2-m3 (Apache 2.0, 568M params)

- **Source model**: `BAAI/bge-reranker-v2-m3` (HF, 2.27GB full safetensors)
- **ONNX conversion**: `onnx-community/bge-reranker-v2-m3-ONNX` exists
- **Available ONNX variants** (in `onnx/` folder):
  | File | Size | Notes |
  |------|------|-------|
  | `model.onnx` + `model.onnx_data` | 657KB + 2.27GB | full fp32 with external data |
  | `model_fp16.onnx` | 1.14GB | half-precision |
  | `model_int8.onnx` | **571MB** | dynamic int8 quantization (MatMul/Gemm) |
  | `model_q4.onnx` | 1.25GB | 4-bit MatMulNBits |
  | `model_q4f16.onnx` | 702MB | 4-bit with fp16 |
  | `model_uint8.onnx` | 571MB | uint8 |
  | `model_quantized.onnx` | 571MB | alias of int8 |
- **Architecture**: XLM-RoBERTa (12 layers, 568M params) + sequence classification head → single logit per (query, doc) pair
- **Pair encoding**: standard `[CLS] query [SEP] doc [SEP]` with token_type_ids
- **Output**: 1 logit → apply sigmoid for [0,1] probability
- **MTEB-Code score**: 41.38 (significantly lower than Qwen3 for code retrieval)

### 2.2 Qwen3-Reranker-0.6B (Apache 2.0, 0.6B params)

- **Source model**: `Qwen/Qwen3-Reranker-0.6B` (HF, 1.21GB safetensors)
- **ONNX conversion**: `onnx-community/Qwen3-Reranker-0.6B-ONNX` exists
- **Available ONNX variants**:
  | File | Format | Notes |
  |------|--------|-------|
  | `onnx/model_quantized.onnx` | **int8** | Dynamic int8 (MatMul/Gemm only) — usable with stock ort |
  | `onnx/model_q4.onnx` | 4-bit | `com.microsoft.MatMulNBits` (block_size=32) — requires this contrib op in ort |
- **Files needed**: `model_quantized.onnx` (~600MB) + `tokenizer.json` (11.4MB)
- **Architecture**: Qwen3 **CausalLM** (not a classifier) — 28 layers, 0.6B params
- **Inference pattern** (from HF README):
  1. Build chat prompt: `<|im_start|>system\nJudge whether the Document meets the requirements...<|im_end|>\n<|im_start|>user\n<Instruct>: {instruction}\n<Query>: {query}\n<Document>: {doc}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n`
  2. Tokenize + add prefix/suffix token IDs
  3. Run forward pass
  4. Extract `logits[:, -1, :]` — last position
  5. Index `tokenizer.convert_tokens_to_ids("yes")` and `"no"`
  6. `score = exp(yes_logit) / (exp(yes_logit) + exp(no_logit))`
- **Tokenizer config**: `padding_side = "left"`, `pad_token = eos_token`
- **Recommended `max_length`**: 8192 (32K context, but 8K is practical)
- **MTEB-Code score**: **73.42** — dramatically better than BGE for code retrieval

### 2.3 Comparison and recommendation

| Criterion | BGE-Reranker-v2-m3 | Qwen3-Reranker-0.6B |
|-----------|---------------------|----------------------|
| MTEB-Code (code retrieval) | 41.38 | **73.42** |
| Implementation complexity | **LOW** (sequence classification, 1 logit) | HIGH (CausalLM, chat template, yes/no logits) |
| ONNX maturity | Mature (multiple quant levels) | Brand new (1 commit, 131 downloads) |
| Download size (int8) | 571MB | ~600MB (quantized.onnx) |
| Context length | 512 tokens | 32K (use 8K in practice) |
| License | Apache 2.0 | Apache 2.0 |
| Production track record | "Más tiempo en producción" (roadmap) | Newer (Jun 2025) |

**Recommendation: lead with Qwen3-Reranker-0.6B as default** (roadmap spec — evaluated on MTEB-Code) **with BGE as the trait alternative**. The complexity of the Qwen3 CausalLM path is real but manageable — the HF model card documents the exact 8-step pattern, and the onnx-community ONNX export handles graph optimization (`com.microsoft.GroupQueryAttention` for the GQA fusion).

**Why both:** the `Reranker` trait (pilar 5 — architecture as contract) exists precisely so we don't paint ourselves into a corner. Start with Qwen3, keep BGE ready as a fallback if CausalLM-on-CPU turns out too slow in practice.

---

## 3. ort 2.0.0-rc.12 capabilities — feasibility check

From the official ort docs (https://ort.pyke.io/introduction, version 2.0.0-rc.12):

- ✅ `Session::builder()?` + `.with_optimization_level(GraphOptimizationLevel::Level3)?` + `.with_intra_threads(N)?` + `.commit_from_memory(bytes)?`
- ✅ `.with_execution_providers([CPU::default().build()])?` — works, already used in `OnnxEmbedder`
- ✅ `model.run(ort::inputs![...])?` — named input API
- ✅ `outputs.get("logits")?.try_extract_tensor::<f32>()` — already used for `last_hidden_state`
- ✅ Supports all ops needed for both BGE (RoBERTa layers, attention, LayerNorm, GELU) and Qwen3 (grouped-query attention, RoPE, RMSNorm, SwiGLU, embedding)
- ✅ Downloaded dylib via `features = ["download-binaries"]` — already enabled
- ⚠️ For Qwen3 q4 variant (`com.microsoft.MatMulNBits`): need to confirm this contrib op is in the standard ORT build; the int8 `model_quantized.onnx` uses only `MatMul`/`Gemm` quant which is universal — **safer to use the int8 variant for default**
- ✅ Multi-threaded CPU inference via `with_intra_threads(4)` — critical for cross-encoder latency

**Verdict: ort 2.0.0-rc.12 fully supports both reranker architectures. No new Cargo dependencies needed.**

---

## 4. Integration architecture — recommendation

### 4.1 Three options considered

| Option | Where reranker sits | Pros | Cons |
|--------|---------------------|------|------|
| **A. Inside `HybridSearcher`** | After RRF fusion, before return | Minimal new types; reuses graceful-degradation pattern; single `build_strategy` arm | Couples Hybrid to Reranker |
| **B. New `HybridRerankSearcher`** strategy | Separate `SearchStrategy` impl | Cleanest separation | More boilerplate; duplicates HybridSearcher logic |
| **C. Decorator pattern** | Wraps any `SearchStrategy` | Most flexible; composable | Adds indirection; harder to test in isolation |

### 4.2 Recommendation: **Option A** (inside `HybridSearcher`)

**Reasoning:**
1. The `HybridSearcher` already has the right shape — after fusion, before the final `take(limit)`, is exactly where reranking belongs.
2. The graceful-degradation pattern (one side fails → return other side) is the **same pattern** needed for "reranker fails → return RRF order". Reusing it is DRY.
3. The `Reranker` trait stays as a **pluggable dependency** (`Option<Arc<dyn Reranker>>` in `HybridSearcher::new`) — the trait is the pilar-5 contract, not the strategy hierarchy.
4. Only one new `SearchMode` variant (`HybridRerank`) needed; `build_strategy` adds one match arm.

**Architecture sketch:**

```rust
// src/reranker/mod.rs
#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<SearchResult>,
        top_k: usize,
    ) -> Result<Vec<SearchResult>>;
    fn model_name(&self) -> &str;
    fn is_available(&self) -> bool;  // for graceful skip
}

// src/reranker/onnx.rs
pub struct OnnxReranker {
    session: tokio::sync::Mutex<Session>,
    tokenizer: Tokenizer,
    model_name: String,  // "Qwen3-Reranker-0.6B" or "bge-reranker-v2-m3"
    model_variant: ModelVariant,  // CausalLM(Qwen3) | SequenceClassification(BGE)
    max_length: u32,
}

#[async_trait]
impl Reranker for OnnxReranker { /* … */ }

// src/engine/fusion.rs — extended
pub struct HybridSearcher {
    dense: Arc<dyn SearchStrategy>,
    sparse: Arc<dyn SearchStrategy>,
    reranker: Option<Arc<dyn Reranker>>,
    rrf_k: u32,
    rerank_top_k: usize,         // default 20
    rerank_timeout: Duration,     // default 5s
}

impl HybridSearcher {
    pub fn new_with_reranker(
        dense: Arc<dyn SearchStrategy>,
        sparse: Arc<dyn SearchStrategy>,
        reranker: Arc<dyn Reranker>,
        rrf_k: u32,
    ) -> Self { /* … */ }
}

#[async_trait]
impl SearchStrategy for HybridSearcher {
    async fn search(&self, query, options) -> Result<Vec<SearchResult>> {
        // … existing dense + sparse join + rrf_fuse …
        let fused = rrf_fuse(&[dense_results, sparse_results], rrf_k, limit);
        if let Some(reranker) = &self.reranker {
            // Take top-K for reranking, keep the rest as fallback
            let (rerank_pool, fallback) = split_for_rerank(fused, self.rerank_top_k);
            match tokio::time::timeout(
                self.rerank_timeout,
                reranker.rerank(query, rerank_pool, options.limit)
            ).await {
                Ok(Ok(reranked)) => Ok(reranked),
                Ok(Err(_)) | Err(_) => {
                    tracing::warn!("Reranker failed/timeout, falling back to RRF order");
                    Ok(fallback)
                }
            }
        } else {
            Ok(fused)
        }
    }
}
```

### 4.3 `SearchMode` extension

```rust
pub enum SearchMode {
    Dense,
    Sparse,
    Hybrid,
    HybridRerank,  // NEW
}
```

- `from_str` adds `"hybrid-rerank" => Ok(Self::HybridRerank)`
- `clap::ValueEnum` — clap converts `HybridRerank` → `hybrid-rerank` (kebab-case)
- `build_strategy` factory adds a new arm that builds `HybridSearcher` with the reranker wired in.
- `valid_modes` array in `Config::validate()` adds `"hybrid-rerank"`.

### 4.4 `RerankConfig` schema

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RerankConfig {
    /// Enable reranking (default: false in Fase 1.5 — opt-in).
    pub enabled: bool,
    /// Reranker model name. Supported: "qwen3-reranker-0.6b" (default), "bge-reranker-v2-m3".
    pub model: String,
    /// Quantization: "int8" (default, supported), "q4" (if MatMulNBits op is in build).
    pub quantization: String,
    /// Max candidates to rerank (default: 20). Lower = faster, less accurate.
    pub top_k: usize,
    /// Timeout for one rerank call in ms (default: 5000). On timeout → fallback to RRF.
    pub timeout_ms: u64,
    /// Max token length for (query, doc) pairs (default: 8192 for Qwen3, 512 for BGE).
    pub max_length: u32,
}
```

Add to `Config`:
```rust
pub struct Config {
    pub provider: ProviderConfig,
    pub indexing: IndexingConfig,
    pub watcher: WatcherConfig,
    pub search: SearchConfig,
    pub rerank: RerankConfig,  // NEW
}
```

Env var override: `VECTORCODE_RERANK_DISABLED=1` → `rerank.enabled = false`.

---

## 5. Files to create / modify

### NEW files (~600-800 LOC including tests)

| File | Purpose | ~LOC |
|------|---------|------|
| `src/reranker/mod.rs` | `Reranker` trait + re-exports | 60 |
| `src/reranker/onnx.rs` | `OnnxReranker` impl (Qwen3 + BGE variants) | 350-450 |
| `src/reranker/model_manager.rs` | Reranker model download/cache (parallels `embedder/model_manager.rs`) | 200-300 |

### MODIFIED files

| File | Change | ~LOC delta |
|------|--------|------------|
| `src/engine/mod.rs` | Re-export `Reranker`, `OnnxReranker`, `SearchMode::HybridRerank` | +5 |
| `src/engine/searcher.rs` | Add `HybridRerank` variant, `FromStr` + tests | +60 |
| `src/engine/fusion.rs` | `HybridSearcher` accepts `Option<Reranker>`, calls after fusion with timeout + fallback | +120 |
| `src/config/schema.rs` | `RerankConfig` struct, `Config.rerank` field, `validate()` rule, tests | +150 |
| `src/config/mod.rs` | Env override `VECTORCODE_RERANK_DISABLED` | +10 |
| `src/cli/search.rs` | `value_parser` adds `hybrid-rerank`, pass reranker to `build_strategy` | +20 |
| `src/cli/init.rs` | Optionally download reranker model if config requests it | +30 |
| `src/cli/mod.rs` | `create_reranker_from_config()` helper, wire into `create_embedder_from_config` flow | +40 |
| `src/bench/runner.rs` | Accept `SearchMode` + `Option<Arc<dyn Reranker>>`, build appropriate strategy | +50 |
| `src/bench/schema.rs` | `QueryResult.mode: String` field for per-mode attribution | +15 |
| `src/embedder/model_manager.rs` | Optional: extract a generic `download_with_progress` helper, share with reranker | +0 or refactor |
| `BASELINE.md` | Add Fase 1.5 results section after running benchmark | +50 |

**No new Cargo dependencies needed.** `ort` and `tokenizers` are already there.

---

## 6. Risks and unknowns

### 6.1 Implementation risks

1. **Qwen3 CausalLM ONNX inference pattern is non-trivial.** It requires:
   - Chat template tokenization (prefix + suffix tokens with `<|im_start|>` / `<|im_end|>`)
   - Left-padding (`padding_side = "left"`)
   - Reading the last-position logits
   - Indexing specific token IDs (`yes`, `no` from `tokenizer.convert_tokens_to_ids`)
   - Sigmoid over the two-token softmax
   - **Mitigation**: start by porting the exact 8-step pattern from the HF README, then write a unit test that loads the model and asserts scores on a known (query, doc) pair are reasonable (>0.9 for "Mars is the red planet" + relevant doc, <0.1 for "Mars" + "Jupiter is the largest planet").

2. **CPU latency on 0.6B Qwen3.** Cross-encoder does N inferences (one per candidate) — 20 candidates × ~50ms = ~1s added latency on CPU. **Mitigation**: int8 quantization (default), `top_k: 20` cap, 5s timeout with fallback.

3. **macOS CoreML EP hang** (known issue from the embedder). **Mitigation**: same `from_cache_with_timeout()` pattern with `std::thread::spawn` raw thread; respect `ORT_DISABLE_COREML`.

4. **`com.microsoft.MatMulNBits` op availability.** The q4 Qwen3 variant needs this contrib op. The int8 `model_quantized.onnx` only uses `MatMul`/`Gemm` quant — universal. **Mitigation**: default to int8, document q4 as experimental.

5. **First-time download size ~600MB.** Significant for `vectorcode init` if user opts in. **Mitigation**: lazy download (only on first rerank-enabled search, not at init), progress bar with ETA, document the size in the init prompt.

6. **Default OFF.** Reranking adds latency. Until benchmark proves it improves quality meaningfully, default `rerank.enabled = false`. User must opt in via config or `--mode hybrid-rerank`.

### 6.2 Design unknowns

1. **Qwen3 ONNX output name.** Need to confirm it's `"logits"` (standard CausalLM output). The onnx-community repo doesn't document this explicitly — verify with a probe load during implementation.

2. **Tokenizer output for Qwen3 chat template.** The `tokenizers` crate may not handle the `<|im_start|>` / `<|im_end|>` special tokens correctly out of the box — may need to manually add them as `additional_special_tokens` or use the chat template via the raw `Encode` API. Need to test.

3. **Benchmark budget.** The mini-corpus benchmark takes ~13s today (with Ollama embedder). Adding a Qwen3 ONNX model load (~2-3s first time, cached) + 20 rerank inferences per query × 15 queries (~15s) = +18s per run. Acceptable but worth measuring.

4. **Does the model need `temperature=0` / deterministic inference?** Cross-encoders produce logits (not sampled tokens) so this should be a non-issue, but verify.

5. **`IndexMeta` does not record rerank model** (and shouldn't — rerank doesn't affect stored vectors). If the user changes rerank model, no reindex needed. OK.

### 6.3 Process risks

1. **Roadmap specifies default Qwen3**, but the implementation effort is 3-5x BGE. If Fase 1.5 budget is tight, **fall back to BGE as Fase 1.5 default** and defer Qwen3 to a later fase. The trait keeps both viable.
2. **Fase 1.6 (re-measure) is the actual decision gate** — if reranker doesn't improve recall/nDCG measurably on the mini corpus, it's reverted. So the implementation must include a clean benchmark comparison artifact.
3. **Roadmap explicitly cites "elimina por diseño el bug de concurrencia de Proyecto B"** — the whole point is "single local inference, no LLM loop". This must be the design north star. No async pipelines, no `tokio::spawn` per candidate, no shared mutable state across candidates. Just one `session.run()` per (query, doc) pair, with a global timeout wrapper.

---

## 7. Recommended implementation order

1. **`Reranker` trait** in `src/reranker/mod.rs` with `MockReranker` for tests.
2. **`RerankConfig`** in `src/config/schema.rs` + `Config::rerank` field + validation.
3. **`SearchMode::HybridRerank`** enum variant + `from_str` + `build_strategy` factory arm.
4. **`OnnxReranker` with BGE first** (simpler — sequence classification). Ship as the working default.
5. **`RerankerModelManager`** for download/cache, parallel to `embedder/model_manager.rs`.
6. **Wire into `HybridSearcher`** with timeout + fallback to RRF.
7. **Wire into CLI** (search `--mode hybrid-rerank`, init download).
8. **Wire into benchmark** (add `mode` to `QueryResult`, run dense vs hybrid vs hybrid-rerank).
9. **Fase 1.5 verification**: run mini benchmark, compare to BASELINE.md Fase 1.3-1.4 numbers.
10. **(Stretch) Qwen3 CausalLM variant** — port chat template, add as alternative via the trait.

---

## 8. Ready for proposal?

**Yes.** The architecture is clear:
- The existing ONNX embedder provides a battle-tested template we can clone with minor adaptations.
- `ort 2.0.0-rc.12` fully supports both BGE and Qwen3 architectures — no new dependencies.
- The `Reranker` trait slots cleanly into the existing `HybridSearcher` graceful-degradation pattern.
- `RerankConfig` extends the config schema without disrupting existing fields.
- Multi-mode benchmark extension is straightforward.
- The single biggest risk (Qwen3 CausalLM complexity) is mitigated by starting with BGE and treating Qwen3 as a stretch goal.

**Key recommendation for the orchestrator:** start the proposal with BGE-Reranker-v2-m3 as the **initial implementation target** (simpler, proven ONNX, satisfies the trait-as-contract pilar), with Qwen3-Reranker-0.6B as the **documented future alternative** behind the same `Reranker` trait. This de-risks Fase 1.5 substantially while keeping the architecture aligned with the roadmap.
