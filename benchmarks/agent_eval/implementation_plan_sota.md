# Implementation Plan: SOTA Agent Evaluation Harness v2

Upgrade the TypeScript Agent Evaluation Harness under `benchmarks/agent_eval/` to a State-Of-The-Art (SOTA) testing suite. This plan incorporates corrections from two independent architecture reviews.

## User Review Required

> [!WARNING]
> Running the full suite in **Live Mode** against OpenCode endpoints consumes ~100K-500K tokens per run (all tasks × all models × both arms). The LLM caching system guarantees subsequent runs are **100% free and deterministic**.

> [!IMPORTANT]
> Execution modes:
> 1. `--cached` (default): Replays deterministic trajectories from `cache/`. Runs in seconds, zero API cost.
> 2. `--live`: Calls real LLMs, writes trajectory to `cache/` on success.
> 3. `--live --update-cache`: Like `--live` but overwrites existing cache.
> 4. `--dry-run`: No LLM calls, no cache. Validate harness plumbing only.

## Decisions

- **Model budget**: Initial cache population uses 3 models:
  - `mimo-v2.5` (OpenAI-compatible SDK)
  - `minimax-m3` (Anthropic-compatible SDK)
  - `deepseek-v4-flash` (OpenAI-compatible SDK)
- **Workspace snapshot**: Dynamic — the workspace git SHA is stored as metadata inside each cache trajectory file, not in filenames. This allows manual override and graceful handling of minor workspace changes without invalidating the entire cache tree.

---

## Architecture Overview

```
benchmarks/agent_eval/
├── src/
│   ├── harness.ts          # Orchestrator: CLI, model matrix, report generation
│   ├── agents.ts           # ReAct loop engine (provider-agnostic)
│   ├── cache.ts            # [NEW] LLM response cache layer
│   ├── judge.ts            # [NEW] LLM-as-Judge verification engine
│   ├── tools/              # [NEW] Tool provider implementations
│   │   ├── types.ts        # Shared ToolProvider interface
│   │   ├── vectorcode.ts   # Arm B: VectorCode MCP client
│   │   └── traditional.ts  # Arm A: ripgrep + find + read_file (real subprocess)
│   ├── tasks/              # [NEW] One file per task (replaces tasks.ts)
│   │   ├── index.ts        # Task registry & loader
│   │   ├── task-symbol-lookup.ts
│   │   ├── task-arch-trace.ts
│   │   ├── task-bug-hunt.ts
│   │   ├── task-impl-status.ts
│   │   └── task-refactor-plan.ts
│   └── types.ts            # Shared type definitions
├── cache/                  # [NEW] Cached LLM responses (gitignored)
│   └── <model>/<task_id>/<arm>/trajectory.jsonl
├── rubrics/                # [NEW] Ground truth & scoring rubrics per task
│   ├── task-symbol-lookup.json
│   ├── task-arch-trace.json
│   ├── task-bug-hunt.json
│   ├── task-impl-status.json
│   └── task-refactor-plan.json
├── snapshots/              # [NEW] Workspace state metadata
│   └── manifest.json       # Git SHA + file hashes at cache-population time
└── results/                # Generated reports (gitignored)
```

---

## Proposed Changes

### 1. Tool Provider Abstraction (`src/tools/`)

Both arms implement the same `ToolProvider` interface. The agent loop is completely arm-agnostic.

#### [NEW] `src/tools/types.ts`

```typescript
export interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: Record<string, any>;
}

export interface ToolProvider {
  name: 'vectorcode' | 'traditional';
  initialize(): Promise<void>;
  listTools(): ToolDefinition[];
  callTool(name: string, args: Record<string, any>): Promise<string>;
  shutdown(): Promise<void>;
}
```

#### [NEW] `src/tools/vectorcode.ts` — Arm B

Wraps the existing MCP client. Spawns `vectorcode serve --mcp`, connects via stdio, proxies `listTools` and `callTool`. Exposes all 8 VectorCode MCP tools:
- `vec_search`, `vec_status`, `vec_reindex`, `vec_read_lines`, `vec_outline`, `vec_find_callers`, `vec_find_dependents`, `vec_trace_imports`

#### [NEW] `src/tools/traditional.ts` — Arm A

**Real subprocess wrappers**, not simulations. Each tool call spawns a real process:

| Tool | Implementation | Why real, not simulated |
|---|---|---|
| `grep(query, path, flags?)` | `child_process.execFile('rg', [...])` — real ripgrep | Simulated grep can't replicate regex edge cases, encoding, binary-file skipping |
| `find_files(pattern, path?)` | `child_process.execFile('find', [...])` or `fd` if available | Real filesystem traversal with gitignore awareness |
| `read_file(path, start?, end?)` | `fs.readFile` + line slicing | Trivial, but real I/O |
| `list_dir(path)` | `fs.readdir` with stat | Directory listing with file/dir/size info |

Both providers output **plain text** (no structured JSON) to avoid giving either arm an unfair parsing advantage.

---

### 2. LLM Response Cache (`src/cache.ts`)

#### Cache Key Design

The naive "hash everything" approach breaks on multi-turn loops because step N's hash depends on step N-1's tool output, which changes when project code changes. The correct design:

```typescript
interface CacheEntry {
  stepIndex: number;
  requestHash: string;       // SHA-256 of the serialized messages array
  response: LLMResponse;
  tokens: { input: number; output: number };
  timestamp: string;
}

interface TrajectoryMetadata {
  workspaceSha: string;      // git rev-parse HEAD at recording time
  model: string;
  taskId: string;
  arm: 'vectorcode' | 'traditional';
  recordedAt: string;
  totalSteps: number;
}

// Storage: cache/<model>/<task_id>/<arm>/trajectory.jsonl
// Line 0: TrajectoryMetadata (JSON)
// Lines 1-N: CacheEntry (one per step)
```

**Trajectory-based caching**: Instead of hashing per-request, we store the entire trajectory as a JSONL file with metadata on line 0. On replay, we match step-by-step: if the request at step N matches the cached request hash, return the cached response. If it diverges (because code changed), we stop replay and switch to live mode from that step forward.

**Dynamic workspace SHA**: The git SHA is stored inside each trajectory file's metadata (line 0), not in the filename. This means:
- Cache files don't need to be renamed/reorganized when code changes
- The harness can warn about SHA mismatches without hard-failing
- Manual override is possible by editing the metadata line
- On `--live` runs, the harness writes the current SHA into new trajectories automatically

On `--cached` runs, the harness compares current `git rev-parse HEAD` against the trajectory's `workspaceSha`. If they differ, it logs a warning and attempts replay anyway (divergence is handled gracefully at the step level).

#### CLI Flags

- `--cached` (default): Replay from cache. Fail-fast if no cache exists for the requested model/task/arm.
- `--live`: Call real LLMs, write trajectory to cache.
- `--live --update-cache`: Like `--live` but overwrites existing cache.
- `--dry-run`: No LLM calls, no cache. Validate harness plumbing.

---

### 3. SOTA Tasks — Graded Difficulty (`src/tasks/`)

Five tasks with escalating difficulty. Each task has a corresponding rubric in `rubrics/`.

#### Task 1: Single-Symbol Lookup (Trivial — Baseline)
- **ID**: `task-symbol-lookup`
- **Difficulty**: ★☆☆☆☆
- **Type**: Read-only
- **Prompt**: *"Find the definition of the `VectorCodeError` enum. List all its variants and the file where it's defined."*
- **Why this task**: Both arms should solve this easily. Establishes baseline cost (tokens/steps) for the simplest possible query. If VectorCode can't beat grep here, something is wrong.
- **Rubric**: Exact file (`src/error.rs`), all variant names, no hallucinated variants.

#### Task 2: Architecture Trace (Medium — Cross-Module Discovery)
- **ID**: `task-arch-trace`
- **Difficulty**: ★★★☆☆
- **Type**: Read-only
- **Prompt**: *"Trace what happens when a file is modified in a VectorCode workspace. Start from the file watcher detecting the change, through chunking, embedding, and storing in the database. For each step, name the specific Rust module and the key function."*
- **Why this task**: Requires discovering 4+ modules and understanding their relationship. VectorCode's semantic search should find the pipeline faster than grep-hopping between files.
- **Rubric**: Must identify all 4 pipeline stages (watcher → chunker → embedder → store) with correct module paths and at least one key function per stage. Scored by LLM-as-Judge.

#### Task 3: Bug Hunt (Medium-Hard — Reasoning + Search)
- **ID**: `task-bug-hunt`
- **Difficulty**: ★★★★☆
- **Type**: Read-only
- **Prompt**: *"The `sanitize_fts_query` function strips certain characters from search queries before passing them to SQLite FTS5. Find this function, explain its sanitization logic, and identify whether it handles the case where the ENTIRE query consists of special characters (i.e., would it return an empty string?)."*
- **Why this task**: Requires finding a specific function, reading it carefully, and reasoning about an edge case. Tests whether the agent actually READS the code vs. guessing from names.
- **Rubric**: Correct file, correct sanitization rules listed, correct analysis of the empty-query edge case. LLM-as-Judge with specific rubric criteria.

#### Task 4: Implement CLI Subcommand (Hard — Write + Compile)
- **ID**: `task-impl-status`
- **Difficulty**: ★★★★☆
- **Type**: Write (generates a file)
- **Prompt**: *"Write a complete Rust file `src/cli/status_eval.rs` implementing a `run()` function that: (1) loads the VectorCode config from `.vectorcode/config.toml`, (2) opens the SQLite database at `.vectorcode/index.db`, (3) reads the `meta` table to get provider name and model, (4) counts rows in the `chunks` table, and (5) prints a formatted status summary. Use the project's existing `config` and `store` modules — do NOT reimplement config parsing or database access."*
- **Why this task**: Tests whether the agent can discover existing APIs (config loader, store module) and compose them correctly. `cargo check` is the verification gate.
- **Verification**:
  1. File exists at `src/cli/status_eval.rs`
  2. `cargo check` passes with the file present (compiler verification)
  3. Cleanup: delete the generated file in a `finally` block after verification

#### Task 5: Cross-Module Refactoring Plan (Expert — Multi-File Analysis)
- **ID**: `task-refactor-plan`
- **Difficulty**: ★★★★★
- **Type**: Read-only (analysis)
- **Prompt**: *"The `Embedder` trait in `src/embedder/mod.rs` currently returns `Vec<f32>` from its `embed` method. Propose a plan to change it to return a generic `EmbeddingVector` type that could support both f32 and f16 representations. Identify: (1) every file that implements the Embedder trait, (2) every file that calls `.embed()` or `.embed_batch()`, (3) the blast radius — which other modules would need to change and why."*
- **Why this task**: This is where VectorCode's `vec_find_callers` and `vec_find_dependents` should massively outperform grep. Grep will find string matches but miss indirect callers through trait objects.
- **Rubric**: Must identify all implementors, all call sites (direct + through trait objects), and correctly assess blast radius. LLM-as-Judge with structured criteria.

---

### 4. LLM-as-Judge Verification (`src/judge.ts`)

For read-only tasks, replace brittle `answer.includes('error.rs')` string matching with a structured judge.

#### How it works

```typescript
interface JudgeResult {
  score: number;           // 0.0 - 1.0
  criteriaScores: Record<string, { score: number; reasoning: string }>;
  overallReasoning: string;
}

async function judge(
  taskId: string,
  agentAnswer: string,
  rubric: TaskRubric,
  judgeModel: string      // defaults to 'mimo-v2.5' (cheapest)
): Promise<JudgeResult>
```

#### Rubric format (`rubrics/task-arch-trace.json`)

```json
{
  "taskId": "task-arch-trace",
  "criteria": [
    {
      "name": "watcher_identified",
      "weight": 0.25,
      "description": "Correctly identifies watcher/mod.rs as the file change detection module",
      "groundTruth": "src/watcher/mod.rs uses the notify crate to watch filesystem events"
    },
    {
      "name": "chunker_identified",
      "weight": 0.25,
      "description": "Correctly identifies tree-sitter chunking in engine/",
      "groundTruth": "src/engine/ uses tree-sitter grammars to split files into semantic chunks"
    },
    {
      "name": "embedder_identified",
      "weight": 0.25,
      "description": "Correctly identifies the Embedder trait and at least one provider",
      "groundTruth": "src/embedder/mod.rs defines the Embedder trait, with implementations in onnx.rs, gemini.rs, ollama.rs, openai.rs"
    },
    {
      "name": "store_identified",
      "weight": 0.25,
      "description": "Correctly identifies SQLite + sqlite-vec storage",
      "groundTruth": "src/store/mod.rs manages rusqlite with sqlite-vec extension for vector storage"
    }
  ]
}
```

#### Judge prompt template

```
You are an expert code reviewer evaluating an AI agent's answer.

## Task
{task.prompt}

## Agent's Answer
{agentAnswer}

## Evaluation Criteria
For each criterion below, score 0.0 (completely wrong/missing) to 1.0 (fully correct):

{criteria[i].name}: {criteria[i].description}
Ground truth: {criteria[i].groundTruth}

Respond in JSON: { "criteria": { "<name>": { "score": <float>, "reasoning": "<why>" } } }
```

#### Why this works

- **No false positives**: A model can't pass by accidentally mentioning "error.rs" in a negative context
- **Partial credit**: A model that gets 3/4 pipeline stages scores 0.75, not binary pass/fail
- **Reproducible**: The judge model is also cached; the same trajectory always gets the same score
- **Cheap**: mimo-v2.5 as judge costs fractions of a cent per evaluation

For write tasks (Task 4), we keep compiler verification (`cargo check`) as the primary gate — it's objectively correct and doesn't need a judge.

---

### 5. Enhanced Metrics & Reporting

#### Metrics collected per (task × model × arm)

| Metric | Description | How measured |
|---|---|---|
| `correctness` | 0.0–1.0 score | LLM-as-Judge (read tasks) or compiler pass/fail (write tasks) |
| `tokens_input` | Total input tokens | SDK usage counters |
| `tokens_output` | Total output tokens | SDK usage counters |
| `tokens_total` | Input + Output | Sum |
| `cost_usd` | Estimated cost | tokens × model pricing |
| `steps` | ReAct loop iterations | Counter |
| `tool_calls` | Total tool invocations | Counter |
| `unique_tools` | Distinct tools used | Set size |
| `duration_ms` | Wall-clock time | `Date.now()` delta |
| `first_correct_step` | Step where correctness >= 0.8 first achieved | Judge evaluated per step (optional, expensive) |
| `error_recovery_count` | Times agent recovered from a tool error | Counter |

#### Report output (`results/agent_eval_report.md`)

```markdown
# Agent Evaluation Report

Generated: 2026-06-22 | Workspace: abc123 | Mode: live

## Summary

| Task | Diff. | mimo-v2.5 (VC) | mimo-v2.5 (Trad) | Δ Tokens | Δ Steps | Δ Correctness |
|------|-------|----------------|------------------|----------|---------|---------------|
| symbol-lookup | ★ | ✅ 1.0 (340t, 2s) | ✅ 1.0 (520t, 3s) | -34.6% | -33% | 0 |
| arch-trace | ★★★ | ✅ 0.95 (1.2Kt, 4s) | ✅ 0.75 (3.8Kt, 8s) | -68.4% | -50% | +0.20 |
| ... | ... | ... | ... | ... | ... | ... |

## Per-Task Details
[Expandable sections with full tool call traces, judge reasoning, etc.]
```

Additionally, `results/agent_eval_report.json` contains the full structured data for programmatic consumption.

---

### 6. Agent Loop Refactor (`src/agents.ts`)

The current implementation duplicates the ReAct loop for OpenAI and Anthropic providers. Refactor to:

1. **Single `reactLoop()` function** that accepts a `ToolProvider` and an LLM call adapter
2. **Provider adapters** that normalize OpenAI/Anthropic responses into a common `LLMResponse` type
3. **Cache integration**: Before each LLM call, check cache. After each call, write to cache.

```typescript
interface LLMResponse {
  text: string;
  toolCalls: { name: string; args: Record<string, any>; id: string }[];
  tokens: { input: number; output: number };
  stopReason: 'end_turn' | 'tool_use' | 'max_tokens';
}

type LLMCallFn = (messages: Message[]) => Promise<LLMResponse>;

async function reactLoop(
  task: Task,
  tools: ToolProvider,
  llmCall: LLMCallFn,
  cache: CacheManager,
  maxSteps: number
): Promise<AgentResult>
```

This eliminates ~150 lines of duplicated loop logic and makes adding new providers a single adapter function.

---

### 7. Workspace Cleanup & Safety

- **Task 4 generates a file** (`src/cli/status_eval.rs`). The harness MUST delete it in a `finally` block regardless of success/failure.
- **Git dirty check**: Before each live run, verify `git status --porcelain` returns empty for `src/`. Abort if the workspace has uncommitted changes that could affect results.
- **Arm isolation**: Each arm runs in sequence (not parallel) against the same workspace state. VectorCode arm runs first (uses index), Traditional arm runs second (pure filesystem).

---

## Model Matrix

Initial cache population targets these 3 models across 2 provider SDKs:

| Model | SDK | Provider Flag | Notes |
|---|---|---|---|
| `mimo-v2.5` | `openai` | OpenAI-compatible | Cheapest, good for judge calls |
| `deepseek-v4-flash` | `openai` | OpenAI-compatible | Fast, competitive quality |
| `minimax-m3` | `anthropic` | Anthropic-compatible | Tests Anthropic SDK path |

Each model runs all 5 tasks × 2 arms = **30 total trajectories** for the initial population.

Future models can be added incrementally by running `--live --model=<new-model>` — the cache system stores each model's trajectories independently.

---

## Implementation Order

| Phase | What | Files | Depends on | Parallelizable |
|---|---|---|---|---|
| **P1** | Tool provider abstraction | `src/tools/*` | — | ✅ |
| **P2** | Cache system | `src/cache.ts`, `snapshots/` | — | ✅ |
| **P3** | Task definitions + rubrics | `src/tasks/*`, `rubrics/*` | — | ✅ |
| **P4** | LLM-as-Judge | `src/judge.ts` | P3 (rubrics) | — |
| **P5** | Agent loop refactor | `src/agents.ts` | P1, P2 | — |
| **P6** | Harness orchestration + reporting | `src/harness.ts` | P1–P5 | — |
| **P7** | First live cache population | `cache/` | P1–P6 | — |

P1, P2, P3 can be built in parallel. P4 depends on P3. P5 depends on P1+P2. P6 ties everything together.

---

## Verification Plan

### Automated Tests

1. **Dry-run plumbing** (no API, no cache):
   ```bash
   npm run eval -- --dry-run
   ```
   Must complete in <5 seconds, validate tool provider init/shutdown, task loading, and report generation.

2. **Cache replay** (no API):
   ```bash
   npm run eval -- --cached --model=mimo-v2.5
   ```
   Must complete in <10 seconds, produce identical results to the original live run.

3. **Compiler verification** (Task 4):
   After the agent generates `src/cli/status_eval.rs`, `cargo check` must pass. If code has syntax errors, the task reports failure.

4. **Judge determinism**:
   Running the judge twice on the same (answer, rubric) must produce identical scores (judge responses are also cached).

### Manual Verification

1. Run `--live` with `mimo-v2.5`, `deepseek-v4-flash`, and `minimax-m3` for all 5 tasks × 2 arms.
2. Review the generated `agent_eval_report.md` for plausibility.
3. Spot-check 2-3 judge evaluations against human judgment.
