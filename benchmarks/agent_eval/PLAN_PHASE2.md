# Phase 2 — End-to-End Agent Benchmark: Implementation Plan

> **Status**: Plan approved, pending implementation by SDD agent.
> **Supersedes**: `implementation_plan_sota.md` (v1 design). This is v2 with multi-corpus support and formal experimental protocol.

---

## 1. Research Objective

Measure whether an LLM agent equipped with semantic search tools (VectorCode MCP) solves code understanding tasks with greater efficiency and quality than an agent equipped with traditional text tools (ripgrep + find + file reading), controlling for model, task complexity, and corpus.

**RQ1**: Do semantic retrieval tools provide a measurable advantage in efficiency and correctness for agents solving development tasks on real codebases?

- **RQ1a (Efficiency)**: Does semantic retrieval reduce tool calls, steps, and tokens needed?
- **RQ1b (Correctness)**: Does semantic retrieval improve answer quality, especially on cross-module tasks?
- **RQ1c (Scaling)**: Does the gap widen as task complexity increases?

---

## 2. Hypotheses

| ID | Hypothesis | Rationale |
|----|-----------|-----------|
| **H1** | VectorCode arm achieves correctness ≥ Traditional arm with fewer total tokens. | Semantic retrieval returns higher information-density results, reducing iterative exploration. |
| **H2** | VectorCode arm completes tasks ≥ ★★★ with fewer tool calls and ReAct steps. | `vec_search` returns relevance-ranked results; `grep` requires precise text queries (vocabulary mismatch problem). |
| **H3** | VectorCode advantage is larger on cross-module tasks (arch-trace, refactor-plan) than on single-symbol lookup. | Graph retrieval tools (`vec_find_callers`, `vec_find_dependents`) provide structural relationships grep cannot discover. |
| **H4** | VectorCode arm shows lower inter-model variance in correctness. | High-quality retrieval reduces dependency on the model's query formulation ability. |
| **H0** | No statistically significant difference between arms on any metric. | Possible if corpora are small enough for grep + read to suffice. |

---

## 3. Directory Structure

Phase separation is enforced at the directory level. Phase 1 remains untouched.

```
benchmarks/
├── README.md                         # Overview + academic taxonomy (UPDATE)
├── CONTRIBUTING.md                   # How to add golden queries (unchanged)
├── corpus.toml                       # Shared corpus definitions (unchanged)
│
├── baseline/                         # PHASE 1 — Retrieval Evaluation (UNCHANGED)
│   ├── SCHEMA.md
│   ├── baseline-mock-mini.json
│   ├── baseline-mock-mini-structural.json
│   └── baseline-store-mock-mini.json
├── queries/                          # PHASE 1 (UNCHANGED)
│   ├── mock-mini.toml
│   ├── mock-mini-structural.toml
│   ├── mini.toml
│   └── mini_structural.toml
│
├── agent_eval/                       # PHASE 2 — End-to-End Agent Evaluation
│   ├── README.md                     # Quickstart (UPDATE)
│   ├── implementation_plan_sota.md   # Historical v1 (keep as reference)
│   ├── PLAN_PHASE2.md                # THIS DOCUMENT
│   ├── package.json                  # Dependencies (UPDATE: add simple-statistics)
│   ├── tsconfig.json
│   ├── src/
│   │   ├── harness.ts                # Orchestrator (REFACTOR: multi-corpus, repetitions, analysis)
│   │   ├── agents.ts                 # ReAct loop (EXTEND: temperature=0, structured logging)
│   │   ├── cache.ts                  # Trajectory cache (EXTEND: experiment metadata)
│   │   ├── judge.ts                  # LLM-as-Judge (EXTEND: multi-threshold)
│   │   ├── corpus.ts                 # NEW — Corpus lifecycle (init, index, cleanup)
│   │   ├── analysis.ts              # NEW — Statistical analysis module
│   │   ├── report.ts                # NEW — Academic report generator
│   │   ├── types.ts                  # EXTEND — Experiment types
│   │   ├── tools/
│   │   │   ├── types.ts              # UNCHANGED
│   │   │   ├── vectorcode.ts         # EXTEND — accept workspaceDir parameter
│   │   │   └── traditional.ts        # EXTEND — accept workspaceDir parameter
│   │   └── tasks/
│   │       ├── index.ts              # REFACTOR — corpus-aware task loader
│   │       ├── mock-mini/            # NEW — tasks for mock-mini corpus
│   │       │   └── *.ts
│   │       ├── mini/                 # NEW — tasks for mini corpus
│   │       │   └── *.ts
│   │       └── vectorcode/           # MOVE — existing 5 tasks
│   │           └── *.ts
│   ├── rubrics/
│   │   ├── mock-mini/                # NEW
│   │   │   └── *.json
│   │   ├── mini/                     # NEW
│   │   │   └── *.json
│   │   └── vectorcode/               # MOVE — existing 5 rubrics
│   │       └── *.json
│   ├── cache/                        # UNCHANGED structure
│   │   └── <model>/<corpus>/<taskId>/<arm>/trajectory.jsonl
│   └── results/
│       └── <corpus>/                 # Per-corpus reports
│           ├── agent_eval_report.json
│           ├── agent_eval_report.md
│           └── statistical_analysis.json
│
├── ctx_efficiency/                   # PHASE 3 — Context Efficiency (PLACEHOLDER)
│   └── README.md
│
└── scripts/                          # Shared scripts (UPDATE)
    ├── run-benchmarks.sh
    └── verify-baseline.sh
```

### Key structural decisions

1. **Tasks and rubrics are organized by corpus** (`tasks/<corpus>/`, `rubrics/<corpus>/`).
2. **Cache paths include corpus** (`cache/<model>/<corpus>/<taskId>/<arm>/`) to avoid collisions.
3. **Results are per-corpus** (`results/<corpus>/`).
4. **Phase 1 directories (`baseline/`, `queries/`) are NOT moved** — CI depends on them.
5. **Existing vectorcode tasks/rubrics are MOVED** into subdirectories, not duplicated.
6. **`ctx_efficiency/`** is created as an empty placeholder for Phase 3.

---

## 4. Corpus Strategy

### 4.1 Corpus definitions

Reuse existing `corpus.toml`. Each corpus has distinct characteristics:

| Corpus | Source | Files | Languages | Purpose in Phase 2 |
|--------|--------|-------|-----------|-------------------|
| `mock-mini` | `tests/fixtures/mini/` | 4 files | Rust, TS, Python | Plumbing tests, CI smoke, development. NOT for publication-quality results (mock embedder). |
| `mini` | thiserror + defu + itsdangerous | ~30 files | Rust, TS, Python | Primary evaluation corpus. Multi-language, multi-pattern. Real embedder. |
| `vectorcode` | The VectorCode project itself | ~75 files | Rust | Self-referential evaluation. Ground truth is well-known to researchers. |
| `vscode` | microsoft/vscode (sparse) | ~15K files | TypeScript | Scale test. Future — not in initial execution. |

### 4.2 Corpus lifecycle (`src/corpus.ts`)

Each corpus requires different setup before evaluation can begin:

```typescript
interface CorpusConfig {
  id: string;                          // 'mock-mini' | 'mini' | 'vectorcode' | 'vscode'
  sourcePath: string;                  // absolute path to corpus root
  needsCloning: boolean;               // true for mini, vscode (git clone/sparse checkout)
  needsIndexing: boolean;              // true for all (vectorcode arm needs the index)
  vectorcodeConfig?: {                 // optional .vectorcode/config.toml overrides
    provider: string;
    model: string;
    dims: number;
  };
}

interface CorpusManager {
  prepare(config: CorpusConfig): Promise<void>;    // clone + init + index
  getIndexStatus(): Promise<{ indexed: boolean; chunks: number }>;
  cleanup(): Promise<void>;                         // optional: remove cloned repos
  getWorkspaceDir(): string;                        // root path for tool providers
}
```

**Preparation flow per corpus:**

| Corpus | Prepare steps |
|--------|--------------|
| `mock-mini` | Verify `tests/fixtures/mini/` exists → `vectorcode init --provider mock` → `vectorcode index` |
| `mini` | Clone 3 repos into `.bench-corpus/` (gitignored) → `vectorcode init` per repo → `vectorcode index` per repo |
| `vectorcode` | Use project root directly → verify `.vectorcode/` exists → `vectorcode index` if stale |
| `vscode` | Sparse checkout into `.bench-corpus/vscode/` → `vectorcode init` → `vectorcode index` (slow) |

**Important**: The `traditional` arm does NOT need indexing — it operates directly on the filesystem. Only the `vectorcode` arm requires the index.

### 4.3 Corpus-specific task sets

Each corpus has its own task set. Tasks are defined as TypeScript modules under `src/tasks/<corpus>/`.

#### mock-mini tasks (2 tasks — plumbing only)

| ID | ★ | Prompt | Type |
|----|---|--------|------|
| `mock-error-lookup` | ★ | "Find the VectorCodeError enum definition. List all its variants." | Read |
| `mock-cross-lang` | ★★ | "Compare how rate limiting is implemented in rate_limiter.ts vs how signing works in signing.py." | Read |

These exist only for plumbing tests and CI. Mock embedder means no real semantic signal.

#### mini tasks (4 tasks — primary evaluation)

| ID | ★ | Prompt | Type |
|----|---|--------|------|
| `mini-error-derive` | ★★ | "In the thiserror crate, find the main derive macro. What error types does it support? List the attributes (#[from], #[source], etc.) and explain what each generates." | Read |
| `mini-merge-trace` | ★★★ | "In defu, trace the full merge pipeline. Start from the exported `defu()` function and follow the code path through to how individual properties are merged. Name each function and file involved." | Read |
| `mini-signing-flow` | ★★★★ | "In itsdangerous, trace the complete token signing and verification flow. Start from `Signer.sign()` through to timestamp encoding and signature generation. Identify the hash algorithm used and explain how tampering is detected." | Read |
| `mini-cross-repo` | ★★★★★ | "Compare how all three repos (thiserror, defu, itsdangerous) handle their primary public API surface. For each: (1) where is the main entry point, (2) how are errors/edge cases handled, (3) what design patterns are used for the public interface." | Read |

Each task targets ONE repo except the cross-repo task which spans all three.

**Rubric design for mini tasks**: Each rubric has 3-5 weighted criteria with ground truth strings. Example for `mini-merge-trace`:

```json
{
  "taskId": "mini-merge-trace",
  "corpus": "mini",
  "targetRepo": "defu",
  "criteria": [
    {
      "name": "entry_point_identified",
      "weight": 0.25,
      "description": "Correctly identifies the exported defu() function as the entry point",
      "groundTruth": "src/defu.ts exports the defu function"
    },
    {
      "name": "merge_function_traced",
      "weight": 0.30,
      "description": "Traces the internal _defu/assignMerge pipeline",
      "groundTruth": "defu calls _defu which recursively merges using assignMerge"
    },
    {
      "name": "array_handling_explained",
      "weight": 0.20,
      "description": "Explains how arrays are handled during merge (concat vs replace)",
      "groundTruth": "Arrays are concatenated by default, not replaced"
    },
    {
      "name": "custom_merger_support",
      "weight": 0.25,
      "description": "Mentions custom merger function support",
      "groundTruth": "defu accepts a custom merger function as the last argument"
    }
  ]
}
```

#### vectorcode tasks (5 tasks — existing, MOVE to subdirectory)

The 5 existing tasks are moved from `src/tasks/*.ts` to `src/tasks/vectorcode/*.ts` and rubrics from `rubrics/*.json` to `rubrics/vectorcode/*.json`. No content changes.

#### vscode tasks (0 tasks — future)

Placeholder. When Phase 2 is validated on smaller corpora, design vscode tasks for scale evaluation.

### 4.4 Task interface (extended)

```typescript
interface Task {
  id: string;
  corpus: string;                      // NEW: which corpus this task belongs to
  name: string;
  prompt: string;
  difficulty: number;                  // NEW: 1-5 stars
  type: 'read' | 'write';             // NEW: explicit type
  targetRepos?: string[];              // NEW: which repos within the corpus (for mini)
  verify: (workspaceDir: string) => Promise<{ success: boolean; error?: string }>;
}
```

---

## 5. Experimental Protocol

### 5.1 Design

Fully-crossed factorial design: **2 (arm) × M (model) × C (corpus) × T (tasks per corpus) × R (repetitions)**

Default parameters:
- **M** = 3 models: `mimo-v2.5`, `minimax-m3`, `deepseek-v4-flash`
- **C** = 3 corpora: `mock-mini`, `mini`, `vectorcode` (vscode deferred)
- **R** = 5 repetitions per cell
- **maxSteps** = 15 per trial
- **timeout** = 120s per trial
- **temperature** = 0

Total trials: 2 × 3 × (2 + 4 + 5) × 5 = **330 trials** (across all corpora).

CLI supports running subsets: `--corpus=mini --model=mimo-v2.5 --repetitions=3`.

### 5.2 Anti-confound controls

| Control | Mechanism |
|---------|-----------|
| **Order randomization** | Latin square over tasks within each (model × arm) block. Arm order alternates across repetitions. |
| **Workspace isolation** | Each trial operates on the same commit SHA. For `vectorcode` corpus, use git worktree or verify clean state. |
| **Temperature = 0** | All LLM calls use temperature=0 to minimize stochasticity. |
| **Identical system prompt** | Both arms receive the same system prompt describing their available tools. |
| **Budget cap** | maxSteps=15, timeout=120s. Trial exceeding either is recorded as timeout with score=0. |
| **Tool description parity** | Both tool sets use descriptions of similar length and detail level. No hints favoring either arm. |

### 5.3 Execution flow

```
FOR EACH corpus c in [mock-mini, mini, vectorcode]:
  Prepare corpus (clone, init, index)
  
  FOR EACH repetition r in [1..R]:
    Generate Latin square order for tasks in corpus c
    Shuffle arm order (50% vectorcode-first, 50% traditional-first)
    
    FOR EACH arm a in shuffled_arms:
      Initialize ToolProvider for arm a on corpus workspace
      
      FOR EACH task t in ordered_tasks:
        1. Record start time
        2. Run reactLoop(prompt, tools, model, maxSteps=15, timeout=120s)
        3. Record: trajectory, tokens, tool_calls, steps, duration
        4. If write task: run compiler gate, record pass/fail
        5. If read task: run LLM-as-Judge(rubric, answer)
        6. Record correctness ∈ [0, 1]
        7. Record trial result
      
      Shutdown ToolProvider
  
  Cleanup corpus (optional)

Run statistical analysis across all results
Generate reports per corpus and aggregate
```

---

## 6. Metrics

### 6.1 Primary metrics (per trial)

| Metric | Type | Definition |
|--------|------|------------|
| `tokens_input` | Continuous | Total input tokens consumed |
| `tokens_output` | Continuous | Total output tokens generated |
| `tokens_total` | Continuous | input + output |
| `steps` | Continuous | ReAct loop iterations |
| `tool_calls` | Continuous | Total tool invocations |
| `unique_tools` | Continuous | Distinct tools used |
| `duration_ms` | Continuous | Wall-clock time |
| `correctness` | Continuous | Judge score ∈ [0, 1] or binary for write tasks |
| `success` | Binary | correctness ≥ 0.8 |

### 6.2 Derived metrics (per arm × corpus × task)

| Metric | Formula |
|--------|---------|
| **Token Efficiency Ratio (TER)** | `tokens_total(traditional) / tokens_total(vectorcode)` |
| **Step Efficiency Ratio (SER)** | `steps(traditional) / steps(vectorcode)` |
| **Task Success Rate (TSR)** | `count(success) / R` |
| **Mean Correctness Score (MCS)** | `mean(correctness)` |
| **Tool Call Efficiency** | `correctness / tool_calls` |
| **Token Cost (USD)** | `tokens_input × price_in + tokens_output × price_out` |

### 6.3 Composite metrics

| Metric | Formula |
|--------|---------|
| **Agent Efficiency Index (AEI)** | `correctness × log(1 + 1/tokens_total) × success` |
| **Difficulty-Adjusted Efficiency (DAE)** | `AEI / task_difficulty` |

### 6.4 Statistical analysis (`src/analysis.ts`)

```typescript
interface StatisticalResult {
  metric: string;
  corpus: string;
  taskId: string;
  model: string;
  vectorcode: { mean: number; std: number; ci95: [number, number] };
  traditional: { mean: number; std: number; ci95: [number, number] };
  testStatistic: number;
  pValue: number;
  effectSize: number;           // Cohen's d
  effectMagnitude: string;      // 'negligible' | 'small' | 'medium' | 'large'
  significant: boolean;         // after Bonferroni correction
}

interface AnalysisReport {
  results: StatisticalResult[];
  bonferroniAlpha: number;      // 0.05 / numComparisons
  totalComparisons: number;
  significantCount: number;
  summary: {
    ter: Record<string, number>;      // corpus → TER
    ser: Record<string, number>;      // corpus → SER
    hypotheses: HypothesisVerdict[];  // H1-H4 verdicts
  };
}

interface HypothesisVerdict {
  id: string;                   // H1, H2, H3, H4
  supported: boolean;
  evidence: string;
  effectSize: number;
}
```

**Statistical tests**:
- **Primary**: Wilcoxon signed-rank test (paired, non-parametric) for each (metric × task × model) comparison.
- **Effect size**: Cohen's d with classification: negligible (<0.2), small (0.2-0.5), medium (0.5-0.8), large (>0.8).
- **Multiple comparison correction**: Bonferroni — α_adjusted = 0.05 / total_comparisons.
- **Confidence intervals**: 95% CI for mean differences using bootstrapping (B=1000) if R < 10.

**Implementation**: Use `simple-statistics` npm package (lightweight, no native deps) for Wilcoxon, Cohen's d, and CI calculations. Do NOT implement statistical tests from scratch.

### 6.5 Hypothesis mapping to metrics

| Hypothesis | Primary metric | Decision rule |
|-----------|---------------|---------------|
| H1 (token efficiency) | TER across all corpora | TER > 1.0 with p < α_adjusted on ≥ 2 corpora |
| H2 (step efficiency) | SER on tasks ≥ ★★★ | SER > 1.0 with p < α_adjusted on ≥ 50% of hard tasks |
| H3 (cross-module advantage) | ΔMCS(arch-trace, refactor) vs ΔMCS(symbol-lookup) | ΔMCS_hard > ΔMCS_easy with p < α_adjusted |
| H4 (lower variance) | Var(correctness_vectorcode) vs Var(correctness_traditional) | Var_VC < Var_Trad with Levene's test p < 0.05 |

---

## 7. Report Generation (`src/report.ts`)

### 7.1 Per-corpus report (`results/<corpus>/agent_eval_report.md`)

```markdown
# Agent Evaluation Report — {corpus}

- **Corpus**: {corpus_id} ({file_count} files, {languages})
- **Workspace SHA**: `{git_sha}`
- **Date**: {iso_timestamp}
- **Mode**: {cache_mode}
- **Models**: {model_list}
- **Repetitions**: R={R}

## Summary

| Task | ★ | Arm | MCS ± CI95 | TSR | Mean Tokens | Mean Steps | Mean Duration |
|------|---|-----|-----------|-----|-------------|------------|---------------|
| {id} | {d} | VC  | 0.85 ± 0.08 | 80% | 1,234 | 4.2 | 8.3s |
| {id} | {d} | TR  | 0.72 ± 0.12 | 60% | 2,891 | 7.1 | 14.2s |

## Efficiency Ratios

| Task | ★ | TER | SER | ΔMCS | Cohen's d | Significant? |
|------|---|-----|-----|------|-----------|-------------|
| {id} | {d} | 2.34 | 1.69 | +0.13 | 0.82 (large) | ✅ p=0.003 |

## Hypothesis Verdicts

| ID | Hypothesis | Verdict | Evidence |
|----|-----------|---------|----------|
| H1 | Token efficiency | ✅ Supported | TER > 1.0 on 3/3 corpora (p < 0.001) |
| H2 | Step efficiency | ⚠️ Partial | SER > 1.0 on 4/6 hard tasks |
| H3 | Cross-module advantage | ✅ Supported | ΔMCS_hard > ΔMCS_easy (p=0.02) |
| H4 | Lower variance | ❌ Not supported | No significant variance difference |
```

### 7.2 Aggregate report (`results/aggregate_report.md`)

Cross-corpus summary table, hypothesis verdicts with combined evidence, and effect size forest plot data.

### 7.3 JSON reports

Both `agent_eval_report.json` (raw trial data) and `statistical_analysis.json` (computed statistics) are emitted per corpus. The JSON schema must be stable for CI consumption.

---

## 8. File-Level Implementation Spec

### 8.1 Files to MODIFY

#### `src/types.ts` — extend with experiment types

Add these interfaces (do not remove existing ones):

```typescript
// Corpus types
export interface CorpusConfig {
  id: string;
  sourcePath: string;
  needsCloning: boolean;
  needsIndexing: boolean;
  vectorcodeConfig?: {
    provider: string;
    model: string;
    dims: number;
  };
}

// Experiment types
export interface ExperimentConfig {
  corpora: string[];
  models: string[];
  arms: ('vectorcode' | 'traditional')[];
  repetitions: number;
  maxSteps: number;
  timeoutMs: number;
  temperature: number;
}

export interface TrialResult {
  corpus: string;
  taskId: string;
  model: string;
  arm: 'vectorcode' | 'traditional';
  repetition: number;
  success: boolean;
  correctness: number;
  steps: number;
  tokens: { input: number; output: number; total: number };
  toolCalls: ToolCallRecord[];
  uniqueTools: number;
  durationMs: number;
  timedOut: boolean;
  error?: string;
  judgeResult?: JudgeResult;
  workspaceSha: string;
  timestamp: string;
}

export interface ExperimentReport {
  config: ExperimentConfig;
  trials: TrialResult[];
  generatedAt: string;
}
```

#### `src/harness.ts` — major refactor

The current `main()` function needs to be restructured into:

1. **CLI argument parsing** (existing, extend with `--corpus=`, `--repetitions=`, `--timeout=`)
2. **Corpus preparation** (NEW: call CorpusManager.prepare() for each corpus)
3. **Trial execution loop** (REFACTOR: add repetition loop, Latin square randomization)
4. **Post-hoc analysis** (NEW: call analysis module)
5. **Report generation** (REFACTOR: per-corpus + aggregate reports)

Key changes:
- Add `--corpus=` CLI arg (default: `mock-mini` for quick runs, `all` for full evaluation)
- Add `--repetitions=` CLI arg (default: 1 for development, 5 for publication)
- Add `--timeout=` CLI arg (default: 120000ms)
- Results path changes from `results/` to `results/<corpus>/`
- Cache path changes from `cache/<model>/<taskId>/<arm>/` to `cache/<model>/<corpus>/<taskId>/<arm>/`

#### `src/agents.ts` — extend with temperature control

- Add `temperature` parameter to `AgentConfig` interface
- Pass `temperature: 0` to all LLM API calls
- Add `timedOut: boolean` to agent result (set when timeout fires before convergence)

#### `src/cache.ts` — extend metadata

Add to `TrajectoryMetadata`:
```typescript
interface TrajectoryMetadata {
  // existing fields...
  corpus: string;           // NEW
  repetition: number;       // NEW
  experimentConfig: {       // NEW — snapshot of experiment parameters
    maxSteps: number;
    timeoutMs: number;
    temperature: number;
  };
}
```

#### `src/judge.ts` — multi-threshold reporting

Currently hardcodes threshold at 0.8. Change to:
- Return raw score without threshold decision
- Let harness decide success based on configurable threshold(s)
- Report scores at multiple thresholds in the analysis (0.6, 0.7, 0.8, 0.9)

#### `src/tools/vectorcode.ts` — accept workspace directory

Constructor changes: `new VectorCodeProvider(binPath, workspaceDir)` instead of hardcoded project root.

#### `src/tools/traditional.ts` — accept workspace directory

Constructor changes: `new TraditionalProvider(workspaceDir)` instead of hardcoded `../../`.

#### `src/tasks/index.ts` — corpus-aware task loader

```typescript
import { mockMiniTasks } from './mock-mini/index.js';
import { miniTasks } from './mini/index.js';
import { vectorcodeTasks } from './vectorcode/index.js';

export function getTasksForCorpus(corpus: string): Task[] {
  switch (corpus) {
    case 'mock-mini': return mockMiniTasks;
    case 'mini': return miniTasks;
    case 'vectorcode': return vectorcodeTasks;
    default: throw new Error(`Unknown corpus: ${corpus}`);
  }
}

export const allTasks = [...mockMiniTasks, ...miniTasks, ...vectorcodeTasks];
```

#### `benchmarks/README.md` — update taxonomy section

Update the status table to reflect Phase 2 implementation.

#### `package.json` — add dependency

```json
{
  "dependencies": {
    "simple-statistics": "^7.8.3"
  }
}
```

### 8.2 Files to CREATE

#### `src/corpus.ts` — corpus lifecycle manager

Responsibilities:
- Parse `corpus.toml` to get corpus definitions
- For `mini`: clone repos into `.bench-corpus/mini/` (sparse checkout `src/` only)
- For `vectorcode`: verify project root is clean, check `.vectorcode/` exists
- For `mock-mini`: verify `tests/fixtures/mini/` exists
- Run `vectorcode init` + `vectorcode index` on each corpus workspace
- Return workspace directory for tool providers
- Provide cleanup function

Key implementation:
```typescript
export class CorpusManager {
  private workspaceDir: string;

  async prepare(corpusId: string): Promise<string> {
    const config = this.loadCorpusConfig(corpusId);
    
    if (config.needsCloning) {
      await this.cloneRepo(config);
    }
    
    this.workspaceDir = this.resolveWorkspaceDir(corpusId);
    
    if (config.needsIndexing) {
      await this.initVectorCode(this.workspaceDir, config.vectorcodeConfig);
      await this.indexCorpus(this.workspaceDir);
    }
    
    return this.workspaceDir;
  }

  async cleanup(): Promise<void> {
    // Remove .bench-corpus/ if it was created by this run
  }
}
```

#### `src/analysis.ts` — statistical analysis module

Responsibilities:
- Collect trial results by (arm × model × corpus × task)
- Run Wilcoxon signed-rank test for paired comparisons
- Compute Cohen's d effect sizes
- Compute 95% confidence intervals
- Apply Bonferroni correction
- Compute derived metrics (TER, SER, TSR, MCS, AEI, DAE)
- Evaluate hypotheses against evidence
- Return structured `AnalysisReport`

Key implementation:
```typescript
import ss from 'simple-statistics';

export function analyzeExperiment(report: ExperimentReport): AnalysisReport {
  const { trials, config } = report;
  const results: StatisticalResult[] = [];
  
  // Group trials by (corpus, task, model, arm)
  const groups = groupBy(trials, ['corpus', 'taskId', 'model', 'arm']);
  
  // For each (corpus, task, model) combination, compare arms
  const comparisons = generateComparisons(groups);
  const numComparisons = comparisons.length;
  const bonferroniAlpha = 0.05 / numComparisons;
  
  for (const comp of comparisons) {
    const vcValues = comp.vectorcode.map(t => t[comp.metric]);
    const tradValues = comp.traditional.map(t => t[comp.metric]);
    
    // Wilcoxon signed-rank (paired)
    const { statistic, pValue } = wilcoxonSignedRank(vcValues, tradValues);
    
    // Cohen's d
    const d = cohensD(vcValues, tradValues);
    
    results.push({
      metric: comp.metric,
      corpus: comp.corpus,
      taskId: comp.taskId,
      model: comp.model,
      vectorcode: { mean: ss.mean(vcValues), std: ss.standardDeviation(vcValues), ci95: bootstrapCI(vcValues) },
      traditional: { mean: ss.mean(tradValues), std: ss.standardDeviation(tradValues), ci95: bootstrapCI(tradValues) },
      testStatistic: statistic,
      pValue,
      effectSize: d,
      effectMagnitude: classifyEffect(d),
      significant: pValue < bonferroniAlpha,
    });
  }
  
  return {
    results,
    bonferroniAlpha,
    totalComparisons: numComparisons,
    significantCount: results.filter(r => r.significant).length,
    summary: computeSummary(results, config),
  };
}
```

**Note on `simple-statistics`**: This package provides mean, standardDeviation, and other descriptive stats. For Wilcoxon signed-rank test, implement a simple version (it's ~30 lines) or use the `wilcoxon` npm package. Cohen's d is straightforward: `d = (mean1 - mean2) / pooled_std`. Bootstrap CI is ~20 lines.

#### `src/report.ts` — academic report generator

Responsibilities:
- Generate per-corpus Markdown report (summary tables, efficiency ratios, hypothesis verdicts)
- Generate aggregate Markdown report (cross-corpus summary)
- Generate JSON reports (raw + statistical)
- Format numbers with appropriate precision

#### `src/tasks/mock-mini/index.ts` — mock-mini task registry

Export 2 tasks: `mock-error-lookup`, `mock-cross-lang`.

#### `src/tasks/mock-mini/task-error-lookup.ts`

Task definition for finding VectorCodeError in mock-mini corpus.

#### `src/tasks/mock-mini/task-cross-lang.ts`

Task definition for cross-language comparison in mock-mini corpus.

#### `src/tasks/mini/index.ts` — mini task registry

Export 4 tasks: `mini-error-derive`, `mini-merge-trace`, `mini-signing-flow`, `mini-cross-repo`.

#### `src/tasks/mini/task-error-derive.ts`
#### `src/tasks/mini/task-merge-trace.ts`
#### `src/tasks/mini/task-signing-flow.ts`
#### `src/tasks/mini/task-cross-repo.ts`

Each follows the same pattern as existing vectorcode tasks.

#### `src/tasks/vectorcode/index.ts` — vectorcode task registry (MOVE existing)

Move existing 5 tasks here. Re-export.

#### Rubric files to create

```
rubrics/mock-mini/mock-error-lookup.json
rubrics/mock-mini/mock-cross-lang.json
rubrics/mini/mini-error-derive.json
rubrics/mini/mini-merge-trace.json
rubrics/mini/mini-signing-flow.json
rubrics/mini/mini-cross-repo.json
```

Move existing rubrics:
```
rubrics/task-*.json → rubrics/vectorcode/task-*.json
```

#### `benchmarks/ctx_efficiency/README.md` — Phase 3 placeholder

```markdown
# Phase 3 — Context Efficiency Evaluation

> Placeholder. This phase measures token cost and RAG system scalability
> during code comprehension tasks. Design will follow Phase 2 validation.

See `../agent_eval/PLAN_PHASE2.md` for the Phase 2 protocol that this
phase will build upon.
```

### 8.3 Files to MOVE (rename)

| From | To |
|------|-----|
| `src/tasks/task-symbol-lookup.ts` | `src/tasks/vectorcode/task-symbol-lookup.ts` |
| `src/tasks/task-arch-trace.ts` | `src/tasks/vectorcode/task-arch-trace.ts` |
| `src/tasks/task-bug-hunt.ts` | `src/tasks/vectorcode/task-bug-hunt.ts` |
| `src/tasks/task-impl-status.ts` | `src/tasks/vectorcode/task-impl-status.ts` |
| `src/tasks/task-refactor-plan.ts` | `src/tasks/vectorcode/task-refactor-plan.ts` |
| `rubrics/task-symbol-lookup.json` | `rubrics/vectorcode/task-symbol-lookup.json` |
| `rubrics/task-arch-trace.json` | `rubrics/vectorcode/task-arch-trace.json` |
| `rubrics/task-bug-hunt.json` | `rubrics/vectorcode/task-bug-hunt.json` |
| `rubrics/task-status-command.json` | `rubrics/vectorcode/task-status-command.json` |
| `rubrics/task-refactor-plan.json` | `rubrics/vectorcode/task-refactor-plan.json` |

---

## 9. Implementation Phases

Ordered by dependency. Each phase is a unit of work for the implementing agent.

### Phase A: Structural reorganization (no new logic)

1. Create directory structure: `tasks/mock-mini/`, `tasks/mini/`, `tasks/vectorcode/`, `rubrics/mock-mini/`, `rubrics/mini/`, `rubrics/vectorcode/`, `ctx_efficiency/`
2. MOVE existing task files and rubrics into subdirectories
3. Update `tasks/index.ts` to import from new locations
4. Update `harness.ts` rubric path lookup: `rubrics/${taskId}.json` → `rubrics/${corpus}/${taskId}.json`
5. Create `ctx_efficiency/README.md` placeholder
6. **Verify**: `npm run test:plumbing` passes (dry-run with existing vectorcode tasks)

### Phase B: Corpus management (`src/corpus.ts`)

1. Implement `CorpusManager` class
2. Parse `corpus.toml` for corpus definitions
3. Implement clone/prepare for `mini` corpus
4. Implement verify for `mock-mini` and `vectorcode`
5. Implement cleanup
6. **Verify**: Can prepare each corpus and get workspace directory

### Phase C: Tool provider workspace parameterization

1. Update `VectorCodeProvider` constructor to accept `workspaceDir`
2. Update `TraditionalProvider` constructor to accept `workspaceDir`
3. Update all tool implementations to use the parameterized workspace
4. **Verify**: Both providers work with explicit workspace directory

### Phase D: New tasks and rubrics

1. Create mock-mini tasks (2 tasks + 2 rubrics)
2. Create mini tasks (4 tasks + 4 rubrics)
   - Requires reading thiserror, defu, and itsdangerous source code
   - Write ground truth based on actual code structure
3. Update `tasks/index.ts` with `getTasksForCorpus()`
4. **Verify**: All tasks load correctly, rubrics parse correctly

### Phase E: Agent loop enhancements

1. Add `temperature: 0` to all LLM calls in `agents.ts`
2. Add timeout support (120s default, configurable)
3. Add `timedOut: boolean` to result
4. Update cache metadata with corpus, repetition, experimentConfig
5. **Verify**: Agent runs respect temperature and timeout

### Phase F: Harness refactor (`src/harness.ts`)

1. Add CLI args: `--corpus=`, `--repetitions=`, `--timeout=`
2. Implement repetition loop with Latin square randomization
3. Implement arm order alternation
4. Integrate `CorpusManager` for corpus preparation
5. Update result recording with new fields (corpus, repetition, timedOut)
6. Update cache paths to include corpus
7. **Verify**: `npm run eval -- --corpus=mock-mini --dry-run` runs all mock-mini tasks

### Phase G: Statistical analysis (`src/analysis.ts`)

1. Add `simple-statistics` dependency
2. Implement trial grouping and comparison generation
3. Implement Wilcoxon signed-rank test
4. Implement Cohen's d calculation
5. Implement bootstrap confidence intervals
6. Implement Bonferroni correction
7. Implement derived metrics (TER, SER, TSR, MCS, AEI, DAE)
8. Implement hypothesis evaluation logic
9. **Verify**: Analysis produces correct results on synthetic data

### Phase H: Report generation (`src/report.ts`)

1. Implement per-corpus Markdown report (summary, efficiency ratios, hypotheses)
2. Implement aggregate Markdown report
3. Implement JSON reports (raw + statistical)
4. Wire report generation into harness main loop
5. **Verify**: Reports generate correctly from trial data

### Phase I: Integration testing

1. Run `--corpus=mock-mini --dry-run` end-to-end
2. Run `--corpus=mock-mini --cached` (after populating cache)
3. Verify all paths: corpus prep → trial execution → analysis → report
4. Fix any integration issues
5. **Verify**: Full dry-run pipeline works for all 3 corpora

### Phase J: First live population

1. Run `--corpus=mock-mini --live` (cheap, mock embedder)
2. Run `--corpus=mini --live --model=mimo-v2.5` (first real evaluation)
3. Run `--corpus=vectorcode --live --model=mimo-v2.5`
4. Review results for plausibility
5. If results look good, run full matrix (3 models × 3 corpora × 5 repetitions)
6. Commit cache as baseline
7. **Verify**: Results are reproducible via `--cached` mode

---

## 10. Verification Plan

### 10.1 Automated checks per phase

| Phase | Check | Command |
|-------|-------|---------|
| A | Dry-run passes after reorganization | `npm run test:plumbing` |
| B | Corpus preparation works | `node -e "new CorpusManager().prepare('mock-mini')"` |
| C | Providers work with workspace param | Integration test with explicit path |
| D | Tasks load for all corpora | `node -e "getTasksForCorpus('mini')"` |
| E | Temperature and timeout respected | Check LLM call params in dry-run |
| F | Harness runs with new CLI args | `npm run eval -- --corpus=mock-mini --repetitions=2 --dry-run` |
| G | Analysis on synthetic data | Unit test with known distributions |
| H | Reports generate correctly | Inspect output files |
| I | Full pipeline end-to-end | `npm run eval -- --corpus=mock-mini --dry-run` |
| J | Live results reproducible | Run cached, compare to live |

### 10.2 Judge validation study

Before Phase J, run a spot-check:
1. Select 20 random (answer, rubric) pairs from any corpus
2. Score them manually (human)
3. Score them with LLM-as-Judge
4. Compute Cohen's κ
5. If κ < 0.7, adjust rubrics before full population

### 10.3 Reproducibility check

1. Run `--cached` twice on the same data
2. Verify bit-exact output (same JSON, same Markdown)
3. Verify all metrics match to floating-point precision

---

## 11. Acceptance Criteria

The Phase 2 benchmark is **complete** when:

- [ ] Directory structure separates phases clearly (Fase 1 untouched, Fase 2 in agent_eval/, Fase 3 placeholder exists)
- [ ] 3 corpora are supported: mock-mini, mini, vectorcode
- [ ] 11 total tasks: 2 mock-mini + 4 mini + 5 vectorcode
- [ ] Repetition runner works with Latin square randomization
- [ ] Statistical analysis produces Wilcoxon tests, Cohen's d, Bonferroni-corrected p-values
- [ ] Reports include per-corpus summaries, efficiency ratios, and hypothesis verdicts
- [ ] `npm run test:plumbing` passes (dry-run for all corpora)
- [ ] At least one live population has been run and cached
- [ ] Cached replay produces identical results
- [ ] Judge validation shows κ ≥ 0.7 against human scoring (spot-check)

---

## 12. Out of Scope (deferred to future work)

- **vscode corpus**: Infrastructure supports it but initial population is deferred (scale test)
- **Multi-language tasks**: Current tasks are per-repo. True multi-language tasks (same concept across Rust/TS/Python) are future work
- **CI integration for Phase 2**: Phase 1 CI gate is sufficient for now. Phase 2 CI gate comes after baseline is established
- **Write tasks for mini/vectorcode corpora beyond the existing one**: Only Task 4 (impl-status) is a write task. Adding more write tasks is future work
- **Cross-model comparison as primary outcome**: The experiment is designed for arm comparison, not model ranking
