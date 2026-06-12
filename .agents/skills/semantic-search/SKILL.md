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

VectorCode exposes semantic search through two equivalent surfaces:

- **MCP tool**: `vec_search` (used by the agent/LLM client)
- **CLI**: `vectorcode search "<query>"` (used from a terminal or scripts)

Both call the same underlying engine: cosine-similarity search over embedded
code chunks, returning ranked results with file paths, line numbers, symbols,
and source code.

---

## MCP Tool: `vec_search`

### When to use `vec_search`

- You need to find code related to a **concept** but don't know the symbol names
- `grep` returned no results because the code uses different terminology
- You want to find **similar** code patterns across the codebase
- You're exploring an unfamiliar area of the codebase by topic

### When NOT to use `vec_search`

- You know the exact function/class name → use `codegraph_explore`
- You know an exact string in the code → use `grep`
- You're looking for past decisions or history → use `mem_search` (Engram)

### MCP tool parameters

| Parameter   | Type     | Description |
|-------------|----------|-------------|
| `query`     | string   | Natural-language description of the concept to find |
| `limit`     | integer  | Max number of results (default 10) |
| `path`      | string   | Restrict search to a file/directory prefix |
| `language`  | string   | Filter by programming language (e.g. `rust`, `typescript`) |
| `threshold` | float    | Minimum similarity score 0.0–1.0 (default 0.3) |

---

## CLI: `vectorcode search`

The CLI mirrors the MCP tool and is useful for shell loops, CI checks, and
manual exploration without an MCP client.

### Synopsis

```bash
vectorcode search [OPTIONS] <QUERY>
```

### Flags

| Flag           | Short | Description |
|----------------|-------|-------------|
| `<QUERY>`      |       | Positional argument: the search query (required) |
| `--limit <N>`  | `-n`  | Number of results to return |
| `--path <PATH>`| `-p`  | Scope search to a specific file or directory |
| `--language <LANG>` |   | Filter by programming language |
| `--threshold <FLOAT>` | | Minimum similarity score (0.0–1.0) |

### CLI examples

```bash
# Basic semantic search
vectorcode search "middleware that validates JWT tokens and extracts user info"

# Top 5 matches scoped to the auth module
vectorcode search --limit 5 --path src/auth "session expiration handling"

# Only Rust files, high-confidence matches
vectorcode search --language rust --threshold 0.6 "retry with exponential backoff"
```

### CLI vs MCP — when to use which

- **CLI** → humans, shell scripts, CI, `git grep`-style workflows, piping to
  `rg`/`fzf`, or when the MCP server is not available.
- **MCP (`vec_search`)** → agent/LLM clients that consume tool calls, where the
  model needs structured results to continue reasoning.

---

## Recommended flow: Semantic → Structural → Historical

For comprehensive code discovery, combine all three tools:

1. **`vec_search("payment error handling")`** *(or `vectorcode search ...` in the shell)*
   → Finds code chunks semantically related to payment errors
   → Returns file paths, line ranges, and ranked source snippets

2. **`codegraph_explore("PaymentError handlePaymentFailure")`**
   → Takes symbol names found in step 1
   → Returns full source code + call graph + blast radius

3. **`mem_search("payment error handling")`**
   → Checks Engram for prior team decisions about this topic
   → Returns architectural context and history

---

## Query tips

- Be specific: "retry with exponential backoff" > "retry"
- Include domain terms: "payment validation" > "validation"
- Describe behavior: "function that sends email notifications" > "email"
- Use `--language` / `--path` filters when you know the target language or module
- Lower `--threshold` to broaden recall; raise it to sharpen precision

---

## Example (MCP)

```
vec_search("middleware that validates JWT tokens and extracts user info")
```

## Example (CLI)

```bash
vectorcode search "middleware that validates JWT tokens and extracts user info"
```
