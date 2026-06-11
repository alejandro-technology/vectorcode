# VectorCode

Semantic code search MCP server using embeddings. Find code by meaning, not just by name.

## What is VectorCode?

VectorCode fills the gap between exact string matching (`grep`) and structural analysis (CodeGraph). It enables **semantic search** over your codebase — finding code by concept when you don't know the exact symbol name, pattern, or terminology.

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

# JSON output
vectorcode search "error handling" --json

# Custom limit and threshold
vectorcode search "database connection" --limit 20 --threshold 0.5
```

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
model = "gemini-embedding-001"
dimensions = 768

[provider.ollama]
url = "http://localhost:11434"
model = "nomic-embed-text"

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
```

### Environment Variable Overrides

| Variable | Description |
|----------|-------------|
| `VECTORCODE_PROVIDER` | Override provider name |
| `GEMINI_API_KEY` | Gemini API key |
| `OPENAI_API_KEY` | OpenAI API key |
| `VECTORCODE_NO_WATCH` | Set to `1` to disable watcher |
| `VECTORCODE_DEBOUNCE_MS` | Override debounce interval |

## Supported Languages

| Language | Extensions | Tree-sitter Grammar |
|----------|-----------|-------------------|
| TypeScript | `.ts` | tree-sitter-typescript |
| TSX | `.tsx` | tree-sitter-typescript |
| JavaScript | `.js`, `.jsx`, `.mjs`, `.cjs` | tree-sitter-javascript |
| Python | `.py` | tree-sitter-python |
| Rust | `.rs` | tree-sitter-rust |
| Go | `.go` | tree-sitter-go |
| Java | `.java` | tree-sitter-java |

## MCP Tools

When running as an MCP server, VectorCode exposes three tools:

### `vec_search`
Semantic code search — find code by meaning, not just by name.

Parameters:
- `query` (required) — Natural language description of what you're looking for
- `limit` (optional, default: 10) — Maximum results
- `threshold` (optional, default: 0.3) — Minimum similarity score (0.0–1.0)
- `language` (optional) — Filter by language
- `path` (optional) — Filter by file path prefix

### `vec_status`
Check the status of the VectorCode index.

### `vec_reindex`
Force re-indexing of the codebase or specific files.

Parameters:
- `path` (optional) — Specific file or directory
- `full` (optional, default: false) — Drop and rebuild from scratch

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

## License

MIT
