import { vectorcodeTasks } from './vectorcode/index.js';
import { mockMiniTasks } from './mock-mini/index.js';
import { miniTasks } from './mini/index.js';
import { Task } from '../types.js';

export { vectorcodeTasks } from './vectorcode/index.js';
export { mockMiniTasks } from './mock-mini/index.js';
export { miniTasks } from './mini/index.js';

export const allTasks: Task[] = [
  ...mockMiniTasks,
  ...miniTasks,
  ...vectorcodeTasks,
];

// Backward-compatible alias
export const tasks = allTasks;

export function getTasksForCorpus(corpus: string): Task[] {
  switch (corpus) {
    case 'mock-mini':
      return mockMiniTasks;
    case 'mini':
      return miniTasks;
    case 'vectorcode':
      return vectorcodeTasks;
    default:
      throw new Error(`Unknown corpus: ${corpus}`);
  }
}
