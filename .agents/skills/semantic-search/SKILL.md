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
