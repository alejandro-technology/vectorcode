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

## MCP Tools — Combined Workflow

Three MCP servers are available: **VectorCode** (semantic search), **Codegraph** (static analysis / call-graph), and **Engram** (persistent memory). They serve different stages of understanding code.

### Mental Model

| MCP | Role | Answers |
|-----|------|---------|
| VectorCode | Discovery | "What code is relevant to this concept?" |
| Codegraph | Understanding | "How does this specific code work? Who calls it?" |
| Engram | Persistence | "What did we learn or decide in past sessions?" |

They do NOT compete — they address different phases of the same workflow.

### Before Every Task — Context Recovery (Engram)

```
engram_mem_context        → recent session history (fast, cheap)
engram_mem_search "query" → full-text search across all sessions
engram_mem_get_observation → untruncated content by ID
```

Always check memory when the user references past work ("remember", "recall", "what did we do"), OR when their FIRST message references the project, a feature, or a problem — search proactively.

### Phase 1 — Discover by Concept (VectorCode)

Use when you do NOT know the file or symbol name — you only know what the code does.

| Tool | When to use |
|------|-------------|
| `vec_search "description of behavior"` | Primary discovery tool. Always add `language` and `path` filters when possible. Adjust `threshold` to filter noise (>0.6) or widen scope (<0.3). |
| `vec_outline "path/to/file"` | Get structural overview of a discovered file before reading it whole. |
| `vec_read_lines "path" start end` | Expand context around a specific snippet from search results. |
| `vec_status` | Check if the index is up to date before searching. |
| `vec_reindex full=false` | Incremental re-index if files changed since last index. |

**Pattern**: `vec_search` → `vec_outline` → then switch to Codegraph for deep understanding.

### Phase 2 — Understand Structure & Relationships (Codegraph)

Use when you know symbol or file names and need precise source, callers, callees, or flow.

| Tool | When to use | Priority |
|------|-------------|----------|
| `codegraph_explore "SymA SymB"` | **PRIMARY** — answers most questions in one call. Returns verbatim source of all matching symbols + call paths between them. Use for: "how does X work?", "what's the flow from A to B?", or before editing multiple related symbols. | ★★★ |
| `codegraph_node file="path/to/file"` | Read a source file with line numbers, plus which files depend on it (blast radius). Prefer this over the `Read` tool for source files — same bytes, faster, includes dependents. | ★★★ |
| `codegraph_node symbol="X" includeCode=true` | Single symbol — definition, signature, full body, callers/callees. Use before editing to see impact. | ★★☆ |
| `codegraph_callers "functionName"` | Trace who calls a function before refactoring or deleting. Pass `file` to disambiguate overloaded names. | ★★☆ |
| `codegraph_search "nameFragment"` | Quick locate by partial name when `codegraph_explore` is overkill. Returns locations only, no code. | ★☆☆ |

**Critical rule**: Prefer `codegraph_explore` over sequential `grep` + `Read` loops. It returns more accurate context in far fewer calls.

### Phase 3 — Persist Knowledge (Engram)

| Tool | Trigger |
|------|---------|
| `mem_save` | **PROACTIVE and IMMEDIATE** after: architecture decision, bug fix (with root cause), configuration change, non-obvious discovery, pattern established. Format: `title` = verb + what. `content` = `**What**` / `**Why**` / `**Where**` / `**Learned**`. Set `topic_key` for evolving decisions to avoid scattering. |
| `mem_session_summary` | **MANDATORY** before session ends. Structure: Goal / Instructions / Discoveries / Accomplished / Next Steps / Relevant Files. |
| `mem_suggest_topic_key` | Before `mem_save` on evolving topics (architecture decisions) to reuse the same key and update a single observation over time. |

### Decision Tree — Which Tool First?

```
User asks a task
│
├─ References past work? ("remember", "recall", "what did we...")
│  → mem_context → mem_search → mem_get_observation
│
├─ "Where is the code that does <concept>?" (don't know file/symbol)
│  → vec_search (discovery) → vec_outline (structure) → codegraph_explore (deep dive)
│
├─ "How does <known symbol> work?" or "Show me <known file>"
│  → codegraph_explore (multiple symbols) or codegraph_node (single file/symbol)
│
├─ Before editing a symbol
│  → codegraph_node symbol="X" includeCode=true (see callers + callees = blast radius)
│
├─ After completing significant work
│  → mem_save (persist what was learned or decided)
│
└─ Session ending
   → mem_session_summary (structured handoff to next session)
```

### Anti-patterns to Avoid

- ❌ `grep` + `Read` loop to understand code flow → use `codegraph_explore` instead.
- ❌ `vec_search` for exact symbol lookup → use `codegraph_search` or `codegraph_node`.
- ❌ `codegraph_explore` for fuzzy conceptual search (no symbol names) → use `vec_search`.
- ❌ Skipping `mem_save` after a bug fix or decision → next session starts blind.
- ❌ Reading files with `Read` when `codegraph_node file="..."` gives the same bytes + dependents.

### Quick Reference Card

```
Discover  → vec_search       "concept" language="..." path="..."
Preview   → vec_outline      "file"
Deep dive → codegraph_explore "SymA SymB SymC"
Read file → codegraph_node    file="path"
Edit prep → codegraph_node    symbol="X" includeCode=true
Persist   → mem_save          title="..." type="decision|bugfix|discovery|..."
Handoff   → mem_session_summary
```
