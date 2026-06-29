# VectorCode Agent Evaluation Harness

A Node.js/TypeScript-based E2E Agent Evaluation Harness that runs LLMs in a ReAct tool-use loop against the VectorCode MCP server to evaluate real developer agent capabilities.

## Models and Endpoints

The harness supports OpenCode.ai's OpenAI and Anthropic compatible model list:

| Model ID | Provider | SDK Protocol |
| --- | --- | --- |
| `glm-5.2` / `glm-5.1` | `openai` | OpenAI-compatible |
| `kimi-k2.7` / `kimi-k2.6` | `openai` | OpenAI-compatible |
| `deepseek-v4-pro` / `deepseek-v4-flash` | `openai` | OpenAI-compatible |
| `mimo-v2.5` / `mimo-v2.5-pro` | `openai` | OpenAI-compatible |
| `minimax-m3` / `minimax-m2.7` | `anthropic` | Anthropic-compatible |
| `qwen3.7-max` / `qwen3.7-plus` | `anthropic` | Anthropic-compatible |

## Setup

1. **Install Node.js dependencies**:
   ```bash
   npm install
   ```

2. **Configure API Keys**:
   Create a `.env` file in this directory or export the environment variable:
   ```env
   OPENCODE_API_KEY=your-opencode-api-key-here
   ```

3. **Compile VectorCode**:
   Ensure you have compiled the Rust binary in debug or release:
   ```bash
   cargo build
   ```

## Running the Evaluation

### CLI Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `--corpus=<id>` | `mock-mini` | Corpus to evaluate: `mock-mini`, `mini`, `vectorcode`, or `all` |
| `--repetitions=<N>` | `1` | Number of repetitions per condition (R≥3 recommended for statistics) |
| `--timeout=<ms>` | `120000` | Per-trial timeout in milliseconds |
| `--model=<list>` | All 3 defaults | Comma-separated model IDs to evaluate |
| `--task=<list>` | All tasks | Comma-separated task IDs to evaluate |
| `--arm=<list>` | Both arms | `vectorcode`, `traditional`, or both |
| `--dry-run` | — | Offline plumbing test (no LLM calls) |
| `--live` | — | Live LLM calls (updates cache) |
| `--cached` | — | Replay from cache only |
| `--update-cache` | — | Update cache with fresh LLM calls |

### 1. Dry Run (Offline / Free Plumbing Check)
To test the harness piping, MCP tool resolution, and task verification without making any LLM calls or spending tokens:
```bash
npm run test:plumbing
```

This runs the mock-mini corpus (2 tasks × 3 models × 2 arms = 12 trials).

To test all corpora (mini corpus requires network access for cloning):
```bash
npm run test:plumbing:all
```

### 2. Live Run (Real LLM Calls)
To evaluate all corpora with default settings:
```bash
npm run eval -- --corpus=all
```

To evaluate a specific corpus with repetitions:
```bash
npm run eval -- --corpus=vectorcode --repetitions=5
```

To run a specific model (e.g. `qwen3.7-plus`):
```bash
npm run eval -- --model=qwen3.7-plus
```

To evaluate a single task (e.g. `task-symbol-lookup`):
```bash
npm run eval -- --task=task-symbol-lookup
```

## Experimental Design

The evaluation uses a fully-crossed factorial design: **2 (arm) × M (model) × C (corpus) × T (tasks) × R (repetitions)**

- **Corpora**: mock-mini (plumbing), mini (primary), vectorcode (self-referential)
- **Arms**: `vectorcode` (semantic search via MCP) vs `traditional` (ripgrep + find + file read)
- **Randomization**: Latin square over tasks, arm order alternation across repetitions
- **Controls**: temperature=0, Bonferroni-corrected hypothesis testing, bootstrap CIs

### Hypotheses

| ID | Hypothesis |
|----|-----------|
| H1 | VectorCode arm achieves correctness ≥ Traditional with fewer total tokens |
| H2 | VectorCode arm completes hard tasks (≥★★★) with fewer tool calls and steps |
| H3 | VectorCode advantage is larger on cross-module tasks |
| H4 | VectorCode arm shows lower inter-model variance in correctness |

## Results and Reports

Each execution saves per-corpus reports in the `results/<corpus>/` directory:
- `agent_eval_report.json`: ExperimentReport with full trial data
- `statistical_analysis.json`: AnalysisReport with Wilcoxon tests, Cohen's d, hypothesis verdicts
- `agent_eval_report.md`: Human-readable Markdown with summary tables and efficiency ratios

When multiple corpora are evaluated, an `aggregate_report.md` is generated at `results/aggregate_report.md`.

## Cache Migration

If upgrading from a pre-Phase 2 version, migrate cache paths:
```bash
npm run migrate-cache
```

This moves `cache/<model>/<taskId>/<arm>/` → `cache/<model>/<corpus>/<taskId>/<arm>/`.
