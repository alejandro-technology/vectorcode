import * as fs from 'fs';
import * as path from 'path';
import * as ss from 'simple-statistics';
import {
  ExperimentReport,
  AnalysisReport,
  TrialResult,
  StatisticalResult,
} from './types.js';
import { allTasks } from './tasks/index.js';

// ── Number formatting ────────────────────────────────────────────────────

function fmt2(n: number): string {
  return n.toFixed(2);
}

function fmt3(n: number): string {
  if (n < 0.001 && n > 0) return n.toExponential(2);
  return n.toFixed(3);
}

function fmtPct(n: number): string {
  return Math.round(n * 100) + '%';
}

function fmtDuration(ms: number): string {
  return (ms / 1000).toFixed(1) + 's';
}

function fmtTokens(n: number): string {
  return Math.round(n).toLocaleString('en-US');
}

function stars(difficulty: number): string {
  const d = Math.max(0, Math.min(5, difficulty));
  return '★'.repeat(d) + '☆'.repeat(5 - d);
}

function classifyEffectLabel(d: number): string {
  const abs = Math.abs(d);
  if (abs < 0.2) return 'negligible';
  if (abs < 0.5) return 'small';
  if (abs < 0.8) return 'medium';
  return 'large';
}

// ── Bootstrap CI (for report display only) ───────────────────────────────

function bootstrapCI(values: number[], B: number = 500, alpha: number = 0.05): [number, number] {
  if (values.length === 0) return [0, 0];
  if (values.length === 1) return [values[0], values[0]];

  const means: number[] = [];
  for (let b = 0; b < B; b++) {
    const sample: number[] = [];
    for (let i = 0; i < values.length; i++) {
      sample.push(values[Math.floor(Math.random() * values.length)]);
    }
    means.push(ss.mean(sample));
  }

  means.sort((a, b) => a - b);
  const lo = means[Math.floor(B * alpha / 2)];
  const hi = means[Math.floor(B * (1 - alpha / 2))];
  return [lo, hi];
}

// ── Corpus metadata ──────────────────────────────────────────────────────

interface CorpusMetadata {
  fileCount: number;
  languages: string[];
}

const EXT_TO_LANG: Record<string, string> = {
  '.rs': 'Rust',
  '.ts': 'TypeScript',
  '.js': 'JavaScript',
  '.py': 'Python',
  '.toml': 'TOML',
  '.json': 'JSON',
  '.md': 'Markdown',
};

function detectCorpusMetadata(workspaceDir: string): CorpusMetadata {
  let fileCount = 0;
  const extSet = new Set<string>();

  try {
    walkDir(workspaceDir, (filePath: string) => {
      const ext = path.extname(filePath).toLowerCase();
      if (EXT_TO_LANG[ext]) {
        fileCount++;
        extSet.add(EXT_TO_LANG[ext]);
      }
    });
  } catch {
    // If we can't read the directory, return defaults
  }

  return {
    fileCount,
    languages: [...extSet].sort() || ['Unknown'],
  };
}

function walkDir(dir: string, callback: (filePath: string) => void): void {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      // Skip common non-source directories
      if (['node_modules', '.git', 'target', 'dist', '.vectorcode'].includes(entry.name)) continue;
      walkDir(fullPath, callback);
    } else if (entry.isFile()) {
      callback(fullPath);
    }
  }
}

// ── Task-level aggregation ───────────────────────────────────────────────

interface TaskArmStats {
  taskId: string;
  difficulty: number;
  arm: 'vectorcode' | 'traditional';
  mcs: number;
  ciHalf: number;
  tsr: number;
  meanTokens: number;
  meanSteps: number;
  meanDurationMs: number;
}

interface TaskEfficiencyStats {
  taskId: string;
  difficulty: number;
  ter: number;
  ser: number;
  deltaMCS: number;
  cohensD: number;
  pValue: number;
  significant: boolean;
}

function getTaskDifficulty(taskId: string): number {
  const task = allTasks.find(t => t.id === taskId);
  return task?.difficulty ?? 0;
}

function computeTaskArmStats(trials: TrialResult[]): TaskArmStats[] {
  const map = new Map<string, TrialResult[]>();
  for (const t of trials) {
    const key = `${t.taskId}|${t.arm}`;
    if (!map.has(key)) map.set(key, []);
    map.get(key)!.push(t);
  }

  const stats: TaskArmStats[] = [];
  for (const [key, group] of map) {
    const [taskId, arm] = key.split('|');
    const correctness = group.map(t => t.correctness);
    const ci = bootstrapCI(correctness);

    stats.push({
      taskId,
      difficulty: getTaskDifficulty(taskId),
      arm: arm as 'vectorcode' | 'traditional',
      mcs: ss.mean(correctness),
      ciHalf: (ci[1] - ci[0]) / 2,
      tsr: group.filter(t => t.success).length / group.length,
      meanTokens: ss.mean(group.map(t => t.tokens.total)),
      meanSteps: ss.mean(group.map(t => t.steps)),
      meanDurationMs: ss.mean(group.map(t => t.durationMs)),
    });
  }

  stats.sort((a, b) => {
    if (a.taskId !== b.taskId) return a.taskId.localeCompare(b.taskId);
    return a.arm === 'vectorcode' ? -1 : 1;
  });

  return stats;
}

function computeTaskEfficiencyStats(
  trials: TrialResult[],
  analysis: AnalysisReport,
): TaskEfficiencyStats[] {
  // Group by (taskId, arm)
  const vcMap = new Map<string, TrialResult[]>();
  const tradMap = new Map<string, TrialResult[]>();
  for (const t of trials) {
    const m = t.arm === 'vectorcode' ? vcMap : tradMap;
    if (!m.has(t.taskId)) m.set(t.taskId, []);
    m.get(t.taskId)!.push(t);
  }

  // Index analysis results by taskId (correctness metric)
  const analysisByTask = new Map<string, StatisticalResult[]>();
  for (const r of analysis.results) {
    if (r.metric !== 'correctness') continue;
    if (!analysisByTask.has(r.taskId)) analysisByTask.set(r.taskId, []);
    analysisByTask.get(r.taskId)!.push(r);
  }

  const taskIds = new Set([...vcMap.keys(), ...tradMap.keys()]);
  const stats: TaskEfficiencyStats[] = [];

  for (const taskId of taskIds) {
    const vcTrials = vcMap.get(taskId) || [];
    const tradTrials = tradMap.get(taskId) || [];
    if (vcTrials.length === 0 || tradTrials.length === 0) continue;

    const vcTokens = ss.mean(vcTrials.map(t => t.tokens.total));
    const tradTokens = ss.mean(tradTrials.map(t => t.tokens.total));
    const ter = vcTokens > 0 ? tradTokens / vcTokens : Infinity;

    const vcSteps = ss.mean(vcTrials.map(t => t.steps));
    const tradSteps = ss.mean(tradTrials.map(t => t.steps));
    const ser = vcSteps > 0 ? tradSteps / vcSteps : Infinity;

    const vcMCS = ss.mean(vcTrials.map(t => t.correctness));
    const tradMCS = ss.mean(tradTrials.map(t => t.correctness));
    const deltaMCS = vcMCS - tradMCS;

    const taskResults = analysisByTask.get(taskId) || [];
    const d = taskResults.length > 0 ? ss.mean(taskResults.map(r => r.effectSize)) : 0;
    const pValue = taskResults.length > 0 ? ss.mean(taskResults.map(r => r.pValue)) : 1.0;
    const significant = taskResults.some(r => r.significant);

    stats.push({
      taskId,
      difficulty: getTaskDifficulty(taskId),
      ter,
      ser,
      deltaMCS,
      cohensD: d,
      pValue,
      significant,
    });
  }

  stats.sort((a, b) => a.taskId.localeCompare(b.taskId));
  return stats;
}

// ── Hypothesis names ─────────────────────────────────────────────────────

const HYPOTHESIS_NAMES: Record<string, string> = {
  H1: 'Token efficiency',
  H2: 'Step efficiency',
  H3: 'Cross-module advantage',
  H4: 'Lower variance',
};

// ── Per-corpus Markdown report ───────────────────────────────────────────

export function generateCorpusMarkdown(
  report: ExperimentReport,
  analysis: AnalysisReport,
  workspaceDir: string,
  cacheMode: string,
): string {
  const corpus = report.config.corpora[0];
  const metadata = detectCorpusMetadata(workspaceDir);
  const workspaceSha = report.trials.length > 0 ? report.trials[0].workspaceSha : 'unknown';

  let md = '';

  // Header
  md += `# Agent Evaluation Report — ${corpus}\n\n`;
  md += `- **Corpus**: ${corpus} (${metadata.fileCount} files, ${metadata.languages.join(', ')})\n`;
  md += `- **Workspace SHA**: \`${workspaceSha}\`\n`;
  md += `- **Date**: ${report.generatedAt}\n`;
  md += `- **Mode**: \`${cacheMode}\`\n`;
  md += `- **Models**: ${report.config.models.join(', ')}\n`;
  md += `- **Repetitions**: R=${report.config.repetitions}\n\n`;

  // Summary table
  const taskArmStats = computeTaskArmStats(report.trials);

  md += `## Summary\n\n`;
  md += `| Task | ★ | Arm | MCS ± CI95 | TSR | Mean Tokens | Mean Steps | Mean Duration |\n`;
  md += `|------|---|-----|-----------|-----|-------------|------------|---------------|\n`;

  for (const s of taskArmStats) {
    const armLabel = s.arm === 'vectorcode' ? 'VC' : 'TR';
    md += `| ${s.taskId} | ${stars(s.difficulty)} | ${armLabel} `;
    md += `| ${fmt2(s.mcs)} ± ${fmt2(s.ciHalf)} `;
    md += `| ${fmtPct(s.tsr)} `;
    md += `| ${fmtTokens(s.meanTokens)} `;
    md += `| ${fmt2(s.meanSteps)} `;
    md += `| ${fmtDuration(s.meanDurationMs)} |\n`;
  }

  // Efficiency ratios table
  const effStats = computeTaskEfficiencyStats(report.trials, analysis);

  md += `\n## Efficiency Ratios\n\n`;
  md += `| Task | ★ | TER | SER | ΔMCS | Cohen's d | Significant? |\n`;
  md += `|------|---|-----|-----|------|-----------|-------------|\n`;

  for (const s of effStats) {
    const sigLabel = s.significant ? `✅ p=${fmt3(s.pValue)}` : `❌ p=${fmt3(s.pValue)}`;
    const dLabel = `${fmt2(s.cohensD)} (${classifyEffectLabel(s.cohensD)})`;
    md += `| ${s.taskId} | ${stars(s.difficulty)} `;
    md += `| ${fmt2(s.ter)} `;
    md += `| ${fmt2(s.ser)} `;
    md += `| ${s.deltaMCS >= 0 ? '+' : ''}${fmt2(s.deltaMCS)} `;
    md += `| ${dLabel} `;
    md += `| ${sigLabel} |\n`;
  }

  // Hypothesis verdicts
  md += `\n## Hypothesis Verdicts\n\n`;
  md += `| ID | Hypothesis | Verdict | Evidence |\n`;
  md += `|----|-----------|---------|----------|\n`;

  for (const h of analysis.summary.hypotheses) {
    const verdict = h.supported ? '✅ Supported' : '❌ Not supported';
    const name = HYPOTHESIS_NAMES[h.id] || h.id;
    md += `| ${h.id} | ${name} | ${verdict} | ${h.evidence} |\n`;
  }

  return md;
}

// ── Aggregate Markdown report ────────────────────────────────────────────

export function generateAggregateMarkdown(
  reports: Map<string, { report: ExperimentReport; analysis: AnalysisReport }>,
): string {
  let md = '';

  md += `# Aggregate Agent Evaluation Report\n\n`;
  md += `- **Corpora**: ${[...reports.keys()].join(', ')}\n`;
  md += `- **Date**: ${new Date().toISOString()}\n\n`;

  // Cross-corpus summary table
  md += `## Cross-Corpus Summary\n\n`;
  md += `| Corpus | TER | SER | Hypotheses Supported | Total Trials |\n`;
  md += `|--------|-----|-----|---------------------|-------------|\n`;

  for (const [corpus, { report, analysis }] of reports) {
    const ter = analysis.summary.ter[corpus];
    const ser = analysis.summary.ser[corpus];
    const hSupported = analysis.summary.hypotheses.filter(h => h.supported).length;
    const totalTrials = report.trials.length;

    md += `| ${corpus} `;
    md += `| ${ter != null && isFinite(ter) ? fmt2(ter) : 'N/A'} `;
    md += `| ${ser != null && isFinite(ser) ? fmt2(ser) : 'N/A'} `;
    md += `| ${hSupported}/${analysis.summary.hypotheses.length} `;
    md += `| ${totalTrials} |\n`;
  }

  // Combined hypothesis verdicts
  md += `\n## Combined Hypothesis Verdicts\n\n`;
  md += `| ID | Hypothesis | Verdicts | Combined Evidence |\n`;
  md += `|----|-----------|----------|------------------|\n`;

  const allHypothesisIds = new Set<string>();
  for (const { analysis } of reports.values()) {
    for (const h of analysis.summary.hypotheses) {
      allHypothesisIds.add(h.id);
    }
  }

  for (const hId of [...allHypothesisIds].sort()) {
    const name = HYPOTHESIS_NAMES[hId] || hId;
    const verdicts: string[] = [];
    const evidences: string[] = [];

    for (const [corpus, { analysis }] of reports) {
      const h = analysis.summary.hypotheses.find(hv => hv.id === hId);
      if (h) {
        verdicts.push(`${corpus}: ${h.supported ? '✅' : '❌'}`);
        evidences.push(h.evidence);
      }
    }

    md += `| ${hId} | ${name} | ${verdicts.join(', ')} | ${evidences.join('; ')} |\n`;
  }

  return md;
}

// ── Main entry points ────────────────────────────────────────────────────

/**
 * Generate all per-corpus reports (JSON + Markdown).
 */
export function generateReports(
  report: ExperimentReport,
  analysis: AnalysisReport,
  outputDir: string,
  workspaceDir: string,
  cacheMode: string,
): void {
  const corpus = report.config.corpora[0];
  const corpusDir = path.join(outputDir, corpus);

  if (!fs.existsSync(corpusDir)) {
    fs.mkdirSync(corpusDir, { recursive: true });
  }

  // JSON: raw trial data (ExperimentReport)
  const reportPath = path.join(corpusDir, 'agent_eval_report.json');
  fs.writeFileSync(reportPath, JSON.stringify(report, null, 2), 'utf8');
  console.log(`[Report] Trial data → ${reportPath}`);

  // JSON: statistical analysis (AnalysisReport)
  const statsPath = path.join(corpusDir, 'statistical_analysis.json');
  fs.writeFileSync(statsPath, JSON.stringify(analysis, null, 2), 'utf8');
  console.log(`[Report] Statistical analysis → ${statsPath}`);

  // Markdown: per-corpus report
  const mdContent = generateCorpusMarkdown(report, analysis, workspaceDir, cacheMode);
  const mdPath = path.join(corpusDir, 'agent_eval_report.md');
  fs.writeFileSync(mdPath, mdContent, 'utf8');
  console.log(`[Report] Markdown report → ${mdPath}`);
}

/**
 * Generate aggregate cross-corpus report.
 */
export function generateAggregateReport(
  reports: Map<string, { report: ExperimentReport; analysis: AnalysisReport }>,
  outputDir: string,
): void {
  if (!fs.existsSync(outputDir)) {
    fs.mkdirSync(outputDir, { recursive: true });
  }

  const mdContent = generateAggregateMarkdown(reports);
  const mdPath = path.join(outputDir, 'aggregate_report.md');
  fs.writeFileSync(mdPath, mdContent, 'utf8');
  console.log(`[Report] Aggregate report → ${mdPath}`);
}
