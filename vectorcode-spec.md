# VectorCode — Semantic Code Search MCP Server

> **Target audience**: SDD orchestrator + subagents. This document is the single source of truth for building the entire tool.

## 1. Project Identity

| Field | Value |
|---|---|
| **Name** | `vectorcode` |
| **Language** | Rust (2021 edition, MSRV 1.75+) |
| **Binary** | Single statically-linked binary |
| **Protocol** | MCP (Model Context Protocol) over stdio |
| **Storage** | SQLite + `sqlite-vec` extension (single file) |
| **License** | MIT |
| **Platforms** | macOS (arm64, x86_64), Linux (x86_64, arm64), Windows (x86_64) |

---

## 2. Problem Statement

AI coding agents (OpenCode, Claude Code, Cursor, Gemini CLI) navigate codebases using two strategies:

1. **Exact match** — `grep`, `ripgrep`, literal string search
2. **Structural** — CodeGraph provides symbol-level knowledge graphs (callers, callees, impact)

Neither supports **semantic search**: finding code by concept when the developer doesn't know the exact symbol name, pattern, or terminology used in the codebase.

**Example queries that cannot be answered today:**
- "code that handles payment retries"
- "where do we validate user permissions"
- "functions similar to createUser"
- "error recovery logic"

VectorCode fills this gap by vectorizing code chunks and enabling cosine-similarity search over natural language queries.

---

## 3. Architecture Overview

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
│       │                 │                                   │
│       │  ┌───────────┐  │                                   │
│       │  │ Chunker   │  │  Tree-sitter AST parsing          │
│       │  └─────┬─────┘  │                                   │
│       │        │        │                                   │
│       │  ┌─────▼─────┐  │                                   │
│       │  │ Embedder  │  │  Provider trait (ONNX/Gemini/     │
│       │  │ (trait)   │  │  Ollama/OpenAI)                   │
│       │  └─────┬─────┘  │                                   │
│       │        │        │                                   │
│       │  ┌─────▼─────┐  │                                   │
│       │  │ Store     │  │  SQLite + sqlite-vec              │
│       │  │           │  │  (.vectorcode/index.db)           │
│       │  └───────────┘  │                                   │
│       └─────────────────┘                                   │
└─────────────────────────────────────────────────────────────┘
```

---

## 4. Directory Structure (Project Layout)

```
vectorcode/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
├── install.sh                    # macOS/Linux installer
├── install.ps1                   # Windows installer
├── build.rs                      # Build script (ONNX model bundling)
│
├── models/                       # Embedded ONNX models
│   └── minilm-l6-v2-q8/
│       ├── model.onnx            # INT8 quantized (~23MB)
│       ├── tokenizer.json        # HuggingFace tokenizer
│       └── config.json           # Model metadata
│
├── grammars/                     # Tree-sitter grammar .so/.dylib (built at compile time)
│
├── skills/                       # Distributable Skill files
│   └── semantic-search/
│       └── SKILL.md
│
├── src/
│   ├── main.rs                   # Entry point, CLI dispatch
│   ├── cli/
│   │   ├── mod.rs
│   │   ├── init.rs               # `vectorcode init`
│   │   ├── index.rs              # `vectorcode index`
│   │   ├── search.rs             # `vectorcode search`
│   │   ├── status.rs             # `vectorcode status`
│   │   ├── serve.rs              # `vectorcode serve --mcp`
│   │   ├── install.rs            # `vectorcode install`
│   │   └── upgrade.rs            # `vectorcode upgrade`
│   │
│   ├── mcp/
│   │   ├── mod.rs                # MCP server (stdio transport)
│   │   ├── transport.rs          # JSON-RPC stdio reader/writer
│   │   ├── handler.rs            # Tool dispatch
│   │   └── schema.rs             # Tool definitions (JSON Schema)
│   │
│   ├── engine/
│   │   ├── mod.rs
│   │   ├── chunker.rs            # AST-aware chunking logic
│   │   ├── languages.rs          # Tree-sitter language registry
│   │   ├── indexer.rs            # Orchestrates chunk → embed → store
│   │   └── searcher.rs           # Query embedding + similarity search
│   │
│   ├── embedder/
│   │   ├── mod.rs                # Embedder trait definition
│   │   ├── onnx.rs               # ONNX Runtime provider (bundled model)
│   │   ├── gemini.rs             # Google Gemini API provider
│   │   ├── ollama.rs             # Ollama local API provider
│   │   └── openai.rs             # OpenAI API provider
│   │
│   ├── store/
│   │   ├── mod.rs
│   │   ├── db.rs                 # SQLite connection, migrations
│   │   ├── chunks.rs             # Chunk CRUD operations
│   │   └── vectors.rs            # sqlite-vec operations
│   │
│   ├── watcher/
│   │   ├── mod.rs                # File watcher with debounce
│   │   └── gitignore.rs          # .gitignore-aware filtering
│   │
│   └── config/
│       ├── mod.rs                # Configuration loading
│       └── schema.rs             # Config struct definitions
│
└── tests/
    ├── integration/
    │   ├── indexing_test.rs
    │   ├── search_test.rs
    │   └── mcp_test.rs
    └── fixtures/
        ├── sample_ts/            # TypeScript sample project
        ├── sample_py/            # Python sample project
        └── sample_rs/            # Rust sample project
```

---

## 5. Data Models

### 5.1 Chunk

A chunk is the atomic unit of indexed code. Each chunk maps to one semantically meaningful block of source code.

```rust
pub struct Chunk {
    /// Deterministic ID: blake3(file_path + ":" + byte_start + ":" + byte_end)
    pub id: String,

    /// Absolute path to the source file
    pub file_path: String,

    /// Line range in the source file (1-indexed, inclusive)
    pub start_line: u32,
    pub end_line: u32,

    /// Byte offset range in the source file (0-indexed)
    pub byte_start: u32,
    pub byte_end: u32,

    /// Symbol name if available (e.g., "UserService.authenticate")
    pub symbol: Option<String>,

    /// AST node kind (e.g., "function_declaration", "class_declaration", "impl_item")
    pub kind: String,

    /// The source code content of this chunk
    pub content: String,

    /// Parent context for retrieval enrichment
    /// e.g., "class UserService" or "mod auth::handlers"
    pub parent_context: Option<String>,

    /// Language identifier (e.g., "typescript", "python", "rust")
    pub language: String,

    /// File modification time at indexing (Unix timestamp seconds)
    pub file_mtime: i64,

    /// Content hash for change detection: blake3(content)
    pub content_hash: String,
}
```

### 5.2 Index Metadata

```rust
pub struct IndexMeta {
    /// Embedding provider used to create this index
    pub provider: String,          // "onnx" | "gemini" | "ollama" | "openai"

    /// Specific model identifier
    pub model: String,             // e.g., "all-MiniLM-L6-v2", "gemini-embedding-001"

    /// Vector dimensions (FIXED at index creation time)
    pub dimensions: u32,           // e.g., 384, 768, 3072

    /// Timestamp of index creation
    pub created_at: String,        // ISO 8601

    /// Timestamp of last completed sync
    pub last_sync_at: Option<String>,

    /// Total files indexed
    pub files_indexed: u32,

    /// Total chunks stored
    pub chunks_stored: u32,

    /// VectorCode version that created this index
    pub vectorcode_version: String,
}
```

### 5.3 Search Result

```rust
pub struct SearchResult {
    /// Chunk metadata
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol: Option<String>,
    pub kind: String,
    pub language: String,
    pub parent_context: Option<String>,

    /// The source code content
    pub content: String,

    /// Cosine similarity score (0.0 to 1.0, higher = more relevant)
    pub score: f32,
}
```

---

## 6. SQLite Schema

All data lives in a single file: `.vectorcode/index.db`

```sql
-- Index metadata (singleton row)
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Chunk metadata
CREATE TABLE chunks (
    id             TEXT PRIMARY KEY,
    file_path      TEXT NOT NULL,
    start_line     INTEGER NOT NULL,
    end_line       INTEGER NOT NULL,
    byte_start     INTEGER NOT NULL,
    byte_end       INTEGER NOT NULL,
    symbol         TEXT,
    kind           TEXT NOT NULL,
    content        TEXT NOT NULL,
    parent_context TEXT,
    language       TEXT NOT NULL,
    file_mtime     INTEGER NOT NULL,
    content_hash   TEXT NOT NULL
);

CREATE INDEX idx_chunks_file_path ON chunks(file_path);
CREATE INDEX idx_chunks_symbol ON chunks(symbol) WHERE symbol IS NOT NULL;
CREATE INDEX idx_chunks_language ON chunks(language);
CREATE INDEX idx_chunks_content_hash ON chunks(content_hash);

-- Vector storage (sqlite-vec virtual table)
-- Dimensions are set at creation time based on the embedding provider.
-- The placeholder {DIMS} MUST be replaced with the actual integer value
-- during `vectorcode init` (e.g., 384 for ONNX MiniLM, 768 for Gemini).
CREATE VIRTUAL TABLE vec_chunks USING vec0(
    chunk_id TEXT PRIMARY KEY,
    embedding float[{DIMS}]
);

-- File tracking for incremental sync
CREATE TABLE files (
    path       TEXT PRIMARY KEY,
    mtime      INTEGER NOT NULL,
    size       INTEGER NOT NULL,
    hash       TEXT NOT NULL,
    indexed_at INTEGER NOT NULL
);
```

---

## 7. Embedding Provider System

### 7.1 Trait Definition

```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Generate embeddings for a batch of texts
    /// Default implementation calls embed() in sequence;
    /// providers with native batch support should override.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// Number of dimensions in the output vectors
    fn dimensions(&self) -> u32;

    /// Provider name for metadata
    fn provider_name(&self) -> &str;

    /// Model identifier for metadata
    fn model_name(&self) -> &str;

    /// Maximum input token length supported
    fn max_tokens(&self) -> u32;
}
```

### 7.2 Provider Specifications

#### ONNX (default, offline)

| Field | Value |
|---|---|
| **Provider name** | `onnx` |
| **Default model** | `all-MiniLM-L6-v2` (INT8 quantized) |
| **Dimensions** | 384 |
| **Max tokens** | 512 |
| **Dependencies** | `ort` crate (ONNX Runtime bindings), `tokenizers` crate (HuggingFace) |
| **Model delivery** | Bundled in binary via `include_bytes!` or downloaded on first `init` |
| **Batch support** | Native (multiple inputs per session run) |
| **Requires internet** | No |
| **Requires API key** | No |

**Implementation notes:**
- Use `tokenizers` crate from HuggingFace for WordPiece tokenization
- Load model via `ort::Session::builder().with_model_from_memory()`
- Input tensors: `input_ids`, `attention_mask`, `token_type_ids` (all i64)
- Output: take `last_hidden_state`, apply mean pooling over token dimension, then L2 normalize
- For chunks longer than 512 tokens: truncate (the chunk should already be sized appropriately by the chunker)

#### Gemini

| Field | Value |
|---|---|
| **Provider name** | `gemini` |
| **Default model** | `gemini-embedding-001` |
| **Dimensions** | 768 (configurable: 256, 512, 768, 1024, 3072 via Matryoshka) |
| **Max tokens** | 2048 |
| **API endpoint** | `https://generativelanguage.googleapis.com/v1beta/models/{model}:embedContent` |
| **Auth** | API key via `GEMINI_API_KEY` env var or config |
| **Batch support** | Yes — `batchEmbedContents` endpoint, up to 100 items per request |
| **Rate limits** | Free tier: 1500 req/min |
| **Requires internet** | Yes |

**Request format:**
```json
POST /v1beta/models/gemini-embedding-001:embedContent
{
  "content": {
    "parts": [{ "text": "function handlePayment()..." }]
  },
  "outputDimensionality": 768
}
```

**Batch request format:**
```json
POST /v1beta/models/gemini-embedding-001:batchEmbedContents
{
  "requests": [
    {
      "content": { "parts": [{ "text": "chunk 1..." }] },
      "outputDimensionality": 768
    },
    {
      "content": { "parts": [{ "text": "chunk 2..." }] },
      "outputDimensionality": 768
    }
  ]
}
```

**Implementation notes:**
- Use `reqwest` for HTTP
- Implement exponential backoff with jitter for rate limiting (429 responses)
- Batch size: 100 items per request (API max)
- The `outputDimensionality` parameter controls Matryoshka truncation

#### Ollama

| Field | Value |
|---|---|
| **Provider name** | `ollama` |
| **Default model** | `embeddinggemma:latest` |
| **Dimensions** | 768 |
| **Max tokens** | 8192 |
| **API endpoint** | `http://localhost:11434/api/embed` (configurable) |
| **Auth** | None |
| **Batch support** | Yes — `input` field accepts array of strings |
| **Requires internet** | No (after model pull) |

**Request format:**
```json
POST /api/embed
{
  "model": "embeddinggemma:latest",
  "input": ["chunk 1...", "chunk 2..."]
}
```

**Response:**
```json
{
  "model": "embeddinggemma:latest",
  "embeddings": [[0.123, -0.456, ...], [0.789, -0.012, ...]]
}
```

**Implementation notes:**
- Verify Ollama is running and model is available before indexing starts
- If Ollama is not reachable, emit a clear error with instructions: `ollama pull embeddinggemma:latest`

#### OpenAI

| Field | Value |
|---|---|
| **Provider name** | `openai` |
| **Default model** | `text-embedding-3-small` |
| **Dimensions** | 1536 |
| **Max tokens** | 8191 |
| **API endpoint** | `https://api.openai.com/v1/embeddings` |
| **Auth** | API key via `OPENAI_API_KEY` env var or config |
| **Batch support** | Yes — `input` field accepts array, up to 2048 items |
| **Requires internet** | Yes |

**Request format:**
```json
POST /v1/embeddings
{
  "model": "text-embedding-3-small",
  "input": ["chunk 1...", "chunk 2..."]
}
```

**Implementation notes:**
- Standard OpenAI SDK pattern
- Implement retry with exponential backoff for 429/500/503

---

## 8. AST-Aware Chunking System

### 8.1 Supported Languages

Tree-sitter grammars to include (start with these, expand later):

| Language | Tree-sitter crate | Priority | File extensions |
|---|---|---|---|
| TypeScript/TSX | `tree-sitter-typescript` | P0 | `.ts`, `.tsx` |
| JavaScript/JSX | `tree-sitter-javascript` | P0 | `.js`, `.jsx`, `.mjs`, `.cjs` |
| Python | `tree-sitter-python` | P0 | `.py` |
| Rust | `tree-sitter-rust` | P0 | `.rs` |
| Go | `tree-sitter-go` | P1 | `.go` |
| Java | `tree-sitter-java` | P1 | `.java` |
| C# | `tree-sitter-c-sharp` | P1 | `.cs` |
| C/C++ | `tree-sitter-c`, `tree-sitter-cpp` | P1 | `.c`, `.h`, `.cpp`, `.hpp`, `.cc` |
| Ruby | `tree-sitter-ruby` | P2 | `.rb` |
| Swift | `tree-sitter-swift` | P2 | `.swift` |
| Kotlin | `tree-sitter-kotlin` | P2 | `.kt`, `.kts` |

Files with unrecognized extensions fall back to a **line-based chunker** (sliding window, 50 lines per chunk, 10 lines overlap).

### 8.2 Chunking Strategy

#### Target AST Node Types (per language)

Each language defines which AST node types constitute a "chunkable" unit:

**TypeScript/JavaScript:**
- `function_declaration`
- `arrow_function` (only when assigned to a variable/export)
- `method_definition`
- `class_declaration`
- `interface_declaration`
- `type_alias_declaration`
- `enum_declaration`
- `export_statement` (wrapping any of the above)

**Python:**
- `function_definition`
- `class_definition`
- `decorated_definition`

**Rust:**
- `function_item`
- `impl_item`
- `struct_item`
- `enum_item`
- `trait_item`
- `mod_item` (only top-level, not inline)

**Go:**
- `function_declaration`
- `method_declaration`
- `type_declaration`

**(Other languages follow the same pattern: extract top-level declarations and methods.)**

#### Chunking Algorithm

```
FUNCTION chunk_file(source: &str, language: Language) -> Vec<Chunk>:
    tree = tree_sitter_parse(source, language)
    chunks = []

    FOR each top-level node in tree.root_node().children():
        IF node.kind() is in CHUNKABLE_TYPES[language]:
            text = source[node.byte_range()]
            size = text.len()

            IF size < 100:
                // Too small to be useful alone — skip or merge with neighbors
                CONTINUE

            ELSE IF size <= 2000:
                // Ideal size — emit as single chunk
                chunks.push(make_chunk(node, text))

            ELSE:
                // Too large — recursively split by children
                sub_chunks = split_large_node(node, source, language)
                chunks.extend(sub_chunks)

    IF chunks.is_empty():
        // Fallback: line-based sliding window
        chunks = line_based_chunks(source, window=50, overlap=10)

    RETURN chunks


FUNCTION split_large_node(node, source, language) -> Vec<Chunk>:
    children = node.named_children()
        .filter(|c| c.kind() is in CHUNKABLE_TYPES[language])

    IF children.is_empty():
        // No meaningful children — split by statements with overlap
        RETURN statement_split(node, source, max_size=1500, overlap=100)

    chunks = []
    FOR child in children:
        text = source[child.byte_range()]
        IF text.len() <= 2000:
            // Prepend parent signature as context
            chunk = make_chunk(child, text)
            chunk.parent_context = extract_signature(node, source)
            chunks.push(chunk)
        ELSE:
            chunks.extend(split_large_node(child, source, language))

    RETURN chunks
```

#### Chunk Metadata Extraction

For each chunk, extract:
- **`symbol`**: full qualified name when possible (e.g., `ClassName.methodName`)
- **`kind`**: the AST node type string
- **`parent_context`**: the signature of the enclosing scope (e.g., `impl UserService` or `class PaymentHandler`)
- **`start_line`** / **`end_line`**: from `node.start_position().row + 1` and `node.end_position().row + 1`

#### Content Enrichment Before Embedding

Before sending a chunk to the embedder, prepend contextual metadata to improve retrieval quality:

```
// Format sent to the embedder (NOT stored in `content` field)
"{language} | {file_path} | {parent_context} | {symbol}\n{content}"
```

Example:
```
"typescript | src/payment/retry.ts | class PaymentRetryHandler | handleRetry\nasync handleRetry(attempt: number): Promise<PaymentResult> {\n  ..."
```

This enrichment helps the embedding model understand that `handleRetry` is a TypeScript method inside a payment retry handler, even if the code alone doesn't mention "payment" explicitly.

---

## 9. Indexing Pipeline

### 9.1 Full Index (`vectorcode index`)

```
1. Load config (provider, dimensions, languages)
2. Validate index exists (.vectorcode/index.db) or fail with "run vectorcode init first"
3. Discover files:
   a. Walk project directory recursively
   b. Filter by supported extensions (§8.1)
   c. Respect .gitignore via `ignore` crate (same library ripgrep uses)
   d. Skip .vectorcode/, .git/, node_modules/, target/, __pycache__/, vendor/
   e. Skip files > 1MB (configurable via max_file_size)
4. For each file:
   a. Check files table: if mtime + size unchanged AND hash matches → skip
   b. Read file content
   c. Compute content hash (blake3)
   d. Parse with tree-sitter → extract chunks
   e. For each chunk:
      - Compute chunk ID (blake3 of file_path + byte_range)
      - Check if chunk with same ID + content_hash exists → skip
   f. Collect all new/changed chunks
5. Batch embed all new chunks:
   a. Group into batches (size depends on provider: 100 for Gemini, 2048 for OpenAI, etc.)
   b. Call embedder.embed_batch() for each batch
   c. Store vectors in vec_chunks table
   d. Store metadata in chunks table
   e. Update files table with new mtime/size/hash
6. Clean stale data:
   a. Remove chunks for files that no longer exist
   b. Remove chunks for file regions that changed
   c. Remove corresponding vectors
7. Update meta table with last_sync_at, files_indexed, chunks_stored
8. Report: "Indexed {N} files, {M} chunks, {T} new embeddings in {D}s"
```

### 9.2 Incremental Sync (`vectorcode sync`)

Same as full index but:
- Only processes files where `mtime` or `size` differ from `files` table
- Used by the file watcher after debounce

### 9.3 Progress Reporting

During indexing, emit progress to stderr (not stdout, which is reserved for MCP):

```
[1/3] Discovering files... 2,515 files found
[2/3] Chunking... 8,432 chunks (2,108 new, 6,324 unchanged)
[3/3] Embedding... 2,108 chunks [████████████████████] 100% (42.3s)
Indexed 2,515 files, 8,432 chunks in 45.1s
```

### 9.4 Concurrency

- File I/O: use `tokio` async runtime with bounded concurrency (default: 8 files in parallel)
- Embedding API calls: bounded concurrency matching provider rate limits
  - ONNX: single session, batch internally (CPU-bound)
  - Gemini: 4 concurrent requests (to stay within 1500 req/min)
  - Ollama: 1 concurrent request (local, sequential)
  - OpenAI: 4 concurrent requests
- SQLite writes: single writer, use WAL mode for concurrent reads during MCP serving

---

## 10. Query Pipeline

### 10.1 Search Flow

```
1. Receive query string from MCP tool call
2. Enrich query (optional): if query is very short (<3 words), prepend "code that"
3. Embed query using same provider/model as index
4. Execute vector similarity search:
   SELECT c.*, v.distance
   FROM vec_chunks v
   JOIN chunks c ON c.id = v.chunk_id
   WHERE v.embedding MATCH ?query_vec
     AND k = ?limit
   ORDER BY v.distance ASC
5. Convert distance to score: score = 1.0 - distance (for cosine distance)
6. Filter results with score < threshold (default: 0.3)
7. Format and return results
```

### 10.2 Search Options

| Parameter | Type | Default | Description |
|---|---|---|---|
| `query` | string | required | Natural language search query |
| `limit` | integer | 10 | Maximum number of results |
| `threshold` | float | 0.3 | Minimum similarity score (0.0–1.0) |
| `language` | string? | null | Filter by language (e.g., "typescript") |
| `path` | string? | null | Filter by file path prefix (e.g., "src/auth/") |
| `kind` | string? | null | Filter by chunk kind (e.g., "function_declaration") |

---

## 11. MCP Server Specification

### 11.1 Transport

- **Protocol**: MCP over stdio (stdin/stdout)
- **Format**: JSON-RPC 2.0
- **Launch**: `vectorcode serve --mcp`
- The server MUST NOT write anything to stdout except valid JSON-RPC messages
- Diagnostic/log output goes to stderr

### 11.2 Server Capabilities

```json
{
  "name": "vectorcode",
  "version": "0.1.0",
  "capabilities": {
    "tools": {}
  }
}
```

### 11.3 Tool Definitions

#### `vec_search`

Primary tool. Semantic search over the indexed codebase.

```json
{
  "name": "vec_search",
  "description": "Semantic code search — find code by meaning, not just by name. Use when you need to find code related to a concept (e.g., 'payment retry logic', 'user authentication', 'error handling for database connections') and you don't know the exact symbol names or file locations. Returns ranked code chunks with file paths, line numbers, and similarity scores. Complements grep (exact match) and codegraph (structural). Use grep when you know the exact string; use codegraph when you know the symbol name; use vec_search when you know the concept but not the code.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Natural language description of the code you're looking for. Be specific about the domain and behavior (e.g., 'retry logic with exponential backoff' is better than 'retry')."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of results to return.",
        "default": 10
      },
      "threshold": {
        "type": "number",
        "description": "Minimum similarity score (0.0–1.0). Lower values return more results with less relevance.",
        "default": 0.3
      },
      "language": {
        "type": "string",
        "description": "Filter results by programming language (e.g., 'typescript', 'python', 'rust')."
      },
      "path": {
        "type": "string",
        "description": "Filter results by file path prefix (e.g., 'src/auth/' to search only in the auth module)."
      }
    },
    "required": ["query"]
  }
}
```

**Response format** (returned as text content in MCP tool result):

```
Found 5 results for "payment retry logic" (threshold: 0.30)

[1] src/payment/retry.ts:45-92 (score: 0.87)
    Symbol: PaymentRetryHandler.handleRetry
    Kind: method_definition

    async handleRetry(attempt: number): Promise<PaymentResult> {
      const delay = Math.min(1000 * Math.pow(2, attempt), 30000);
      const jitter = Math.random() * 1000;
      await sleep(delay + jitter);
      ...
    }

[2] src/payment/processor.ts:120-145 (score: 0.72)
    Symbol: processPaymentWithRetry
    Kind: function_declaration
    ...

(3 more results)
```

#### `vec_status`

Report index health and statistics.

```json
{
  "name": "vec_status",
  "description": "Check the status of the VectorCode index — provider, model, dimensions, number of indexed files and chunks, last sync time, and any pending file changes.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "projectPath": {
        "type": "string",
        "description": "Path to a project with .vectorcode/ initialized. Defaults to current directory."
      }
    }
  }
}
```

**Response format:**

```
VectorCode Index Status
═══════════════════════
Provider:    gemini
Model:       gemini-embedding-001
Dimensions:  768
Version:     0.1.0

Files:       2,515 indexed
Chunks:      8,432 stored
Last sync:   2026-06-10T20:00:00Z (3 minutes ago)

Pending sync:
  src/payment/retry.ts (modified 5s ago)
  src/auth/handler.ts (modified 12s ago)
```

#### `vec_reindex`

Force a full or partial re-index.

```json
{
  "name": "vec_reindex",
  "description": "Force re-indexing of the codebase or specific files. Use after changing the embedding provider, or when the index seems stale or corrupted.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "path": {
        "type": "string",
        "description": "Specific file or directory to reindex. If omitted, reindexes the entire project."
      },
      "full": {
        "type": "boolean",
        "description": "If true, drops all existing data and rebuilds from scratch. If false, only reindexes changed files.",
        "default": false
      }
    }
  }
}
```

---

## 12. CLI Commands

### 12.1 Command Reference

```
vectorcode <COMMAND>

Commands:
  init       Initialize VectorCode in a project directory
  index      Build or update the embedding index
  search     Search the index from the command line
  status     Show index status and health
  serve      Start the MCP server
  install    Auto-configure agents (OpenCode, Claude Code, Cursor, etc.)
  uninstall  Remove VectorCode from agent configurations
  upgrade    Self-update the binary
  help       Print help

Global options:
  --project-path <PATH>   Path to project (default: current directory)
  --verbose               Enable verbose logging to stderr
  --quiet                 Suppress progress output
```

### 12.2 `vectorcode init`

```
vectorcode init [OPTIONS]

Options:
  --provider <PROVIDER>   Embedding provider [default: onnx]
                          [possible values: onnx, gemini, ollama, openai]
  --model <MODEL>         Model name (provider-specific default if omitted)
  --dims <DIMS>           Embedding dimensions (provider-specific default if omitted)
  --index                 Also run initial indexing after init (like codegraph init -i)
```

**Behavior:**
1. Create `.vectorcode/` directory
2. Create `index.db` with schema (§6)
3. Write `meta` table with provider, model, dimensions
4. Create `.vectorcode/.gitignore` containing `index.db` (the DB should not be committed)
5. Create `.vectorcode/config.toml` with chosen provider settings
6. If `--index` flag: run full indexing pipeline

### 12.3 `vectorcode index`

```
vectorcode index [OPTIONS]

Options:
  --full            Drop all data and rebuild from scratch
  --file <PATH>     Index only a specific file
  --concurrency <N> Max concurrent file processing [default: 8]
```

### 12.4 `vectorcode search`

```
vectorcode search <QUERY> [OPTIONS]

Options:
  --limit <N>          Max results [default: 10]
  --threshold <F>      Min similarity score [default: 0.3]
  --language <LANG>    Filter by language
  --path <PREFIX>      Filter by path prefix
  --json               Output results as JSON
```

### 12.5 `vectorcode serve`

```
vectorcode serve [OPTIONS]

Options:
  --mcp              Start as MCP server (stdio transport)
  --watch            Enable file watcher for auto-sync [default: true]
  --debounce <MS>    File watcher debounce interval [default: 2000]
```

### 12.6 `vectorcode install`

```
vectorcode install [OPTIONS]

Options:
  --target <AGENT>   Install for specific agent only
                     [possible values: opencode, claude-code, cursor, gemini-cli, antigravity]
```

**Behavior:**
- Detect installed agents by checking known config file locations
- For each detected agent, add the VectorCode MCP server entry to its config
- Patterns per agent:
  - **OpenCode**: modify `opencode.json` → `mcpServers` section
  - **Claude Code**: modify `~/.claude/claude_desktop_config.json` → `mcpServers`
  - **Cursor**: modify `.cursor/mcp.json`
  - **Gemini CLI**: modify `~/.gemini/settings.json` → `mcpServers`
  - **Antigravity**: modify `~/.gemini/antigravity/settings.json` → `mcpServers`

---

## 13. Configuration

### 13.1 Config File Location

`.vectorcode/config.toml` (per-project)

### 13.2 Config Schema

```toml
# .vectorcode/config.toml

[provider]
# Which embedding provider to use
# Values: "onnx", "gemini", "ollama", "openai"
name = "onnx"

[provider.onnx]
# Model bundled with the binary — no config needed
# model = "all-MiniLM-L6-v2"  (default, currently the only bundled option)

[provider.gemini]
# API key: reads from this field OR from GEMINI_API_KEY env var
api_key = ""
model = "gemini-embedding-001"
dimensions = 768  # Matryoshka: 256, 512, 768, 1024, 3072

[provider.ollama]
url = "http://localhost:11434"
model = "embeddinggemma:latest"

[provider.openai]
# API key: reads from this field OR from OPENAI_API_KEY env var
api_key = ""
model = "text-embedding-3-small"

[indexing]
# Maximum file size to index (bytes). Files larger than this are skipped.
max_file_size = 1_048_576  # 1MB

# Directories to always exclude (in addition to .gitignore)
exclude_dirs = [
    ".vectorcode",
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    "vendor",
    "dist",
    "build",
    ".next",
]

# File extensions to always exclude
exclude_extensions = [
    ".min.js",
    ".map",
    ".lock",
    ".svg",
    ".png",
    ".jpg",
    ".ico",
    ".woff",
    ".woff2",
    ".ttf",
]

# Max concurrent file processing
concurrency = 8

[watcher]
# File watcher debounce in milliseconds
debounce_ms = 2000

# Disable file watcher entirely
disabled = false

[search]
# Default result limit
default_limit = 10

# Default similarity threshold
default_threshold = 0.3
```

### 13.3 Environment Variable Overrides

| Env var | Overrides |
|---|---|
| `GEMINI_API_KEY` | `provider.gemini.api_key` |
| `OPENAI_API_KEY` | `provider.openai.api_key` |
| `VECTORCODE_PROVIDER` | `provider.name` |
| `VECTORCODE_NO_WATCH` | `watcher.disabled` (set to `1` to disable) |
| `VECTORCODE_DEBOUNCE_MS` | `watcher.debounce_ms` |

---

## 14. File Watcher

### 14.1 Behavior

When the MCP server runs (`vectorcode serve --mcp`), it starts a file watcher on the project directory.

**Watcher flow:**
1. Use `notify` crate with native OS events (FSEvents on macOS, inotify on Linux, ReadDirectoryChanges on Windows)
2. Filter events through `.gitignore` rules (using `ignore` crate)
3. Filter by supported file extensions
4. Debounce: collect all changed file paths over a configurable window (default 2000ms)
5. After debounce: run incremental sync on changed files only
6. Track pending files (changed but not yet re-indexed) for staleness reporting

### 14.2 Staleness Banner

When a `vec_search` result references a file that has pending changes (modified after last sync but before debounce completes), prepend a banner:

```
⚠️ Some files referenced below were modified since the last index sync
and may not reflect the latest content:
  - src/payment/retry.ts (modified 1s ago)
Use grep or read these files directly for accurate content.

Found 5 results for "payment retry logic" (threshold: 0.30)
...
```

### 14.3 Connect-Time Catch-Up

When the MCP server starts, before answering the first query:
1. Run a fast `(mtime, size)` reconciliation against the `files` table
2. If any files changed since last sync: run incremental sync
3. This catches changes made while no MCP server was running (git pull, editor, etc.)

---

## 15. Skill File

### 15.1 Location

Distributed with the binary and installable via `vectorcode install`:

- **Per-project**: `.agents/skills/semantic-search/SKILL.md`
- **Global**: `~/.agents/skills/semantic-search/SKILL.md`

### 15.2 Content

```markdown
---
name: semantic-search
description: >
  Use when searching for code by concept, meaning, or behavior — not by exact
  symbol name or literal string. Ideal for queries like "payment retry logic",
  "user authentication flow", "error handling for database connections", or
  "functions similar to createUser". Do NOT use for exact string matches (use
  grep) or known symbol lookups (use codegraph_explore).
---

## Semantic Code Search Protocol

### Tool: `vec_search`

Performs cosine-similarity search over embedded code chunks. Returns ranked
results with file paths, line numbers, symbols, and source code.

### When to use `vec_search`

- You need to find code related to a **concept** but don't know the symbol names
- `grep` returned no results because the code uses different terminology
- You want to find **similar** code patterns across the codebase
- You're exploring an unfamiliar area of the codebase by topic

### When NOT to use `vec_search`

- You know the exact function/class name → use `codegraph_explore`
- You know an exact string in the code → use `grep`
- You're looking for past decisions or history → use `mem_search` (Engram)

### Recommended flow: Semantic → Structural → Historical

For comprehensive code discovery, combine all three tools:

1. **`vec_search("payment error handling")`**
   → Finds code chunks semantically related to payment errors
   → Returns file paths, line ranges, and ranked source snippets

2. **`codegraph_explore("PaymentError handlePaymentFailure")`**
   → Takes symbol names found in step 1
   → Returns full source code + call graph + blast radius

3. **`mem_search("payment error handling")`**
   → Checks Engram for prior team decisions about this topic
   → Returns architectural context and history

### Query tips

- Be specific: "retry with exponential backoff" > "retry"
- Include domain terms: "payment validation" > "validation"
- Describe behavior: "function that sends email notifications" > "email"
- Use `--language` filter when you know the target language
- Use `--path` filter to scope to a specific module

### Example

```
vec_search("middleware that validates JWT tokens and extracts user info")
```
```

---

## 16. MCP `instructions.md`

This file is placed alongside the MCP tool schemas and is automatically loaded by agents.

### 16.1 Location

`~/.gemini/antigravity/mcp/vectorcode/instructions.md` (written by `vectorcode install`)

### 16.2 Content

```markdown
# VectorCode — semantic code search over embedded vectors

VectorCode indexes the codebase into vector embeddings and enables
semantic similarity search. It finds code by meaning, not by name.

## Tool selection

- **"Find code about X concept / behavior / domain"** → `vec_search`
- **"Check if index is healthy / current"** → `vec_status`
- **"Force re-index after major changes"** → `vec_reindex`

## When to use vec_search vs other tools

- **Know the exact string** → grep (exact match, faster)
- **Know the symbol name** → codegraph_explore (structural, precise)
- **Know the concept but not the name** → vec_search (semantic, fuzzy)
- **Looking for past decisions** → mem_search / Engram (memory)

## Anti-patterns

- Don't use vec_search to find a symbol you already know the name of —
  codegraph_explore is faster and returns structural context.
- Don't re-verify vec_search results with grep — the source code in the
  result IS the current indexed content. Check the staleness banner if present.
- Don't ignore the score — results below 0.4 are usually noise.

## Staleness

The file watcher keeps the index current (2-second debounce after edits).
If a result has a ⚠️ staleness banner, read those specific files directly.
All files NOT in the banner are fresh.
```

---

## 17. Rust Crate Dependencies

```toml
[dependencies]
# CLI
clap = { version = "4", features = ["derive"] }

# Async runtime
tokio = { version = "1", features = ["full"] }

# SQLite
rusqlite = { version = "0.32", features = ["bundled", "vtab"] }

# sqlite-vec (loaded as extension)
# Build sqlite-vec from source via build.rs or load as shared library

# Tree-sitter
tree-sitter = "0.24"
tree-sitter-typescript = "0.23"
tree-sitter-javascript = "0.23"
tree-sitter-python = "0.23"
tree-sitter-rust = "0.23"
tree-sitter-go = "0.23"
tree-sitter-java = "0.23"
# Add more languages as needed

# ONNX Runtime
ort = { version = "2", features = ["load-dynamic"] }

# Tokenizer (for ONNX provider)
tokenizers = { version = "0.20", features = ["http"] }

# HTTP client (for API providers)
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Config
toml = "0.8"

# File watching
notify = "7"
notify-debouncer-full = "0.4"

# .gitignore support
ignore = "0.4"

# Hashing
blake3 = "1"

# Error handling
anyhow = "1"
thiserror = "2"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Async trait
async-trait = "0.1"

[dev-dependencies]
tempfile = "3"
assert_cmd = "2"
predicates = "3"
```

> [!IMPORTANT]
> Pin exact versions of tree-sitter grammar crates to avoid breaking changes.
> The `ort` crate version must match the ONNX Runtime version shipped with the binary.
> `rusqlite` must use `features = ["bundled"]` to avoid system SQLite version conflicts.

---

## 18. Error Handling

### 18.1 Error Categories

```rust
#[derive(thiserror::Error, Debug)]
pub enum VectorCodeError {
    #[error("Index not initialized. Run `vectorcode init` first.")]
    NotInitialized,

    #[error("Index was created with provider '{expected}' ({expected_dims}d) but current config uses '{actual}' ({actual_dims}d). Run `vectorcode index --full` to rebuild.")]
    ProviderMismatch {
        expected: String,
        expected_dims: u32,
        actual: String,
        actual_dims: u32,
    },

    #[error("Embedding provider error: {message}")]
    EmbedderError { message: String },

    #[error("API rate limited. Retrying in {retry_after_secs}s...")]
    RateLimited { retry_after_secs: u64 },

    #[error("Ollama not reachable at {url}. Is it running? Try: ollama serve")]
    OllamaUnavailable { url: String },

    #[error("Model '{model}' not found in Ollama. Try: ollama pull {model}")]
    OllamaModelNotFound { model: String },

    #[error("API key not set. Set {env_var} or configure in .vectorcode/config.toml")]
    ApiKeyMissing { env_var: String },

    #[error("Tree-sitter parse error for {file_path}: {message}")]
    ParseError { file_path: String, message: String },

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

### 18.2 MCP Error Responses

MCP tool errors MUST be returned as JSON-RPC error objects, never as panics or crashes:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32000,
    "message": "Index not initialized. Run `vectorcode init` first."
  }
}
```

---

## 19. Testing Strategy

### 19.1 Unit Tests

| Module | What to test |
|---|---|
| `chunker` | Correct AST node extraction per language; size-based splitting; fallback to line-based |
| `embedder::onnx` | Model loading, tokenization, embedding output dimensions, L2 normalization |
| `store` | CRUD operations, vector insertion/search, incremental updates, stale cleanup |
| `config` | TOML parsing, env var overrides, default values |
| `watcher::gitignore` | Pattern matching against .gitignore rules |

### 19.2 Integration Tests

| Test | Description |
|---|---|
| `full_index_cycle` | Init → index a fixture project → verify chunk count and metadata |
| `incremental_sync` | Index → modify a file → sync → verify only changed chunks updated |
| `search_relevance` | Index a fixture project → run known queries → verify expected files appear in top-3 |
| `provider_switch` | Init with ONNX → attempt search → switch to Gemini → verify error on dimension mismatch |
| `mcp_protocol` | Spawn MCP server → send JSON-RPC requests → verify correct responses |
| `large_file_handling` | Index a file > 1MB → verify it's skipped with max_file_size config |
| `gitignore_respect` | Create project with .gitignore → index → verify ignored files excluded |

### 19.3 Fixture Projects

Provide small, self-contained projects in `tests/fixtures/`:

- **`sample_ts/`**: TypeScript project (~20 files) with classes, functions, interfaces
- **`sample_py/`**: Python project (~15 files) with classes, decorators, modules
- **`sample_rs/`**: Rust project (~10 files) with structs, impls, traits, mods

Each fixture should have a `queries.json` with expected search results:

```json
[
  {
    "query": "user authentication with password hashing",
    "expected_files": ["src/auth/password.ts", "src/auth/service.ts"],
    "min_score": 0.5
  }
]
```

---

## 20. Distribution & Installation

### 20.1 Binary Distribution

- Build with `cargo build --release` for each target triple
- Strip symbols: `strip target/release/vectorcode`
- Targets:
  - `x86_64-apple-darwin`
  - `aarch64-apple-darwin`
  - `x86_64-unknown-linux-gnu`
  - `aarch64-unknown-linux-gnu`
  - `x86_64-pc-windows-msvc`

### 20.2 Install Script

**macOS/Linux (`install.sh`):**
```bash
#!/bin/sh
set -e
REPO="alejandro-technology/vectorcode"
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
# Map arch names
case "$ARCH" in
  x86_64) ARCH="x86_64" ;;
  arm64|aarch64) ARCH="aarch64" ;;
esac
URL="https://github.com/$REPO/releases/latest/download/vectorcode-$OS-$ARCH.tar.gz"
curl -fsSL "$URL" | tar xz -C /usr/local/bin vectorcode
echo "vectorcode installed to /usr/local/bin/vectorcode"
```

### 20.3 Homebrew (future)

```ruby
class Vectorcode < Formula
  desc "Semantic code search MCP server using embeddings"
  homepage "https://github.com/alejandro-technology/vectorcode"
  # ...
end
```

---

## 21. Non-Functional Requirements

| Requirement | Target |
|---|---|
| **Cold search latency** | < 200ms for 10K chunks (query embed + vector search + result formatting) |
| **Index throughput (ONNX)** | > 300 chunks/sec on Apple M-series |
| **Index throughput (API)** | Limited by provider rate limits, not by VectorCode |
| **Memory usage (serving)** | < 100MB RSS for 50K chunk index |
| **Disk usage** | ~2KB per chunk (metadata + vector at 768d) → ~100MB for 50K chunks |
| **Binary size** | < 50MB (including bundled ONNX model) |
| **Startup time** | < 500ms to first MCP response (excluding catch-up sync) |
| **Crash recovery** | WAL mode SQLite — no corruption on unexpected termination |

---

## 22. Future Considerations (Out of Scope for v0.1)

These are NOT part of the initial build but should be considered in the architecture:

- **Hybrid search**: combine vector similarity with FTS5 keyword search for better precision
- **Cross-project search**: query multiple `.vectorcode/` indices in one call
- **Code-to-code search**: "find code similar to this snippet" (embed the snippet, not a query)
- **Custom model support**: allow users to bring their own ONNX model
- **GPU acceleration**: CUDA/Metal execution providers for ONNX Runtime
- **Index compression**: quantize stored vectors from float32 to int8 for 4x storage reduction
- **Shared index server**: HTTP transport for multi-user/CI environments
- **Engram integration**: automatically save search patterns and findings as Engram memories
- **CodeGraph integration**: enrich chunks with call graph metadata before embedding
