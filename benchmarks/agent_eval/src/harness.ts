import * as dotenv from 'dotenv';
import * as fs from 'fs';
import * as path from 'path';
import { tasks } from './tasks.js';
import { runAgent } from './agents.js';
import { VectorCodeProvider } from './tools/vectorcode.js';
import { TraditionalProvider } from './tools/traditional.js';
import { ToolProvider } from './tools/types.js';
import { parseCacheMode, getGitSha, isGitDirty } from './cache.js';
import { judge } from './judge.js';

dotenv.config();

function getBinPath(): string {
  if (process.env.VECTORCODE_BIN) {
    return path.resolve(process.env.VECTORCODE_BIN);
  }
  // Default to workspace debug binary
  return path.resolve('../../target/debug/vectorcode');
}

function extractRustCode(answer: string): string {
  const match = answer.match(/```rust\s*([\s\S]*?)```/) || answer.match(/```\s*([\s\S]*?)```/);
  if (match) {
    return match[1].trim();
  }
  return answer.trim();
}

async function main() {
  const args = process.argv.slice(2);
  const cacheMode = parseCacheMode(args);

  // Dirty check for live run
  if ((cacheMode === 'live' || cacheMode === 'update-cache') && isGitDirty()) {
    console.error('\n[Harness] Error: Workspace is dirty (has changes in src/, Cargo.toml, etc.).');
    console.error('Please commit or stash your changes before running live cache population.\n');
    process.exit(1);
  }

  // Parse arguments or set defaults
  const taskIdArg = args.find(a => a.startsWith('--task='))?.split('=')[1];
  const modelArg = args.find(a => a.startsWith('--model='))?.split('=')[1];
  const armArg = args.find(a => a.startsWith('--arm='))?.split('=')[1];

  const tasksToRun = taskIdArg ? taskIdArg.split(',') : tasks.map(t => t.id);
  const modelsToRun = modelArg ? modelArg.split(',') : ['mimo-v2.5', 'minimax-m3', 'deepseek-v4-flash'];
  const armsToRun = (armArg ? armArg.split(',') : ['vectorcode', 'traditional']) as ('vectorcode' | 'traditional')[];

  const anthropicModels = [
    'minimax-m3', 'minimax-m2.7', 'minimax-m2.5',
    'qwen3.7-max', 'qwen3.7-plus', 'qwen3.6-plus'
  ];

  console.log('===================================================');
  console.log(` Starting VectorCode Agent Evaluation Suite`);
  console.log(` Mode:   ${cacheMode.toUpperCase()}`);
  console.log(` Models: ${modelsToRun.join(', ')}`);
  console.log(` Tasks:  ${tasksToRun.join(', ')}`);
  console.log(` Arms:   ${armsToRun.join(', ')}`);
  console.log('===================================================');

  const results: any[] = [];
  const binPath = getBinPath();

  for (const model of modelsToRun) {
    for (const taskId of tasksToRun) {
      const task = tasks.find(t => t.id === taskId);
      if (!task) {
        console.error(`[Harness] Task with ID ${taskId} not found. Skipping.`);
        continue;
      }

      for (const arm of armsToRun) {
        console.log(`\n---------------------------------------------------`);
        console.log(`Running: Model: ${model} | Task: ${taskId} | Arm: ${arm}`);
        console.log(`---------------------------------------------------`);

        let provider: 'openai' | 'anthropic' | 'dry-run';
        if (cacheMode === 'dry-run') {
          provider = 'dry-run';
        } else if (anthropicModels.includes(model)) {
          provider = 'anthropic';
        } else {
          provider = 'openai';
        }

        // Instantiate appropriate tool provider
        const workspaceDir = path.resolve('../../');
        let toolProvider: ToolProvider;
        if (arm === 'vectorcode') {
          toolProvider = new VectorCodeProvider(binPath, workspaceDir);
        } else {
          toolProvider = new TraditionalProvider(workspaceDir);
        }

        const startMs = Date.now();
        let success = false;
        let correctness = 0.0;
        let error: string | undefined;
        let agentResult: any;
        let judgeResult: any;

        try {
          if (cacheMode !== 'dry-run') {
            await toolProvider.initialize();
          }

          agentResult = await runAgent(
            task,
            { model, provider, arm, cacheMode, corpus: task.corpus },
            toolProvider
          );

          // Task Verification & Safe cleanup block (Task 6.2)
          let wroteFile = false;
          const filePath = path.join(workspaceDir, 'src/cli/status_eval.rs');

          try {
            if (taskId === 'task-status-command' && cacheMode !== 'dry-run') {
              const code = extractRustCode(agentResult.finalAnswer);
              fs.writeFileSync(filePath, code, 'utf8');
              wroteFile = true;
            }

            const verification = await task.verify(workspaceDir);
            success = verification.success;
            if (!success) {
              error = verification.error || 'Task verification failed';
            }
          } finally {
            if (wroteFile) {
              if (fs.existsSync(filePath)) {
                try {
                  fs.unlinkSync(filePath);
                } catch (e) {
                  console.error(`[Harness] Failed to clean up status_eval.rs: ${e}`);
                }
              }
            }
          }

          // Evaluate correctness
          if (task.type === 'write') {
            correctness = success ? 1.0 : 0.0;
          } else {
            // Read-only task, use LLM-as-Judge
            const rubricPath = path.resolve(process.cwd(), `rubrics/${task.corpus}/${taskId}.json`);
            let rubric = { taskId, criteria: [] };
            if (fs.existsSync(rubricPath)) {
              rubric = JSON.parse(fs.readFileSync(rubricPath, 'utf8'));
            }

            judgeResult = await judge(
              taskId,
              task.prompt,
              agentResult.finalAnswer,
              rubric,
              cacheMode === 'dry-run' ? 'dry-run-model' : 'mimo-v2.5'
            );
            correctness = judgeResult.score;
            success = correctness >= 0.8; // threshold of 0.8 for success
          }

        } catch (err: any) {
          success = false;
          correctness = 0.0;
          error = err.message || 'Unknown runtime error';
          agentResult = {
            steps: 0,
            tokens: { input: 0, output: 0, total: 0 },
            toolCalls: [],
            finalAnswer: ''
          };
        } finally {
          try {
            await toolProvider.shutdown();
          } catch (e) {
            console.error(`[Harness] Error shutting down tool provider:`, e);
          }
        }

        const durationMs = Date.now() - startMs;
        const record = {
          taskId,
          model,
          provider,
          arm,
          success,
          correctness,
          steps: agentResult.steps,
          tokens: agentResult.tokens,
          toolCalls: agentResult.toolCalls,
          error,
          durationMs,
          judgeResult
        };

        results.push(record);

        console.log(`Result:     ${success ? '✅ SUCCESS' : '❌ FAILED'}`);
        console.log(`Score:      ${correctness.toFixed(2)}`);
        console.log(`Steps:      ${agentResult.steps}`);
        console.log(`Tokens:     In: ${agentResult.tokens.input} | Out: ${agentResult.tokens.output} | Total: ${agentResult.tokens.total}`);
        console.log(`Duration:   ${(durationMs / 1000).toFixed(2)}s`);
        if (error) {
          console.log(`Error:      ${error}`);
        }
      }
    }
  }

  // Save report
  const resultsDir = path.resolve(process.cwd(), 'results');
  if (!fs.existsSync(resultsDir)) {
    fs.mkdirSync(resultsDir, { recursive: true });
  }

  const reportPath = path.join(resultsDir, 'agent_eval_report.json');
  fs.writeFileSync(reportPath, JSON.stringify(results, null, 2), 'utf8');
  console.log(`\n[Harness] Report written to: ${reportPath}`);

  // Generate markdown report
  const gitSha = getGitSha();
  let mdReport = `# Agent Evaluation Report\n\n`;
  mdReport += `- **Workspace SHA**: \`${gitSha}\`\n`;
  mdReport += `- **Date**: ${new Date().toISOString()}\n`;
  mdReport += `- **Mode**: \`${cacheMode}\`\n\n`;

  mdReport += `## Summary Table\n\n`;
  mdReport += `| Model | Task | Arm | Success | Correctness | Steps | Tokens | Tool Calls | Duration |\n`;
  mdReport += `| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n`;

  for (const r of results) {
    const successEmoji = r.success ? '✅' : '❌';
    const duration = (r.durationMs / 1000).toFixed(2) + 's';
    mdReport += `| ${r.model} | ${r.taskId} | ${r.arm} | ${successEmoji} | ${r.correctness.toFixed(2)} | ${r.steps} | ${r.tokens.total} | ${r.toolCalls.length} | ${duration} |\n`;
  }

  const mdReportPath = path.join(resultsDir, 'agent_eval_report.md');
  fs.writeFileSync(mdReportPath, mdReport, 'utf8');
  console.log(`[Harness] Markdown report written to: ${mdReportPath}`);
}

main().catch(err => {
  console.error('[Harness] Critical error in harness main:', err);
  process.exit(1);
});

