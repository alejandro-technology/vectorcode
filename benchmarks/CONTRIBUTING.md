# Contributing to the Benchmark Golden Set

This document describes the process for proposing, validating, and merging new golden-set queries into the benchmark harness.

## Purpose

The golden query set defines the acceptance criteria for code-search quality. Changes to this set directly affect baseline metrics and regression detection, so they require careful review.

## Proposing a New Query

1. **Identify the gap**: What search behavior is not covered by existing queries? Examples:
   - A common code pattern (e.g., "iterator with error handling")
   - A cross-language concept (e.g., "async/await in TypeScript vs Python")
   - A regression case (e.g., "search for function with specific signature")

2. **Draft the query**: Write the query text in natural language or keyword style. Keep it realistic — what would a developer actually type?

3. **Label relevance**: For each file in the corpus that might be returned, assign a grade:
   - **3** (highly relevant): Direct match, core example of the concept
   - **2** (relevant): Related code, partial match
   - **1** (marginally relevant): Tangentially related
   - **0** (irrelevant): Not related (omit these from the file — only list files with grade >= 1)

4. **Validate locally**: Run the benchmark with your proposed query:
   ```bash
   cargo run -- benchmark --corpus mini
   ```
   Verify that the metrics change in an expected direction. If your query has grade-3 files but they rank below grade-1 files, investigate why.

## Review Process

1. **Open a PR** with:
   - The new query added to `benchmarks/queries/<corpus>.toml`
   - A brief rationale in the PR description (why this query matters)
   - Before/after metric changes

2. **Two reviewers** must approve:
   - At least one reviewer should verify the relevance judgments are correct
   - At least one reviewer should verify the query text is realistic

3. **Merge criteria**:
   - All CI checks pass (including the benchmark workflow)
   - No metric regression beyond ±0.01 tolerance (unless intentional)
   - Both reviewers have approved

## Query Quality Guidelines

- **Be specific**: "error handling" is too vague; "custom error type with Display trait" is better
- **Cross-language coverage**: Ensure queries test semantic search across Rust, TypeScript, and Python
- **Avoid ambiguity**: If a query could match multiple unrelated concepts, split it into separate queries
- **Realistic phrasing**: Use natural language that developers would actually type, not artificial test strings

## Updating Relevance Judgments

If you discover that a file's relevance judgment is incorrect:
1. Open a PR with the corrected grade
2. Explain why the original grade was wrong
3. Follow the same two-reviewer approval process

## Metrics Tolerance

When adding queries, expect small metric fluctuations (±0.01). If metrics change by more than this:
- Verify the new query's judgments are correct
- Check if the query exposes a real search quality issue
- Document the expected change in the PR description

## Real-corpus verification (thiserror / defu / itsdangerous / vscode)

The `mini` and `vscode` query sets in this directory are placeholders for
the phase-4.4 real-corpus verification path. The current mock-mini
baselines under `baseline/` gate the deterministic smoke test only — they
are not a published measurement of retrieval quality.

The real-corpus path (with a pinned model + OLLAMA setup) is intentionally
out of scope here. If you need a real baseline, see phase 4.4 in the SDD
roadmap; the comparator added in 4.1 is model-agnostic so 4.4 will only
need new JSON files and a workflow update.

## Example: Adding a Query

```toml
[[queries]]
text = "iterator pattern with error propagation"
[[queries.judgments]]
file = "src/iter.rs"
grade = 3

[[queries.judgments]]
file = "src/error.rs"
grade = 2
```

**Rationale**: Tests whether the search can find iterator implementations that handle errors, a common Rust pattern. The `iter.rs` file is a direct match (grade 3), while `error.rs` is related but not the primary target (grade 2).
