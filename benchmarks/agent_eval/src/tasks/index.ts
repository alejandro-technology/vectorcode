import { vectorcodeTasks } from './vectorcode/index.js';

// Backward-compatible alias — Phase D will add corpus-aware getTasksForCorpus()
export const tasks = vectorcodeTasks;
export { vectorcodeTasks };
