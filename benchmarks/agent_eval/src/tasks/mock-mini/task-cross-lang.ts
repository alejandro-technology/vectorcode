import { Task } from '../../types.js';

export const taskMockCrossLang: Task = {
  id: 'mock-cross-lang',
  name: 'Cross-Language Comparison',
  prompt: 'Compare how rate limiting is implemented in rate_limiter.ts vs how signing works in signing.py.',
  corpus: 'mock-mini',
  difficulty: 2,
  type: 'read',
  verify: async () => {
    return { success: true };
  }
};
