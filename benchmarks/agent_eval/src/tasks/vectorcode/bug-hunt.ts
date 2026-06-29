import { Task } from '../../types.js';

export const taskBugHunt: Task = {
  id: 'task-bug-hunt',
  name: 'Bug Hunt',
  prompt: 'The `sanitize_fts_query` function strips certain characters from search queries before passing them to SQLite FTS5. Find this function, explain its sanitization logic, and identify whether it handles the case where the ENTIRE query consists of special characters (i.e., would it return an empty string?).',
  verify: async () => {
    return { success: true };
  }
};
