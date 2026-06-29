import { Task } from '../../types.js';

export const taskMiniErrorDerive: Task = {
  id: 'mini-error-derive',
  name: 'Error Derive Macro Analysis',
  prompt: 'In the thiserror crate, find the main derive macro. What error types does it support? List the attributes (#[from], #[source], etc.) and explain what each generates.',
  corpus: 'mini',
  difficulty: 2,
  type: 'read',
  targetRepos: ['thiserror'],
  verify: async () => {
    return { success: true };
  }
};
