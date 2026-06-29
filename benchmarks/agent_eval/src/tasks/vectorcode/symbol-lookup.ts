import { Task } from '../../types.js';

export const taskSymbolLookup: Task = {
  id: 'task-symbol-lookup',
  name: 'Single-Symbol Lookup',
  prompt: 'Find the definition of the `VectorCodeError` enum. List all its variants and the file where it\'s defined.',
  corpus: 'vectorcode',
  difficulty: 1,
  type: 'read',
  verify: async () => {
    return { success: true };
  }
};
