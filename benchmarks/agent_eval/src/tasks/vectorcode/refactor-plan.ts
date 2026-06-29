import { Task } from '../../types.js';

export const taskRefactorPlan: Task = {
  id: 'task-refactor-plan',
  name: 'Cross-Module Refactoring Plan',
  prompt: 'The `Embedder` trait in `src/embedder/mod.rs` currently returns `Vec<f32>` from its `embed` method. Propose a plan to change it to return a generic `EmbeddingVector` type that could support both f32 and f16 representations. Identify: (1) every file that implements the Embedder trait, (2) every file that calls `.embed()` or `.embed_batch()`, (3) the blast radius — which other modules would need to change and why.',
  verify: async () => {
    return { success: true };
  }
};
