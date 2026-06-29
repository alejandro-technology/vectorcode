export interface Task {
  id: string;
  name: string;
  prompt: string;
  corpus: string;                    // corpus this task belongs to
  difficulty: number;                // 1-5 star rating
  type: 'read' | 'write';           // task classification
  targetRepos?: string[];            // repos within corpus (for mini multi-repo tasks)
  verify: (workspaceDir: string) => Promise<{ success: boolean; error?: string }>;
}

export interface AgentConfig {
  model: string;
  provider: 'openai' | 'anthropic' | 'dry-run';
  maxSteps?: number;
  temperature?: number;              // default 0
  timeoutMs?: number;                // default 120000
}

export interface ToolCallRecord {
  toolName: string;
  input: any;
  output?: string;
  durationMs: number;
}

export interface EvalResult {
  taskId: string;
  model: string;
  provider: string;
  success: boolean;
  steps: number;
  tokens: {
    input: number;
    output: number;
    total: number;
  };
  toolCalls: ToolCallRecord[];
  error?: string;
  durationMs: number;
  timedOut: boolean;                 // true if timeout fired before convergence
}

export interface RubricCriterion {
  name: string;
  weight: number;
  description: string;
  groundTruth: string;
}

export interface TaskRubric {
  taskId: string;
  corpus: string;                    // corpus this rubric belongs to
  targetRepo?: string;               // repo within corpus (for mini tasks)
  criteria: RubricCriterion[];
}

export interface JudgeResult {
  score: number;           // 0.0 - 1.0
  criteriaScores: Record<string, { score: number; reasoning: string }>;
  overallReasoning: string;
}

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

export interface StatisticalResult {
  metric: string;
  corpus: string;
  taskId: string;
  model: string;
  vectorcode: { mean: number; std: number; ci95: [number, number] };
  traditional: { mean: number; std: number; ci95: [number, number] };
  testStatistic: number;
  pValue: number;
  effectSize: number;
  effectMagnitude: 'negligible' | 'small' | 'medium' | 'large';
  significant: boolean;
}

export interface AnalysisReport {
  results: StatisticalResult[];
  bonferroniAlpha: number;
  totalComparisons: number;
  significantCount: number;
  summary: {
    ter: Record<string, number>;
    ser: Record<string, number>;
    hypotheses: HypothesisVerdict[];
  };
}

export interface HypothesisVerdict {
  id: string;
  supported: boolean;
  evidence: string;
  effectSize: number;
}

