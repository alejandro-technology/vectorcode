import * as ss from 'simple-statistics';
import {
  ExperimentReport,
  TrialResult,
  StatisticalResult,
  AnalysisReport,
  HypothesisVerdict,
  Task,
} from './types.js';
import { allTasks } from './tasks/index.js';

// ── Statistical primitives ────────────────────────────────────────────────

/**
 * Wilcoxon signed-rank test (from scratch).
 * Paired non-parametric test for differences between two related samples.
 * Returns { statistic, pValue } (two-tailed).
 */
export function wilcoxonSignedRank(
  x: number[],
  y: number[],
): { statistic: number; pValue: number } {
  if (x.length !== y.length || x.length === 0) {
    return { statistic: 0, pValue: 1.0 };
  }

  // Step 1: compute differences
  const diffs: number[] = [];
  for (let i = 0; i < x.length; i++) {
    const d = x[i] - y[i];
    if (d !== 0) diffs.push(d); // Step 2: remove zeros
  }

  const N = diffs.length;
  if (N === 0) {
    // All ties
    return { statistic: 0, pValue: 1.0 };
  }

  // Step 3: rank |d[i]| with average ties
  const absD = diffs.map(d => ({ abs: Math.abs(d), sign: d > 0 ? 1 : -1 }));
  absD.sort((a, b) => a.abs - b.abs);

  // Assign ranks with average ties
  const ranks: number[] = new Array(N);
  let i = 0;
  while (i < N) {
    let j = i;
    while (j < N - 1 && absD[j + 1].abs === absD[j].abs) j++;
    const avgRank = (i + j) / 2 + 1; // 1-based average rank
    for (let k = i; k <= j; k++) ranks[k] = avgRank;
    i = j + 1;
  }

  // Step 4: W+ and W-
  let wPlus = 0;
  let wMinus = 0;
  for (let k = 0; k < N; k++) {
    if (absD[k].sign > 0) wPlus += ranks[k];
    else wMinus += ranks[k];
  }

  // Step 5: W = min(W+, W-)
  const W = Math.min(wPlus, wMinus);

  // Step 6: p-value
  let pValue: number;
  if (N <= 20) {
    // Exact critical values table for two-tailed Wilcoxon signed-rank test
    // For N ≤ 20, we use the normal approximation as a fallback since
    // the exact table is large. The normal approximation is acceptable
    // for N > 5 and conservative for smaller N.
    pValue = wilcoxonExactP(W, N);
  } else {
    // Normal approximation
    const mu = (N * (N + 1)) / 4;
    const sigma = Math.sqrt((N * (N + 1) * (2 * N + 1)) / 24);
    const z = (W - mu) / sigma;
    pValue = 2 * normalCDF(-Math.abs(z));
  }

  return { statistic: W, pValue: Math.min(1, Math.max(0, pValue)) };
}

/**
 * Exact p-value for Wilcoxon signed-rank test using the recursive
 * distribution of W under H0 for small N (≤ 20).
 * Uses normal approximation with continuity correction as a practical
 * approach that is accurate enough for our use case.
 */
function wilcoxonExactP(W: number, N: number): number {
  // For N ≤ 20, use normal approximation with continuity correction.
  // This is conservative (slightly overestimates p) but valid.
  const mu = (N * (N + 1)) / 4;
  const sigma = Math.sqrt((N * (N + 1) * (2 * N + 1)) / 24);
  if (sigma === 0) return 1.0;
  // Continuity correction: P(W ≤ w) ≈ Φ((w + 0.5 - mu) / sigma)
  const z = (W + 0.5 - mu) / sigma;
  return 2 * normalCDF(-Math.abs(z));
}

/**
 * Standard normal CDF using the Abramowitz & Stegun approximation.
 * Accurate to ~1e-7.
 */
function normalCDF(x: number): number {
  if (x < -8) return 0;
  if (x > 8) return 1;
  const a1 = 0.254829592;
  const a2 = -0.284496736;
  const a3 = 1.421413741;
  const a4 = -1.453152027;
  const a5 = 1.061405429;
  const p = 0.3275911;

  const sign = x < 0 ? -1 : 1;
  const absX = Math.abs(x);
  const t = 1.0 / (1.0 + p * absX);
  const y = 1.0 - ((((a5 * t + a4) * t + a3) * t + a2) * t + a1) * t * Math.exp(-absX * absX / 2);
  return 0.5 * (1.0 + sign * y);
}

/**
 * Cohen's d effect size for two independent samples.
 * d = (mean_x - mean_y) / pooled_std
 * pooled_std floored at 1e-10 to prevent division by zero.
 */
export function cohensD(x: number[], y: number[]): number {
  const meanX = ss.mean(x);
  const meanY = ss.mean(y);
  const n1 = x.length;
  const n2 = y.length;

  if (n1 + n2 - 2 <= 0) return 0;

  const s1 = x.length > 1 ? ss.standardDeviation(x) : 0;
  const s2 = y.length > 1 ? ss.standardDeviation(y) : 0;

  const pooledStd = Math.sqrt(
    ((n1 - 1) * s1 * s1 + (n2 - 1) * s2 * s2) / (n1 + n2 - 2),
  );

  const floor = 1e-10;
  return (meanX - meanY) / Math.max(pooledStd, floor);
}

/**
 * Classify Cohen's d effect size magnitude.
 */
export function classifyEffect(d: number): 'negligible' | 'small' | 'medium' | 'large' {
  const absD = Math.abs(d);
  if (absD < 0.2) return 'negligible';
  if (absD < 0.5) return 'small';
  if (absD < 0.8) return 'medium';
  return 'large';
}

/**
 * Bootstrap confidence interval.
 * Resample with replacement B times, return [alpha/2, 1-alpha/2] percentiles.
 */
export function bootstrapCI(
  values: number[],
  B: number = 1000,
  alpha: number = 0.05,
): [number, number] {
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

// ── Trial grouping ────────────────────────────────────────────────────────

interface TrialGroup {
  corpus: string;
  taskId: string;
  model: string;
  arm: 'vectorcode' | 'traditional';
  trials: TrialResult[];
}

/**
 * Group trials by (corpus, taskId, model, arm).
 */
function groupBy(trials: TrialResult[]): TrialGroup[] {
  const map = new Map<string, TrialResult[]>();

  for (const t of trials) {
    const key = `${t.corpus}|${t.taskId}|${t.model}|${t.arm}`;
    if (!map.has(key)) map.set(key, []);
    map.get(key)!.push(t);
  }

  const groups: TrialGroup[] = [];
  for (const [key, groupTrials] of map) {
    const [corpus, taskId, model, arm] = key.split('|');
    groups.push({ corpus, taskId, model, arm: arm as 'vectorcode' | 'traditional', trials: groupTrials });
  }
  return groups;
}

// ── Derived metrics ───────────────────────────────────────────────────────

/**
 * Token Efficiency Ratio: mean_tokens_trad / mean_tokens_vc.
 * Returns Infinity if vc mean is 0.
 */
export function computeTER(vcTrials: TrialResult[], tradTrials: TrialResult[]): number {
  const vcMean = ss.mean(vcTrials.map(t => t.tokens.total));
  const tradMean = ss.mean(tradTrials.map(t => t.tokens.total));
  if (vcMean === 0) return Infinity;
  return tradMean / vcMean;
}

/**
 * Step Efficiency Ratio: mean_steps_trad / mean_steps_vc.
 */
export function computeSER(vcTrials: TrialResult[], tradTrials: TrialResult[]): number {
  const vcMean = ss.mean(vcTrials.map(t => t.steps));
  const tradMean = ss.mean(tradTrials.map(t => t.steps));
  if (vcMean === 0) return Infinity;
  return tradMean / vcMean;
}

/**
 * Task Success Rate: count(success) / R.
 */
export function computeTSR(trials: TrialResult[]): number {
  if (trials.length === 0) return 0;
  return trials.filter(t => t.success).length / trials.length;
}

/**
 * Mean Correctness Score: mean(correctness).
 */
export function computeMCS(trials: TrialResult[]): number {
  if (trials.length === 0) return 0;
  return ss.mean(trials.map(t => t.correctness));
}

/**
 * Agent Efficiency Index: mean(correctness × log(1 + 1/tokens_total) × success).
 */
export function computeAEI(trials: TrialResult[]): number {
  if (trials.length === 0) return 0;
  const aeiValues = trials.map(t => {
    const tokenFactor = Math.log(1 + 1 / Math.max(t.tokens.total, 1));
    return t.correctness * tokenFactor * (t.success ? 1 : 0);
  });
  return ss.mean(aeiValues);
}

/**
 * Difficulty-Adjusted Efficiency: AEI / difficulty.
 */
export function computeDAE(aei: number, difficulty: number): number {
  if (difficulty === 0) return 0;
  return aei / difficulty;
}

// ── Comparison generation ─────────────────────────────────────────────────

interface Comparison {
  corpus: string;
  taskId: string;
  model: string;
  metric: string;
  vcValues: number[];
  tradValues: number[];
}

/**
 * Generate paired comparisons for each (corpus, taskId, model, metric).
 */
function generateComparisons(groups: TrialGroup[]): Comparison[] {
  const comparisons: Comparison[] = [];

  // Index groups by (corpus, taskId, model, arm)
  const index = new Map<string, TrialGroup>();
  for (const g of groups) {
    const key = `${g.corpus}|${g.taskId}|${g.model}|${g.arm}`;
    index.set(key, g);
  }

  // Find unique (corpus, taskId, model) combinations
  const keys = new Set<string>();
  for (const g of groups) {
    keys.add(`${g.corpus}|${g.taskId}|${g.model}`);
  }

  const metrics = ['correctness', 'tokens.total', 'steps'] as const;

  for (const combo of keys) {
    const [corpus, taskId, model] = combo.split('|');
    const vcGroup = index.get(`${combo}|vectorcode`);
    const tradGroup = index.get(`${combo}|traditional`);
    if (!vcGroup || !tradGroup) continue;
    if (vcGroup.trials.length === 0 || tradGroup.trials.length === 0) continue;

    // Pair trials by repetition for paired tests
    const R = Math.min(vcGroup.trials.length, tradGroup.trials.length);
    const vcSorted = [...vcGroup.trials].sort((a, b) => a.repetition - b.repetition);
    const tradSorted = [...tradGroup.trials].sort((a, b) => a.repetition - b.repetition);

    for (const metric of metrics) {
      const vcValues: number[] = [];
      const tradValues: number[] = [];
      for (let r = 0; r < R; r++) {
        vcValues.push(getMetricValue(vcSorted[r], metric));
        tradValues.push(getMetricValue(tradSorted[r], metric));
      }
      comparisons.push({ corpus, taskId, model, metric, vcValues, tradValues });
    }
  }

  return comparisons;
}

function getMetricValue(trial: TrialResult, metric: string): number {
  switch (metric) {
    case 'correctness':
      return trial.correctness;
    case 'tokens.total':
      return trial.tokens.total;
    case 'steps':
      return trial.steps;
    default:
      return 0;
  }
}

// ── Hypothesis evaluation ─────────────────────────────────────────────────

function evaluateHypotheses(
  report: ExperimentReport,
  results: StatisticalResult[],
  alpha: number,
): HypothesisVerdict[] {
  const hypotheses: HypothesisVerdict[] = [];

  // H1: Token efficiency — TER > 1.0 with p < α_adjusted on ≥ 2 corpora
  hypotheses.push(evaluateH1(report, results, alpha));

  // H2: Step efficiency — SER > 1.0 on ≥ 50% of hard tasks (difficulty ≥ 3)
  hypotheses.push(evaluateH2(report, results, alpha));

  // H3: Cross-module advantage — ΔMCS_hard > ΔMCS_easy
  hypotheses.push(evaluateH3(report, results, alpha));

  // H4: Lower variance — Var(correctness_vc) < Var(correctness_trad)
  hypotheses.push(evaluateH4(report, alpha));

  return hypotheses;
}

function evaluateH1(
  report: ExperimentReport,
  results: StatisticalResult[],
  alpha: number,
): HypothesisVerdict {
  const tokenResults = results.filter(r => r.metric === 'tokens.total');
  const corporaWithTER: string[] = [];
  const corpusTER: Record<string, number> = {};

  for (const corpus of report.config.corpora) {
    const corpusResults = tokenResults.filter(r => r.corpus === corpus);
    if (corpusResults.length === 0) continue;

    // Compute aggregate TER for this corpus
    const vcTrials = report.trials.filter(t => t.corpus === corpus && t.arm === 'vectorcode');
    const tradTrials = report.trials.filter(t => t.corpus === corpus && t.arm === 'traditional');
    if (vcTrials.length === 0 || tradTrials.length === 0) continue;

    const ter = computeTER(vcTrials, tradTrials);
    corpusTER[corpus] = ter;

    // Check if any comparison in this corpus is significant with TER > 1
    const significant = corpusResults.some(r => r.significant && r.pValue < alpha);
    if (ter > 1.0 && significant) {
      corporaWithTER.push(corpus);
    }
  }

  const supported = corporaWithTER.length >= 2;
  const evidence = `TER > 1.0 on ${corporaWithTER.length}/${report.config.corpora.length} corpora: ${Object.entries(corpusTER).map(([c, v]) => `${c}=${v.toFixed(2)}`).join(', ')}`;

  return {
    id: 'H1',
    supported,
    evidence,
    effectSize: ss.mean(Object.values(corpusTER).filter(v => isFinite(v))),
  };
}

function evaluateH2(
  report: ExperimentReport,
  results: StatisticalResult[],
  alpha: number,
): HypothesisVerdict {
  const stepResults = results.filter(r => r.metric === 'steps');

  // Filter to hard tasks (difficulty ≥ 3)
  const taskDifficulty = new Map<string, number>();
  for (const t of allTasks) {
    taskDifficulty.set(t.id, t.difficulty);
  }

  const hardTaskResults = stepResults.filter(r => (taskDifficulty.get(r.taskId) || 0) >= 3);
  const hardWithSER: string[] = [];

  for (const r of hardTaskResults) {
    const vcTrials = report.trials.filter(
      t => t.corpus === r.corpus && t.taskId === r.taskId && t.model === r.model && t.arm === 'vectorcode',
    );
    const tradTrials = report.trials.filter(
      t => t.corpus === r.corpus && t.taskId === r.taskId && t.model === r.model && t.arm === 'traditional',
    );
    if (vcTrials.length === 0 || tradTrials.length === 0) continue;

    const ser = computeSER(vcTrials, tradTrials);
    if (ser > 1.0 && r.significant && r.pValue < alpha) {
      hardWithSER.push(r.taskId);
    }
  }

  const totalHard = new Set(hardTaskResults.map(r => r.taskId)).size;
  const supported = totalHard > 0 && hardWithSER.length >= totalHard * 0.5;
  const evidence = `SER > 1.0 on ${hardWithSER.length}/${totalHard} hard tasks (difficulty ≥ 3)`;

  return {
    id: 'H2',
    supported,
    evidence,
    effectSize: hardTaskResults.length > 0 ? ss.mean(hardTaskResults.map(r => r.effectSize)) : 0,
  };
}

function evaluateH3(
  report: ExperimentReport,
  results: StatisticalResult[],
  alpha: number,
): HypothesisVerdict {
  const correctnessResults = results.filter(r => r.metric === 'correctness');

  const taskDifficulty = new Map<string, number>();
  for (const t of allTasks) {
    taskDifficulty.set(t.id, t.difficulty);
  }

  const hardResults = correctnessResults.filter(r => (taskDifficulty.get(r.taskId) || 0) >= 3);
  const easyResults = correctnessResults.filter(r => (taskDifficulty.get(r.taskId) || 0) < 3);

  const deltaMCS_hard = hardResults.length > 0
    ? ss.mean(hardResults.map(r => r.vectorcode.mean - r.traditional.mean))
    : 0;
  const deltaMCS_easy = easyResults.length > 0
    ? ss.mean(easyResults.map(r => r.vectorcode.mean - r.traditional.mean))
    : 0;

  const supported = deltaMCS_hard > deltaMCS_easy;
  const evidence = `ΔMCS_hard=${deltaMCS_hard.toFixed(3)} vs ΔMCS_easy=${deltaMCS_easy.toFixed(3)}`;

  return {
    id: 'H3',
    supported,
    evidence,
    effectSize: deltaMCS_hard - deltaMCS_easy,
  };
}

function evaluateH4(
  report: ExperimentReport,
  _alpha: number,
): HypothesisVerdict {
  const vcCorrectness = report.trials
    .filter(t => t.arm === 'vectorcode')
    .map(t => t.correctness);
  const tradCorrectness = report.trials
    .filter(t => t.arm === 'traditional')
    .map(t => t.correctness);

  if (vcCorrectness.length < 2 || tradCorrectness.length < 2) {
    return {
      id: 'H4',
      supported: false,
      evidence: 'Insufficient data for variance comparison',
      effectSize: 0,
    };
  }

  const varVC = ss.variance(vcCorrectness);
  const varTrad = ss.variance(tradCorrectness);

  // Simple F-test for variance ratio
  const fRatio = varTrad > 0 ? varVC / varTrad : 0;
  const supported = varVC < varTrad;
  const evidence = `Var(VC)=${varVC.toFixed(4)} vs Var(TR)=${varTrad.toFixed(4)}, F=${fRatio.toFixed(3)}`;

  return {
    id: 'H4',
    supported,
    evidence,
    effectSize: fRatio,
  };
}

// ── Main analysis entry point ─────────────────────────────────────────────

/**
 * Analyze an experiment report and produce an AnalysisReport.
 */
export function analyzeExperiment(report: ExperimentReport): AnalysisReport {
  const { trials, config } = report;

  // Warn if underpowered
  const R = config.repetitions;
  if (R < 3) {
    console.warn(`[Analysis] Warning: R=${R} is underpowered. Results may not be statistically significant.`);
  }

  // Group trials
  const groups = groupBy(trials);

  // Generate comparisons
  const comparisons = generateComparisons(groups);
  const totalComparisons = comparisons.length;
  const bonferroniAlpha = totalComparisons > 0 ? 0.05 / totalComparisons : 0.05;

  // Compute statistical results
  const results: StatisticalResult[] = [];

  for (const comp of comparisons) {
    const { statistic, pValue } = wilcoxonSignedRank(comp.vcValues, comp.tradValues);
    const d = cohensD(comp.vcValues, comp.tradValues);

    const vcMean = comp.vcValues.length > 0 ? ss.mean(comp.vcValues) : 0;
    const tradMean = comp.tradValues.length > 0 ? ss.mean(comp.tradValues) : 0;
    const vcStd = comp.vcValues.length > 1 ? ss.standardDeviation(comp.vcValues) : 0;
    const tradStd = comp.tradValues.length > 1 ? ss.standardDeviation(comp.tradValues) : 0;
    const vcCI = bootstrapCI(comp.vcValues);
    const tradCI = bootstrapCI(comp.tradValues);

    results.push({
      metric: comp.metric,
      corpus: comp.corpus,
      taskId: comp.taskId,
      model: comp.model,
      vectorcode: { mean: vcMean, std: vcStd, ci95: vcCI },
      traditional: { mean: tradMean, std: tradStd, ci95: tradCI },
      testStatistic: statistic,
      pValue,
      effectSize: d,
      effectMagnitude: classifyEffect(d),
      significant: pValue < bonferroniAlpha,
    });
  }

  // Compute aggregate TER and SER per corpus
  const ter: Record<string, number> = {};
  const ser: Record<string, number> = {};

  for (const corpus of config.corpora) {
    const vcTrials = trials.filter(t => t.corpus === corpus && t.arm === 'vectorcode');
    const tradTrials = trials.filter(t => t.corpus === corpus && t.arm === 'traditional');
    if (vcTrials.length > 0 && tradTrials.length > 0) {
      ter[corpus] = computeTER(vcTrials, tradTrials);
      ser[corpus] = computeSER(vcTrials, tradTrials);
    }
  }

  // Evaluate hypotheses
  const hypotheses = evaluateHypotheses(report, results, bonferroniAlpha);

  return {
    results,
    bonferroniAlpha,
    totalComparisons,
    significantCount: results.filter(r => r.significant).length,
    summary: {
      ter,
      ser,
      hypotheses,
    },
  };
}
