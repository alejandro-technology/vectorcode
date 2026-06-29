<div align="center">

<img width="1920" height="1080" alt="VectorCode banner" src="docs/assets/VectorCodeBanner.webp" />

<h1>VectorCode</h1>

<p><strong>Semantic code search MCP server using embeddings. Find code by meaning, not just by name.</strong></p>

<p>
<a href="https://github.com/alejandro-technology/vectorcode/actions/workflows/coverage.yml"><img src="https://github.com/alejandro-technology/vectorcode/actions/workflows/coverage.yml/badge.svg" alt="Coverage"></a>
<a href="https://github.com/alejandro-technology/vectorcode/actions/workflows/ci.yml"><img src="https://github.com/alejandro-technology/vectorcode/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
<img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey" alt="Platform">


</p>

</div>

## What is VectorCode?

VectorCode fills the gap between exact string matching (`grep`) and structural analysis (CodeGraph). It enables **semantic search** over your codebase — finding code by concept when you don't know the exact symbol name, pattern, or terminology.

> **Honest status:** see [`docs/STATUS.md`](docs/STATUS.md) for the per-pilar
> verdict (P1-P7) and the deep dives under `docs/pilar-status/`. Every
> claim in this README is cross-checked against that index.

**Example queries that VectorCode answers:**

- "code that handles payment retries"
- "where do we validate user permissions"
- "functions similar to createUser"
- "error recovery logic"

## How It Works

1. **Chunk** — Source files are parsed with tree-sitter into semantically meaningful chunks (functions, classes, methods)
2. **Embed** — Each chunk is converted to a vector embedding using your chosen provider (ONNX, Gemini, Ollama, OpenAI)
3. **Store** — Vectors are stored in SQLite with `sqlite-vec` for fast similarity search
4. **Search** — Natural language queries are embedded and compared via cosine similarity
5. **Watch** — A file watcher auto-syncs the index when files change (debounced, gitignore-aware)

## Installation

### From Source (requires Rust 1.75+)

```bash
cargo install --path .
```

### Using install.sh (macOS/Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/alejandro-technology/vectorcode/main/install.sh | bash
```

### Configure Your Agent

```bash
vectorcode install
```

This auto-detects your AI coding agents and adds VectorCode to their MCP configuration.

Supported agents:

- **OpenCode** — `opencode.json` → `mcpServers`
- **Claude Code** — `~/.claude/claude_desktop_config.json`
- **Cursor** — `.cursor/mcp.json`
- **Gemini CLI** — `~/.gemini/settings.json`
- **Antigravity** — `~/.gemini/antigravity/settings.json`

## Usage

### Initialize a Project

```bash
cd your-project
vectorcode init
```

Options:

- `--provider <onnx|gemini|ollama|openai>` — Embedding provider (default: onnx)
- `--model <name>` — Model name for the provider
- `--dims <n>` — Embedding dimensions
- `--index` — Also run initial indexing

### Index Your Codebase

```bash
# Full index
vectorcode index

# Index specific file
vectorcode index --file src/auth.ts

# Full reindex (drop and rebuild)
vectorcode index --full

# Custom concurrency
vectorcode index --concurrency 16
```

### Search

```bash
# Basic search
vectorcode search "payment retry logic"

# With filters
vectorcode search "auth middleware" --language typescript --path src/

# Search modes
vectorcode search --mode dense "query"      # Dense vector search (default)
vectorcode search --mode sparse "query"     # BM25 lexical search (FTS5)
vectorcode search --mode hybrid "query"     # Dense + Sparse RRF fusion
vectorcode search --mode hybrid-rerank "query"  # Hybrid + ONNX cross-encoder reranking

# JSON output
vectorcode search "error handling" --json

# Custom limit and threshold
vectorcode search "database connection" --limit 20 --threshold 0.5
```

### Reranker (Hybrid+Rerank Mode)

VectorCode supports an optional ONNX cross-encoder reranker that re-scores the
top-K hybrid search results for higher precision. The reranker runs locally
(no API calls) using the [BGE-Reranker-v2-m3](https://huggingface.co/Xenova/bge-reranker-v2-m3) model (~571MB).

```bash
# Enable reranker in config (.vectorcode/config.toml):
[search.rerank]
enabled = true
top_k = 20          # Re-rank top 20 hybrid results
timeout_ms = 5000   # Fallback to hybrid if reranker exceeds timeout
```

If the reranker fails to load or times out, search gracefully falls back to
plain hybrid mode — no errors, no interrupted queries.

### MCP Server

```bash
# Start the MCP server (used by AI agents)
vectorcode serve --mcp

# Disable file watcher
vectorcode serve --mcp --no-watch

# Custom debounce interval
vectorcode serve --mcp --debounce 5000
```

### Status

```bash
vectorcode status
```

### Install/Uninstall

```bash
# Auto-configure all detected agents
vectorcode install

# Configure specific agent
vectorcode install --target opencode

# Remove from all agents
vectorcode uninstall

# Remove from specific agent
vectorcode uninstall --target cursor
```

## Configuration

Configuration is stored in `.vectorcode/config.toml`:

```toml
[provider]
name = "onnx"  # onnx | gemini | ollama | openai

[provider.gemini]
api_key = "your-api-key"
model = "gemini-embedding-2"
dimensions = 768

[provider.ollama]
url = "http://localhost:11434"
model = "embeddinggemma:latest"

[provider.openai]
api_key = "your-api-key"
model = "text-embedding-3-small"

[indexing]
max_file_size = 1048576   # 1MB
concurrency = 8
exclude_dirs = [".vectorcode", ".git", "node_modules", "target"]
exclude_extensions = [".min.js", ".map", ".lock"]

[watcher]
debounce_ms = 2000
disabled = false

[search]
default_limit = 10
default_threshold = 0.3
mode = "dense"            # dense | sparse | hybrid | hybrid-rerank

[search.rrf]
k = 60                    # RRF fusion constant

[search.rerank]
enabled = false
top_k = 20                # Re-rank top-K hybrid results
timeout_ms = 5000         # Fallback to hybrid on timeout
```

### Environment Variable Overrides

| Variable                 | Description                   |
| ------------------------ | ----------------------------- |
| `VECTORCODE_PROVIDER`    | Override provider name        |
| `GEMINI_API_KEY`         | Gemini API key                |
| `OPENAI_API_KEY`         | OpenAI API key                |
| `VECTORCODE_NO_WATCH`    | Set to `1` to disable watcher |
| `VECTORCODE_DEBOUNCE_MS` | Override debounce interval    |

## Supported Languages

VectorCode parses 14 languages via tree-sitter. All 14 are chunked; 3 of
them (Rust, the TS/JS family, Python) also emit graph edges. The
per-pilar deep dive at
[`docs/pilar-status/P3-estructura-ast-grafo.md`](docs/pilar-status/P3-estructura-ast-grafo.md)
documents the language × edge-type matrix.

| Language   | Extensions                          | Tree-sitter Grammar      | Graph edges |
| ---------- | ----------------------------------- | ------------------------ | ----------- |
| TypeScript | `.ts`                               | tree-sitter-typescript   | Call, Import |
| TSX        | `.tsx`                              | tree-sitter-typescript   | Call, Import |
| JavaScript | `.js`, `.mjs`, `.cjs`               | tree-sitter-javascript   | Call, Import |
| JSX        | `.jsx`                              | tree-sitter-javascript   | Call, Import |
| Python     | `.py`                               | tree-sitter-python       | Call, Import |
| Rust       | `.rs`                               | tree-sitter-rust         | Call, Import |
| Go         | `.go`                               | tree-sitter-go           | —           |
| Java       | `.java`                             | tree-sitter-java         | —           |
| C#         | `.cs`                               | tree-sitter-c-sharp      | —           |
| C          | `.c`, `.h`                          | tree-sitter-c            | —           |
| C++        | `.cpp`, `.hpp`, `.cc`, `.cxx`       | tree-sitter-cpp          | —           |
| Ruby       | `.rb`                               | tree-sitter-ruby         | —           |
| Swift      | `.swift`                            | tree-sitter-swift        | —           |
| Kotlin     | `.kt`, `.kts`                       | tree-sitter-kotlin-ng    | —           |

## MCP Tools

When running as an MCP server, VectorCode exposes the following tools:

### `vec_search`

Semantic code search — find code by meaning, not just by name.

Parameters:

- `query` (required) — Natural language description of what you're looking for
- `limit` (optional, default: 10) — Maximum results (max: 100)
- `threshold` (optional, default: 0.3) — Minimum similarity score (0.0–1.0)
- `language` (optional) — Filter by language
- `path` (optional) — Filter by file path prefix

### `vec_status`

Check the status of the VectorCode index, including provider, dimensions, number of files indexed, and last sync time.

### `vec_reindex`

Trigger a background re-index of the project.

Parameters:

- `full` (required) — Set to true to drop the index and start fresh

### `vec_read_lines`

Read a specific range of lines from a file. Use this instead of generic file reading when you only need to expand the context around a snippet found via vec_search.

Parameters:

- `file_path` (required) — The file path to read
- `start_line` (required) — The starting line number (1-indexed, inclusive)
- `end_line` (required) — The ending line number (1-indexed, inclusive)

Notes:
- Max 500 lines per call
- Max file size: 2MB
- Path must be within project bounds

### `vec_outline`

Get a structural outline of a source file — top-level functions, classes, structs, interfaces, and traits with their signatures. Useful for understanding file structure without reading the entire file.

Parameters:

- `file_path` (required) — The file path to outline (relative to project root)

Notes:
- Max file size: 2MB
- Path must be within project bounds

### `vec_find_callers`

Find functions or methods that call a given symbol. Uses the graph port
(`src/store/graph.rs`). Currently emits `Call` and `Import` edges for
Rust, the TS/JS family, and Python; returns an empty list for languages
without a graph extractor.

Parameters:

- `symbol` (required) — Symbol name to search for (function, method, or
  fully-qualified path)

### `vec_find_dependents`

Find symbols that depend on the given one (e.g. via class extension or
symbol reference). Backed by the graph port's `get_dependents` method.

Parameters:

- `symbol` (required) — Symbol to find dependents for
- `file_path` (optional) — Restrict the search to a single file

### `vec_trace_imports`

Trace the import graph for a symbol: every file that imports it, and
every symbol those files re-export. Backed by `GraphStore::get_imports`.

Parameters:

- `symbol` (required) — Symbol to trace imports for
- `file_path` (optional) — Restrict to imports within a single file

> Honest status: see [docs/STATUS.md](docs/STATUS.md) for the per-pilar
> verdict (P1-P7) and the deep dives under `docs/pilar-status/`.

## Benchmarks

Our benchmarking efforts align with formal research terminology for LLM agents and Augmented Retrieval Systems. The project is divided into three major evaluation phases:

| Phase | Academic Taxonomy | Measurement Focus | Status |
|-------|-------------------|-------------------|--------|
| **1** | **Retrieval Evaluation** (Information Retrieval Benchmark) | Quality of the retriever (Recall, Precision, MRR, nDCG). | ✅ **Implemented** |
| **2** | **End-to-End Agent Evaluation** (Task-Oriented Benchmark) | Agent's efficiency and success rate using tools. | 🚧 *WIP* |
| **3** | **Context Efficiency Evaluation** (Long-Context Efficiency) | Token cost and RAG system scalability. | 🚧 *WIP* |

For full details on the testing methodology, query sets, and historical evolution, see [`benchmarks/README.md`](benchmarks/README.md) and [`BASELINE.md`](BASELINE.md).

### Phase 1: Retrieval Evaluation

Measurements taken using the `mini` integration corpus (Rust, TypeScript, Python) with the `embeddinggemma` model across different retrieval strategies.

#### Semantic Search (IR Quality)

| Mode | Recall@5 | nDCG@10 | MRR | Latency | Note |
|------|----------|---------|-----|---------|------|
| **Dense** (Vector) | 0.2667 | 0.1983 | 0.2333 | 11.6s | Pure semantic search |
| **Sparse** (FTS5) | 0.0333 | 0.0469 | 0.0667 | 9.2s | Pure lexical search |
| **Hybrid** (RRF) | 0.2000 | 0.1417 | 0.1389 | 11.3s | Dense + Sparse fusion |
| **Hybrid+Rerank** | **0.2000** | **0.2083** | **0.3000** | 32.6s | Re-ranked with ONNX cross-encoder |

*Adding the ONNX cross-encoder reranker improves ranking quality (MRR) by 116% over standard hybrid search, though at the cost of higher CPU latency.*

#### Structural Search (Knowledge Graph)

Measurements for exact symbol resolution (callers, dependents, imports) utilizing the extracted syntax tree graph.

| Metric | Result |
|--------|--------|
| **Symbol Recall@5** | 100% (1.00) |
| **Symbol Recall@10** | 100% (1.00) |
| **Symbol Precision@5** | 65% (0.65)* |

*\*Precision reflects that structural queries return exact sets (often <5 results), not ranked lists. Recall is 100%, meaning the graph never misses a known dependency.*

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     vectorcode (Rust binary)                │
│                                                             │
│  ┌──────────┐   ┌──────────────┐   ┌─────────────────────┐ │
│  │ CLI      │   │ MCP Server   │   │ File Watcher        │ │
│  │ (clap)   │   │ (stdio JSON- │   │ (notify crate,      │ │
│  │          │   │  RPC)        │   │  debounced)          │ │
│  └────┬─────┘   └──────┬───────┘   └──────────┬──────────┘ │
│       │                │                       │            │
│       └────────┬───────┴───────────────────────┘            │
│                │                                            │
│       ┌────────▼────────┐                                   │
│       │   Core Engine   │                                   │
│       │  ┌───────────┐  │  Tree-sitter AST parsing          │
│       │  │ Chunker   │  │                                   │
│       │  └─────┬─────┘  │                                   │
│       │        │        │                                   │
│       │  ┌─────▼─────┐  │  Provider trait (ONNX/Gemini/     │
│       │  │ Embedder  │  │  Ollama/OpenAI)                   │
│       │  │ (trait)   │  │                                   │
│       │  └─────┬─────┘  │                                   │
│       │        │        │                                   │
│       │  ┌─────▼─────┐  │  SQLite + sqlite-vec              │
│       │  │ Store     │  │  (.vectorcode/index.db)           │
│       │  └───────────┘  │                                   │
│       └────────────────┘                                   │
└─────────────────────────────────────────────────────────────┘
```

## Security

VectorCode enforces path-boundary checks across MCP handlers, CLI
commands, and the indexer to prevent reading or embedding files outside
the initialized workspace. See [`docs/SECURITY.md`](docs/SECURITY.md)
for the full threat model, validated defenses, and known limits
(deferred items like root allowlist, gitignore read-gate, TOCTOU fix,
and rate limiting).

Quick rules of thumb:
- Initialize VectorCode in a dedicated project directory, not in `$HOME` or `/`.
- Keep secrets in `.gitignore` — VectorCode respects it during indexing.
- Do not point the MCP client at system roots.

## License

MIT