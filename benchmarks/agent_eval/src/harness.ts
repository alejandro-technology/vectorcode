import * as dotenv from 'dotenv';
import * as fs from 'fs';
import * as path from 'path';
import { getTasksForCorpus } from './tasks/index.js';
import { runAgent } from './agents.js';
import { VectorCodeProvider } from './tools/vectorcode.js';
import { TraditionalProvider } from './tools/traditional.js';
import { ToolProvider } from './tools/types.js';
import { parseCacheMode, getGitSha, isGitDirty } from './cache.js';
import { judge } from './judge.js';
import { CorpusManager } from './corpus.js';
import { latinSquareOrder, alternateArmOrder } from './randomization.js';
import { analyzeExperiment } from './analysis.js';
import { TrialResult, ExperimentReport, ExperimentConfig, JudgeResult } from './types.js';

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

// ── CLI Parsing ───────────────────────────────────────────────────────────

interface CLIArgs {
  cacheMode: string;
  taskIdFilter: string[] | null;
  modelsToRun: string[];
  armsToRun: ('vectorcode' | 'traditional')[];
  corpora: string[];
  repetitions: number;
  timeoutMs: number;
}

function parseCLIArgs(args: string[]): CLIArgs {
  const cacheMode = parseCacheMode(args);

  // Existing flags
  const taskIdArg = args.find(a => a.startsWith('--task='))?.split('=')[1];
  const modelArg = args.find(a => a.startsWith('--model='))?.split('=')[1];
  const armArg = args.find(a => a.startsWith('--arm='))?.split('=')[1];

  // New flags
  const corpusArg = args.find(a => a.startsWith('--corpus='))?.split('=')[1] || 'mock-mini';
  const repetitionsArg = args.find(a => a.startsWith('--repetitions='))?.split('=')[1];
  const timeoutArg = args.find(a => a.startsWith('--timeout='))?.split('=')[1];

  // Validate corpus
  const validCorpora = ['mock-mini', 'mini', 'vectorcode', 'all'];
  if (!validCorpora.includes(corpusArg)) {
    console.error(`[Harness] Error: Invalid corpus '${corpusArg}'. Must be one of: ${validCorpora.join(', ')}`);
    process.exit(1);
  }

  // Expand 'all' to the three corpora
  const corpora = corpusArg === 'all'
    ? ['mock-mini', 'mini', 'vectorcode']
    : [corpusArg];

  // Validate repetitions
  const repetitions = repetitionsArg ? parseInt(repetitionsArg, 10) : 1;
  if (isNaN(repetitions) || repetitions < 1) {
    console.error(`[Harness] Error: --repetitions must be ≥ 1, got '${repetitionsArg}'`);
    process.exit(1);
  }

  // Validate timeout
  const timeoutMs = timeoutArg ? parseInt(timeoutArg, 10) : 120000;
  if (isNaN(timeoutMs) || timeoutMs <= 0) {
    console.error(`[Harness] Error: --timeout must be > 0, got '${timeoutArg}'`);
    process.exit(1);
  }

  const taskIdFilter = taskIdArg ? taskIdArg.split(',') : null;
  const modelsToRun = modelArg ? modelArg.split(',') : ['mimo-v2.5', 'minimax-m3', 'deepseek-v4-flash'];
  const armsToRun = (armArg ? armArg.split(',') : ['vectorcode', 'traditional']) as ('vectorcode' | 'traditional')[];

  return {
    cacheMode,
    taskIdFilter,
    modelsToRun,
    armsToRun,
    corpora,
    repetitions,
    timeoutMs,
  };
}

// ── Main ──────────────────────────────────────────────────────────────────

async function main() {
  const args = process.argv.slice(2);

  // ── Stage 1: Parse CLI args ──────────────────────────────────────────
  const cli = parseCLIArgs(args);
  const { cacheMode, taskIdFilter, modelsToRun, armsToRun, corpora, repetitions, timeoutMs } = cli;

  // Dirty check for live run
  if ((cacheMode === 'live' || cacheMode === 'update-cache') && isGitDirty()) {
    console.error('\n[Harness] Error: Workspace is dirty (has changes in src/, Cargo.toml, etc.).');
    console.error('Please commit or stash your changes before running live cache population.\n');
    process.exit(1);
  }

  const anthropicModels = [
    'minimax-m3', 'minimax-m2.7', 'minimax-m2.5',
    'qwen3.7-max', 'qwen3.7-plus', 'qwen3.6-plus'
  ];

  console.log('===================================================');
  console.log(` Starting VectorCode Agent Evaluation Suite`);
  console.log(` Mode:         ${cacheMode.toUpperCase()}`);
  console.log(` Corpora:      ${corpora.join(', ')}`);
  console.log(` Models:       ${modelsToRun.join(', ')}`);
  console.log(` Arms:         ${armsToRun.join(', ')}`);
  console.log(` Repetitions:  ${repetitions}`);
  console.log(` Timeout:      ${timeoutMs}ms`);
  if (taskIdFilter) console.log(` Task filter:  ${taskIdFilter.join(', ')}`);
  console.log('===================================================');

  const binPath = getBinPath();
  const allTrialResults: TrialResult[] = [];

  // ── Stage 2+3+4+5: Per-corpus loop ───────────────────────────────────
  for (const corpus of corpora) {
    console.log(`\n${'='.repeat(60)}`);
    console.log(` Corpus: ${corpus}`);
    console.log(`${'='.repeat(60)}`);

    // ── Stage 2: Corpus preparation ────────────────────────────────────
    const corpusManager = new CorpusManager();
    let workspaceDir: string;

    if (cacheMode === 'dry-run') {
      // Dry-run: skip actual corpus preparation, just resolve workspace dir
      switch (corpus) {
        case 'mock-mini':
          workspaceDir = path.resolve(process.cwd(), '../../tests/fixtures/mini');
          break;
        case 'mini':
          workspaceDir = path.resolve(process.cwd(), '../../.bench-corpus/mini');
          break;
        case 'vectorcode':
          workspaceDir = path.resolve(process.cwd(), '../../');
          break;
        default:
          workspaceDir = path.resolve(process.cwd(), '../../');
      }
      console.log(`[Harness] Dry-run: skipping corpus preparation for ${corpus}, workspaceDir=${workspaceDir}`);
    } else {
      try {
        workspaceDir = await corpusManager.prepare(corpus);
      } catch (e: any) {
        console.error(`[Harness] Error preparing corpus '${corpus}': ${e.message}`);
        console.error(`[Harness] Skipping corpus '${corpus}' and continuing...`);
        continue;
      }
    }

    // Load tasks for this corpus
    let corpusTasks = getTasksForCorpus(corpus);

    // Apply task filter if provided
    if (taskIdFilter) {
      corpusTasks = corpusTasks.filter(t => taskIdFilter.includes(t.id));
      if (corpusTasks.length === 0) {
        console.warn(`[Harness] No tasks match the filter for corpus '${corpus}'. Skipping.`);
        continue;
      }
    }

    console.log(`[Harness] Tasks for ${corpus}: ${corpusTasks.map(t => t.id).join(', ')}`);

    const config: ExperimentConfig = {
      corpora: [corpus],
      models: modelsToRun,
      arms: armsToRun,
      repetitions,
      maxSteps: 15,
      timeoutMs,
      temperature: 0,
    };

    const corpusTrials: TrialResult[] = [];

    // ── Stage 3: Trial execution ───────────────────────────────────────
    for (const model of modelsToRun) {
      for (let rep = 1; rep <= repetitions; rep++) {
        // Latin square task order for this repetition
        const orderedTasks = latinSquareOrder(corpusTasks, corpus, rep, model);

        // Arm order alternation
        const armOrder = alternateArmOrder(rep);
        // Filter to only requested arms
        const activeArms = armOrder.filter(a => armsToRun.includes(a));

        for (const arm of activeArms) {
          console.log(`\n---------------------------------------------------`);
          console.log(`Model: ${model} | Rep: ${rep}/${repetitions} | Arm: ${arm}`);
          console.log(`Task order: ${orderedTasks.map(t => t.id).join(' → ')}`);
          console.log(`---------------------------------------------------`);

          let provider: 'openai' | 'anthropic' | 'dry-run';
          if (cacheMode === 'dry-run') {
            provider = 'dry-run';
          } else if (anthropicModels.includes(model)) {
            provider = 'anthropic';
          } else {
            provider = 'openai';
          }

          // Instantiate tool provider with workspace directory
          let toolProvider: ToolProvider;
          if (arm === 'vectorcode') {
            toolProvider = new VectorCodeProvider(binPath, workspaceDir);
          } else {
            toolProvider = new TraditionalProvider(workspaceDir);
          }

          try {
            if (cacheMode !== 'dry-run') {
              await toolProvider.initialize();
            }

            for (const task of orderedTasks) {
              const trialStart = Date.now();
              let success = false;
              let correctness = 0.0;
              let error: string | undefined;
              let agentResult: any;
              let judgeResult: JudgeResult | undefined;

              try {
                agentResult = await runAgent(
                  task,
                  {
                    model,
                    provider,
                    arm,
                    cacheMode: cacheMode as any,
                    corpus,
                    repetition: rep,
                    timeoutMs,
                  },
                  toolProvider
                );

                // Write-task guard: only write on vectorcode corpus
                let wroteFile = false;
                const filePath = path.join(workspaceDir, 'src/cli/status_eval.rs');

                try {
                  if (task.id === 'task-status-command' && corpus === 'vectorcode' && cacheMode !== 'dry-run') {
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
                  const rubricPath = path.resolve(process.cwd(), `rubrics/${corpus}/${task.id}.json`);
                  let rubric = { taskId: task.id, corpus, criteria: [] };
                  if (fs.existsSync(rubricPath)) {
                    rubric = JSON.parse(fs.readFileSync(rubricPath, 'utf8'));
                  }

                  judgeResult = await judge(
                    task.id,
                    task.prompt,
                    agentResult.finalAnswer,
                    rubric,
                    cacheMode === 'dry-run' ? 'dry-run-model' : 'mimo-v2.5'
                  );
                  correctness = judgeResult.score;
                  success = correctness >= 0.8; // configurable threshold (default 0.8)
                }

              } catch (err: any) {
                success = false;
                correctness = 0.0;
                error = err.message || 'Unknown runtime error';
                agentResult = {
                  steps: 0,
                  tokens: { input: 0, output: 0, total: 0 },
                  toolCalls: [],
                  finalAnswer: '',
                  timedOut: false,
                };
              }

              const durationMs = Date.now() - trialStart;
              const uniqueTools = new Set(
                (agentResult.toolCalls || []).map((tc: any) => tc.toolName || tc.name)
              ).size;

              const trial: TrialResult = {
                corpus,
                taskId: task.id,
                model,
                arm,
                repetition: rep,
                success,
                correctness,
                steps: agentResult.steps,
                tokens: agentResult.tokens,
                toolCalls: agentResult.toolCalls || [],
                uniqueTools,
                durationMs,
                timedOut: agentResult.timedOut || false,
                error,
                judgeResult,
                workspaceSha: getGitSha(workspaceDir),
                timestamp: new Date().toISOString(),
              };

              corpusTrials.push(trial);
              allTrialResults.push(trial);

              console.log(`  ${task.id}: ${success ? '✅' : '❌'} score=${correctness.toFixed(2)} steps=${agentResult.steps} tokens=${agentResult.tokens.total} ${agentResult.timedOut ? '[TIMEOUT]' : ''}`);
            }
          } finally {
            try {
              await toolProvider.shutdown();
            } catch (e) {
              console.error(`[Harness] Error shutting down tool provider:`, e);
            }
          }
        }
      }
    }

    // ── Stage 4: Statistical analysis ──────────────────────────────────
    if (corpusTrials.length > 0 && repetitions >= 1) {
      console.log(`\n[Harness] Running statistical analysis for corpus: ${corpus}...`);
      const experimentReport: ExperimentReport = {
        config,
        trials: corpusTrials,
        generatedAt: new Date().toISOString(),
      };

      try {
        const analysis = analyzeExperiment(experimentReport);
        console.log(`[Harness] Analysis complete: ${analysis.totalComparisons} comparisons, ${analysis.significantCount} significant (α=${analysis.bonferroniAlpha.toFixed(4)})`);

        // Write per-corpus reports
        const resultsDir = path.resolve(process.cwd(), `results/${corpus}`);
        if (!fs.existsSync(resultsDir)) {
          fs.mkdirSync(resultsDir, { recursive: true });
        }

        // JSON report: raw trial data
        const reportPath = path.join(resultsDir, 'agent_eval_report.json');
        fs.writeFileSync(reportPath, JSON.stringify(experimentReport, null, 2), 'utf8');
        console.log(`[Harness] Trial data written to: ${reportPath}`);

        // JSON report: statistical analysis
        const statsPath = path.join(resultsDir, 'statistical_analysis.json');
        fs.writeFileSync(statsPath, JSON.stringify(analysis, null, 2), 'utf8');
        console.log(`[Harness] Statistical analysis written to: ${statsPath}`);

        // Log hypothesis verdicts
        console.log(`\n[Harness] Hypothesis Verdicts (${corpus}):`);
        for (const h of analysis.summary.hypotheses) {
          console.log(`  ${h.id}: ${h.supported ? '✅ Supported' : '❌ Not supported'} — ${h.evidence}`);
        }
      } catch (e: any) {
        console.error(`[Harness] Error during analysis for corpus '${corpus}': ${e.message}`);
      }
    }

    // ── Stage 5: Cleanup ───────────────────────────────────────────────
    if (cacheMode !== 'dry-run') {
      try {
        await corpusManager.cleanup();
      } catch (e: any) {
        console.warn(`[Harness] Corpus cleanup warning: ${e.message}`);
      }
    }
  }

  // ── Aggregate results (if multiple corpora) ────────────────────────────
  if (corpora.length > 1 && allTrialResults.length > 0) {
    console.log(`\n${'='.repeat(60)}`);
    console.log(` Aggregate Report (${corpora.length} corpora, ${allTrialResults.length} trials)`);
    console.log(`${'='.repeat(60)}`);

    const resultsDir = path.resolve(process.cwd(), 'results');
    if (!fs.existsSync(resultsDir)) {
      fs.mkdirSync(resultsDir, { recursive: true });
    }

    // Aggregate JSON
    const aggregateReport = {
      corpora,
      totalTrials: allTrialResults.length,
      generatedAt: new Date().toISOString(),
    };

    const aggPath = path.join(resultsDir, 'aggregate_report.json');
    fs.writeFileSync(aggPath, JSON.stringify(aggregateReport, null, 2), 'utf8');
    console.log(`[Harness] Aggregate report written to: ${aggPath}`);
  }

  // ── Legacy flat report (backward compat) ───────────────────────────────
  if (allTrialResults.length > 0) {
    const resultsDir = path.resolve(process.cwd(), 'results');
    if (!fs.existsSync(resultsDir)) {
      fs.mkdirSync(resultsDir, { recursive: true });
    }

    const reportPath = path.join(resultsDir, 'agent_eval_report.json');
    fs.writeFileSync(reportPath, JSON.stringify(allTrialResults, null, 2), 'utf8');
    console.log(`\n[Harness] Flat report written to: ${reportPath}`);

    // Generate markdown report
    const gitSha = getGitSha();
    let mdReport = `# Agent Evaluation Report\n\n`;
    mdReport += `- **Workspace SHA**: \`${gitSha}\`\n`;
    mdReport += `- **Date**: ${new Date().toISOString()}\n`;
    mdReport += `- **Mode**: \`${cacheMode}\`\n`;
    mdReport += `- **Corpora**: ${corpora.join(', ')}\n`;
    mdReport += `- **Repetitions**: ${repetitions}\n\n`;

    mdReport += `## Summary Table\n\n`;
    mdReport += `| Corpus | Model | Task | Arm | Rep | Success | Correctness | Steps | Tokens | Duration |\n`;
    mdReport += `| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n`;

    for (const r of allTrialResults) {
      const successEmoji = r.success ? '✅' : '❌';
      const duration = (r.durationMs / 1000).toFixed(2) + 's';
      mdReport += `| ${r.corpus} | ${r.model} | ${r.taskId} | ${r.arm} | ${r.repetition} | ${successEmoji} | ${r.correctness.toFixed(2)} | ${r.steps} | ${r.tokens.total} | ${duration} |\n`;
    }

    const mdReportPath = path.join(resultsDir, 'agent_eval_report.md');
    fs.writeFileSync(mdReportPath, mdReport, 'utf8');
    console.log(`[Harness] Markdown report written to: ${mdReportPath}`);
  }

  console.log(`\n[Harness] Analysis complete. All done.`);
}

main().catch(err => {
  console.error('[Harness] Critical error in harness main:', err);
  process.exit(1);
});
