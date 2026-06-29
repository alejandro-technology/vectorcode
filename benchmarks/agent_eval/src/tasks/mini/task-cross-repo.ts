import { Task } from '../../types.js';

export const taskMiniCrossRepo: Task = {
  id: 'mini-cross-repo',
  name: 'Cross-Repo API Comparison',
  prompt: 'Compare how all three repos (thiserror, defu, itsdangerous) handle their primary public API surface. For each: (1) where is the main entry point, (2) how are errors/edge cases handled, (3) what design patterns are used for the public interface.',
  corpus: 'mini',
  difficulty: 5,
  type: 'read',
  targetRepos: ['thiserror', 'defu', 'itsdangerous'],
  verify: async () => {
    return { success: true };
  }
};
