import { Task } from '../../types.js';

export const taskMockErrorLookup: Task = {
  id: 'mock-error-lookup',
  name: 'Error Enum Lookup',
  prompt: 'Find the VectorCodeError enum definition. List all its variants.',
  corpus: 'mock-mini',
  difficulty: 1,
  type: 'read',
  verify: async () => {
    return { success: true };
  }
};
