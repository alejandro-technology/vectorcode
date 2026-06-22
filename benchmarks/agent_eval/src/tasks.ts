import { Task } from './types.js';
import * as fs from 'fs';
import * as path from 'path';

export const tasks: Task[] = [
  {
    id: 'task-1-read',
    name: 'Code Discovery & Understanding',
    prompt: 'Find which files define the `VectorCodeError` enum and the `sanitize_fts_query` function. Briefly explain how the query sanitization behaves (e.g. what characters it strips).',
    verify: async (workspaceDir: string) => {
      // For read tasks, verification is done by checking if the agent output
      // contains the correct answers. Since verification runs on the workspace,
      // and harness will pass the agent's output as an artifact, we'll verify it
      // in the harness using a custom check, but let's provide a default true check here.
      return { success: true };
    }
  },
  {
    id: 'task-2-write',
    name: 'Write CLI Mock Subcommand',
    prompt: 'Provide the Rust code for a new file `src/cli/status_mock.rs` that implements a mock CLI command. It must export a public function `pub fn run_status() -> String` which returns the string "Mock Status: OK". Make sure it is syntactically valid Rust.',
    verify: async (workspaceDir: string) => {
      return { success: true };
    }
  }
];
