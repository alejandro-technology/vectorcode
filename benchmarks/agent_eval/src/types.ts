export interface Task {
  id: string;
  name: string;
  prompt: string;
  verify: (workspaceDir: string) => Promise<{ success: boolean; error?: string }>;
}

export interface AgentConfig {
  model: string;
  provider: 'openai' | 'anthropic' | 'dry-run';
  maxSteps?: number;
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
}

export interface RubricCriterion {
  name: string;
  weight: number;
  description: string;
  groundTruth: string;
}

export interface TaskRubric {
  taskId: string;
  criteria: RubricCriterion[];
}

export interface JudgeResult {
  score: number;           // 0.0 - 1.0
  criteriaScores: Record<string, { score: number; reasoning: string }>;
  overallReasoning: string;
}

