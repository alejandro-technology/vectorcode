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

