import { Task } from '../../types.js';

export const taskMiniMergeTrace: Task = {
  id: 'mini-merge-trace',
  name: 'Merge Pipeline Trace',
  prompt: 'In defu, trace the full merge pipeline. Start from the exported `defu()` function and follow the code path through to how individual properties are merged. Name each function and file involved.',
  corpus: 'mini',
  difficulty: 3,
  type: 'read',
  targetRepos: ['defu'],
  verify: async () => {
    return { success: true };
  }
};
