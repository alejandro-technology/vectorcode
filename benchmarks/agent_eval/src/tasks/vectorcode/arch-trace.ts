import { Task } from '../../types.js';

export const taskArchTrace: Task = {
  id: 'task-arch-trace',
  name: 'Architecture Trace',
  prompt: 'Trace what happens when a file is modified in a VectorCode workspace. Start from the file watcher detecting the change, through chunking, embedding, and storing in the database. For each step, name the specific Rust module and the key function.',
  verify: async () => {
    return { success: true };
  }
};
