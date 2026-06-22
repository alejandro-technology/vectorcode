import { Task } from '../types.js';
import * as fs from 'fs';
import * as path from 'path';
import { exec } from 'child_process';
import { promisify } from 'util';

const execAsync = promisify(exec);

export const taskStatusCommand: Task = {
  id: 'task-status-command',
  name: 'Implement CLI Subcommand',
  prompt: 'Write a complete Rust file `src/cli/status_eval.rs` implementing a `run()` function that: (1) loads the VectorCode config from `.vectorcode/config.toml`, (2) opens the SQLite database at `.vectorcode/index.db`, (3) reads the `meta` table to get provider name and model, (4) counts rows in the `chunks` table, and (5) prints a formatted status summary. Use the project\'s existing `config` and `store` modules — do NOT reimplement config parsing or database access.',
  verify: async (workspaceDir: string) => {
    // If dry-run, immediately pass
    if (process.argv.includes('--dry-run')) {
      return { success: true };
    }

    const filePath = path.join(workspaceDir, 'src/cli/status_eval.rs');
    if (!fs.existsSync(filePath)) {
      return { success: false, error: 'src/cli/status_eval.rs was not created' };
    }

    const modRsPath = path.join(workspaceDir, 'src/cli/mod.rs');
    if (!fs.existsSync(modRsPath)) {
      return { success: false, error: `CLI module definition not found at: ${modRsPath}` };
    }

    const originalModContent = fs.readFileSync(modRsPath, 'utf8');

    try {
      if (!originalModContent.includes('pub mod status_eval;')) {
        fs.writeFileSync(modRsPath, originalModContent + '\npub mod status_eval;\n');
      }

      console.log('[Verify] Running cargo check --all-targets to verify status_eval.rs...');
      await execAsync('cargo check --all-targets', { cwd: workspaceDir });
      return { success: true };
    } catch (err: any) {
      return {
        success: false,
        error: `Compiler verification failed: ${err.stderr || err.stdout || err.message}`
      };
    } finally {
      // Restore mod.rs
      fs.writeFileSync(modRsPath, originalModContent);
      // Clean up the created status_eval.rs file
      if (fs.existsSync(filePath)) {
        try {
          fs.unlinkSync(filePath);
        } catch (e) {
          console.error(`Failed to clean up status_eval.rs: ${e}`);
        }
      }
    }
  }
};
