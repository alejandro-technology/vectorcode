# P1 — Local-first, sin excepciones silenciosas

> Verdict: **85%** — 6 embedder providers with hard-fail on missing API keys, default is local ONNX, and no telemetry crate in `Cargo.toml`. The remaining 15% covers first-run network calls and the missing `--list-providers` affordance.

## Verdict

VectorCode is local-first by default. The default provider is ONNX (no API key, no network at query time), and every cloud provider hard-fails with `ApiKeyMissing` if the configured key is empty — there is no silent cloud fallback. The whole `Embedder` port is object-safe and has 6 production impls plus 1 deterministic test variant. `Cargo.toml` has zero telemetry / analytics / auto-update crates. Two known limits block the final 15%: the first run downloads the ONNX model and the BGE reranker from the HuggingFace CDN, and the CLI has no `--list-providers` command to show which providers are local.

## Evidence

- **Embedder port with 6 production impls** — `src/embedder/mod.rs:26` defines the `Embedder` trait; 6 production impls exist (`OnnxEmbedder` at `src/embedder/onnx.rs:242`, `OllamaEmbedder` at `src/embedder/ollama.rs:243`, `OpenAiEmbedder` at `src/embedder/openai.rs:112`, `GeminiEmbedder` at `src/embedder/gemini.rs:169`, `OpenRouterEmbedder` at `src/embedder/openrouter.rs:127`, `MockEmbedder` at `src/embedder/mock.rs:53`). A 7th test-only impl `MockDeterministicEmbedder` lives at `src/embedder/mock.rs:97` for the benchmark harness.
- **Default is ONNX, factory dispatch** — `src/cli/mod.rs:137` `create_embedder_from_config` is the single dispatch point; ONNX branch at `:143-152` loads via `from_cache_with_timeout()` (no network at query time). API providers (gemini `:153`, openai `:174`, openrouter `:182`) are explicit, not automatic.
- **Hard-fail on missing API key** — `src/embedder/openai.rs:51-58` returns `VectorCodeError::ApiKeyMissing` when the key is empty; `src/embedder/gemini.rs:66-71` does the same. The same pattern is mirrored in `openrouter.rs` (see `src/embedder/openrouter.rs:127-140`).
- **No telemetry in dependencies** — `Cargo.toml` lists `clap`, `tokio`, `rusqlite`, `sqlite-vec`, tree-sitter grammars, `ort`, `tokenizers`, `reqwest`, `serde`, `serde_json`, `toml`, `notify`, `ignore`, `blake3`. No `opentelemetry`, `sentry`, `posthog`, `mixpanel`, `datadog`, or auto-update crate. `rg "telemetry|analytics" Cargo.toml` returns 0 matches.
- **ONNX / Ollama skip the API key prompt entirely** — `src/cli/init.rs:266-273` `api_key_env_var` returns `""` for `Onnx` and `Ollama`; `provider_requires_api_key` (`:276-278`) gates the prompt on that. ONNX users never see an API key question.
- **File watcher is local-only** — `src/watcher/mod.rs` uses `notify` + `notify-debouncer-full`; it reads the local filesystem and respects `.gitignore` via the `ignore` crate. No outbound HTTP from the watcher.

## Known limits

- **First-run ONNX model download** — `src/embedder/model_manager.rs:85-90` calls `download_model()` which builds the URL as `{HF_BASE_URL}/{ONNX_MODEL_PATH}`. The path is selected per-platform in `:27-41`: `model_qint8_arm64.onnx` (~23 MB) on ARM64 macOS / Linux, `model_quint8_avx2.onnx` (~23 MB) on x86_64, `model.onnx` (~90 MB) as fallback. After the first run the model is cached at `~/.vectorcode/models/minilm-l6-v2-q8/` and no further network call is made. Air-gapped users must pre-cache.
- **First-run reranker download** — `src/reranker/onnx.rs:24-25` points at `huggingface.co/onnx-community/bge-reranker-v2-m3-ONNX`; the model is ~571 MB (per the file's own header comment at `:1-5`). Same caveat as the embedder: cached after first run.
- **No automatic network check on startup** — selecting ONNX does not verify HF CDN reachability. If the CDN is down on first run, `init` fails with a download error. A `--check-network` flag would be additive but does not exist.
- **No `--list-providers` command** — the user has to read the README to know which of the 6 providers are local. A `vectorcode providers` subcommand printing the local vs. cloud classification would close this gap.
- **Cloud providers (OpenAI, Gemini, OpenRouter) are opt-in but indistinguishable from local at the prompt level** — the `init` flow shows the same numbered list for everyone; a first-class "I want local-only" mode is not exposed.

## Links

- [BASELINE.md](../../BASELINE.md) — original Phase 1.2 dense-search baseline (Ollama / `embeddinggemma:latest`, mini corpus, R@5 = 0.30).
- [ADR-0001](adr/0001-store-choice.md) — store choice + the "LanceDB honesty" section that exemplifies P7-style disclosure.
- [docs/SECURITY.md](SECURITY.md) — threat model and validated defenses from phase-4.2.
- Related: [P5](P5-arquitectura-contrato.md) (port contract) · [P7](P7-honestidad.md) (this doc is part of the P7 fix).
