import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import * as dotenv from 'dotenv';
import * as fs from 'fs';
import * as path from 'path';
import { tasks } from './tasks.js';
import { runAgent } from './agents.js';
import { EvalResult, ToolCallRecord } from './types.js';

dotenv.config();

function getBinPath(): string {
  if (process.env.VECTORCODE_BIN) {
    return path.resolve(process.env.VECTORCODE_BIN);
  }
  // Default to workspace debug binary
  return path.resolve('../../target/debug/vectorcode');
}

async function runTask(
  taskId: string,
  model: string,
  provider: 'openai' | 'anthropic' | 'dry-run'
): Promise<EvalResult> {
  const task = tasks.find(t => t.id === taskId);
  if (!task) {
    throw new Error(`Task with ID ${taskId} not found`);
  }

  const binPath = getBinPath();
  if (!fs.existsSync(binPath) && provider !== 'dry-run') {
    throw new Error(`VectorCode binary not found at: ${binPath}. Please compile it using 'cargo build' first.`);
  }

  console.log(`[Harness] Launching VectorCode MCP Server: ${binPath}`);
  
  let client: Client | null = null;
  let transport: StdioClientTransport | null = null;
  let mcpTools: any[] = [];

  if (provider !== 'dry-run') {
    transport = new StdioClientTransport({
      command: binPath,
      args: ['serve', '--mcp'],
      env: process.env as any
    });

    client = new Client(
      { name: 'vectorcode-eval-harness', version: '1.0.0' },
      { capabilities: {} }
    );

    await client.connect(transport);
    console.log('[Harness] Connected to VectorCode MCP Server');

    const toolsResponse = await client.listTools();
    mcpTools = toolsResponse.tools;
    console.log(`[Harness] Discovered ${mcpTools.length} MCP tools`);
  } else {
    // Dummy tools for dry-run
    mcpTools = [
      { name: 'vec_search', description: 'Semantic search', inputSchema: {} },
      { name: 'vec_outline', description: 'AST outline', inputSchema: {} }
    ];
  }

  const callMcpTool = async (name: string, args: any): Promise<string> => {
    if (provider === 'dry-run' || !client) {
      if (name === 'vec_search') {
        return 'Mock search results: src/error.rs contains VectorCodeError';
      }
      return 'Mock tool result';
    }
    const response = await client.callTool({
      name,
      arguments: args
    });
    return response.content
      .filter(c => c.type === 'text')
      .map((c: any) => c.text)
      .join('\n');
  };

  const startMs = Date.now();
  console.log(`[Harness] Executing task: ${task.name} with model: ${model}`);

  let success = false;
  let steps = 0;
  let tokens = { input: 0, output: 0, total: 0 };
  let toolCalls: ToolCallRecord[] = [];
  let error: string | undefined;

  try {
    const agentResult = await runAgent(task, { model, provider }, mcpTools, callMcpTool);
    steps = agentResult.steps;
    tokens = agentResult.tokens;
    toolCalls = agentResult.toolCalls;

    // Verify workspace state
    const workspaceDir = path.resolve('../../'); // Project root
    const verification = await task.verify(workspaceDir);
    success = verification.success;
    if (!success) {
      error = verification.error || 'Task verification failed';
    }

    // Special verification for read task: check if agent answered correctly
    if (task.id === 'task-1-read') {
      const answer = agentResult.finalAnswer.toLowerCase();
      const hasErrorFile = answer.includes('error.rs');
      const hasFtsFile = answer.includes('fts.rs');
      if (hasErrorFile && hasFtsFile) {
        success = true;
      } else {
        success = false;
        error = `Agent did not identify the correct files. Output: ${agentResult.finalAnswer.substring(0, 200)}...`;
      }
    }

    // Special verification for write task: check if agent outputted correct code
    if (task.id === 'task-2-write') {
      const answer = agentResult.finalAnswer;
      const hasFn = answer.includes('pub fn run_status') || answer.includes('pub fn run_status(') || answer.includes('fn run_status');
      const hasReturn = answer.includes('Mock Status: OK');
      if (hasFn && hasReturn) {
        success = true;
      } else {
        success = false;
        error = `Agent did not output the correct Rust implementation. Output: ${agentResult.finalAnswer.substring(0, 200)}...`;
      }
    }

  } catch (err: any) {
    success = false;
    error = err.message;
  } finally {
    if (client) {
      await client.close();
    }
  }

  const durationMs = Date.now() - startMs;
  return {
    taskId,
    model,
    provider,
    success,
    steps,
    tokens,
    toolCalls,
    error,
    durationMs
  };
}

async function main() {
  const args = process.argv.slice(2);
  const isDryRun = args.includes('--dry-run');

  // Parse arguments or set defaults
  const taskIdArg = args.find(a => a.startsWith('--task='))?.split('=')[1];
  const modelArg = args.find(a => a.startsWith('--model='))?.split('=')[1];
  
  const tasksToRun = taskIdArg ? [taskIdArg] : tasks.map(t => t.id);

  let defaultModel = 'kimi-k2.6';
  let defaultProvider: 'openai' | 'anthropic' | 'dry-run' = 'openai';

  if (modelArg) {
    defaultModel = modelArg;
    // Map model name to provider based on user model list
    const anthropicModels = [
      'minimax-m3', 'minimax-m2.7', 'minimax-m2.5',
      'qwen3.7-max', 'qwen3.7-plus', 'qwen3.6-plus'
    ];
    if (anthropicModels.includes(defaultModel)) {
      defaultProvider = 'anthropic';
    } else {
      defaultProvider = 'openai';
    }
  }

  if (isDryRun) {
    defaultModel = 'dry-run-model';
    defaultProvider = 'dry-run';
  }

  console.log('===================================================');
  console.log(` Starting VectorCode Agent Evaluation Suite`);
  console.log(` Provider: ${defaultProvider.toUpperCase()}`);
  console.log(` Model:    ${defaultModel}`);
  console.log('===================================================');

  const results: EvalResult[] = [];

  for (const taskId of tasksToRun) {
    try {
      const result = await runTask(taskId, defaultModel, defaultProvider);
      results.push(result);
      console.log(`\nTask: ${taskId}`);
      console.log(`Status:    ${result.success ? '✅ SUCCESS' : '❌ FAILED'}`);
      console.log(`Duration:  ${(result.durationMs / 1000).toFixed(2)}s`);
      console.log(`Steps:     ${result.steps}`);
      console.log(`Tokens:    In: ${result.tokens.input} | Out: ${result.tokens.output} | Total: ${result.tokens.total}`);
      console.log(`ToolCalls: ${result.toolCalls.length} calls`);
      if (result.error) {
        console.log(`Error:     ${result.error}`);
      }
      console.log('---------------------------------------------------');
    } catch (err: any) {
      console.error(`Failed executing task ${taskId}:`, err.message);
    }
  }

  // Save report
  const resultsDir = path.resolve('../results');
  if (!fs.existsSync(resultsDir)) {
    fs.mkdirSync(resultsDir, { recursive: true });
  }

  const reportPath = path.join(resultsDir, 'agent_eval_report.json');
  fs.writeFileSync(reportPath, JSON.stringify(results, null, 2));
  console.log(`[Harness] Report written to: ${reportPath}`);

  // Generate a beautiful markdown summary table
  let mdReport = `# Agent Evaluation Summary\n\n`;
  mdReport += `| Task ID | Model | Success | Steps | Tokens | Tool Calls | Duration |\n`;
  mdReport += `| --- | --- | --- | --- | --- | --- | --- |\n`;
  for (const r of results) {
    mdReport += `| ${r.taskId} | ${r.model} | ${r.success ? '✅' : '❌'} | ${r.steps} | ${r.tokens.total} | ${r.toolCalls.length} | ${(r.durationMs / 1000).toFixed(2)}s |\n`;
  }
  
  const mdReportPath = path.join(resultsDir, 'agent_eval_report.md');
  fs.writeFileSync(mdReportPath, mdReport);
  console.log(`[Harness] Markdown report written to: ${mdReportPath}`);
}

main().catch(err => {
  console.error('[Harness] Critical error in harness main:', err);
  process.exit(1);
});
