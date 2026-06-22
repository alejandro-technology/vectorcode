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

### 1. Dry Run (Offline / Free Plumbing Check)
To test the harness piping, MCP tool resolution, and task verification without making any LLM calls or spending tokens:
```bash
npm run test:plumbing
```

### 2. Live Run (Real LLM Calls)
To evaluate the default model (`kimi-k2.6`) on all tasks:
```bash
npm run eval
```

To run a specific model (e.g. `qwen3.7-plus` or `mimo-v2.5-pro`):
```bash
npm run eval -- --model=qwen3.7-plus
```

To evaluate a single task (e.g. `task-2-write`):
```bash
npm run eval -- --task=task-2-write
```

## Results and Reports

Each execution saves reports in the `benchmarks/results/` directory:
- `agent_eval_report.json`: Detailed JSON containing token metrics, steps, duration, and list of tool calls.
- `agent_eval_report.md`: Markdown summary table for quick comparison.
