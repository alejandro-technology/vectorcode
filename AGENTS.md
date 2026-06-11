# AGENTS.md

## Build & Run

```bash
cargo build
cargo run -- <args>
cargo run -- serve --mcp        # start MCP server
cargo run -- init               # initialize a project
cargo run -- index              # index codebase
cargo run -- search "query"     # semantic search
```

## Testing

```bash
cargo test --all-targets        # run all tests
cargo test --lib                # unit tests only
cargo test --test '*'           # integration tests only
cargo test <test_name>          # single test
```

## Lint & Format

```bash
cargo fmt                       # auto-format
cargo fmt --check               # check formatting (CI)
cargo clippy --all-targets -- -D warnings   # lint (CI)
```

All three (test, clippy, fmt --check) MUST pass before committing. CI enforces this.

## Architecture

```
src/
├── main.rs          # entry point — clap dispatch
├── lib.rs           # module root + re-exports
├── error.rs         # VectorCodeError (thiserror)
├── types.rs         # Chunk, SearchResult, IndexMeta
├── cli/             # one file per subcommand (init, index, search, serve, status, install, uninstall, upgrade)
├── config/          # TOML config schema + loader (.vectorcode/config.toml)
├── embedder/        # Embedder trait + providers (ONNX, Gemini, Ollama, OpenAI)
├── engine/          # core orchestration: chunker (tree-sitter) → embedder → store
├── mcp/             # MCP server: JSON-RPC 2.0 over stdio, tool handlers
├── store/           # SQLite + sqlite-vec: db, chunks, files, meta tables
└── watcher/         # file watcher (notify crate, debounced, gitignore-aware)
```

## Key Conventions

- **Rust edition**: 2021, MSRV 1.75
- **Error handling**: `VectorCodeError` (thiserror) for library code, `anyhow::Result` in `main.rs` and CLI handlers
- **Async**: tokio runtime, `async-trait` for trait objects
- **CLI**: clap with derive macros — one module per subcommand under `src/cli/`
- **MCP protocol**: JSON-RPC 2.0 over stdio — schema types in `src/mcp/schema.rs`, handler dispatch in `src/mcp/server.rs`
- **Embedding providers**: implement the `Embedder` trait from `src/embedder/mod.rs`
- **Database**: rusqlite with bundled sqlite + sqlite-vec extension
- **Tree-sitter**: one grammar per language, used by the chunker in `src/engine/`
- **Tests**: unit tests inline (`#[cfg(test)] mod tests`), integration tests in `tests/`
- **No unwrap/expect** in library code — propagate errors with `?`

## Adding a New Embedding Provider

1. Create `src/embedder/<provider>.rs`
2. Implement the `Embedder` trait (async `embed` + `embed_batch`)
3. Register in the provider factory in `src/embedder/mod.rs`
4. Add config schema to `src/config/schema.rs`
5. Add tests

## Adding a New Tree-sitter Language

1. Add the grammar crate to `Cargo.toml`
2. Register in the chunker's language dispatch in `src/engine/`
3. Add the extension mapping to `src/types.rs`

## Environment Variables

| Variable | Purpose |
|---|---|
| `GEMINI_API_KEY` | Gemini embedding API key |
| `OPENAI_API_KEY` | OpenAI embedding API key |
| `VECTORCODE_PROVIDER` | Override provider |
| `VECTORCODE_NO_WATCH` | Set `1` to disable file watcher |
| `RUST_LOG` | tracing filter (e.g. `debug`, `vectorcode=trace`) |

## Project Data

- Config: `.vectorcode/config.toml`
- Index DB: `.vectorcode/index.db`
- Both are gitignored — never commit them
