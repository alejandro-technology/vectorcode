#!/usr/bin/env tsx
/**
 * Cache Path Migration Script
 *
 * Migrates old-format cache paths to new corpus-aware paths:
 *   Old: cache/<model>/<taskId>/<arm>/trajectory.jsonl
 *   New: cache/<model>/<corpus>/<taskId>/<arm>/trajectory.jsonl
 *
 * Known vectorcode task IDs are detected and moved into the vectorcode corpus.
 * The script is idempotent: skips if the new path already exists.
 * Old directories are cleaned up after successful copy.
 */

import * as fs from 'fs';
import * as path from 'path';

const CACHE_DIR = path.resolve(import.meta.dirname ?? process.cwd(), '..', 'cache');

// Known vectorcode task IDs (Phase 1 tasks that predate multi-corpus)
const VECTORCODE_TASK_IDS = new Set([
  'task-symbol-lookup',
  'task-arch-trace',
  'task-bug-hunt',
  'task-status-command',
  'task-refactor-plan',
]);

// Known mini task IDs
const MINI_TASK_IDS = new Set([
  'mini-error-derive',
  'mini-merge-trace',
  'mini-signing-flow',
  'mini-cross-repo',
]);

// Known mock-mini task IDs
const MOCK_MINI_TASK_IDS = new Set([
  'mock-error-lookup',
  'mock-cross-lang',
]);

function classifyTask(taskId: string): string | null {
  if (VECTORCODE_TASK_IDS.has(taskId)) return 'vectorcode';
  if (MINI_TASK_IDS.has(taskId)) return 'mini';
  if (MOCK_MINI_TASK_IDS.has(taskId)) return 'mock-mini';
  return null;
}

function migrateCache(): void {
  if (!fs.existsSync(CACHE_DIR)) {
    console.log(`[Migrate] Cache directory not found: ${CACHE_DIR}`);
    console.log(`[Migrate] Nothing to migrate.`);
    return;
  }

  let copiedCount = 0;
  let skippedCount = 0;
  let cleanedCount = 0;

  // Scan model directories (skip 'judge' — it has its own format)
  const modelEntries = fs.readdirSync(CACHE_DIR, { withFileTypes: true });
  for (const modelEntry of modelEntries) {
    if (!modelEntry.isDirectory() || modelEntry.name === 'judge') continue;
    const modelDir = path.join(CACHE_DIR, modelEntry.name);

    // Look for old-format task directories directly under model/
    const taskEntries = fs.readdirSync(modelDir, { withFileTypes: true });
    for (const taskEntry of taskEntries) {
      if (!taskEntry.isDirectory()) continue;

      const taskId = taskEntry.name;
      const corpus = classifyTask(taskId);

      // Skip if not a known task ID (might already be in new format)
      if (!corpus) {
        // Check if this is already a corpus directory (new format)
        if (['vectorcode', 'mini', 'mock-mini'].includes(taskId)) {
          continue; // Already migrated format
        }
        console.log(`[Migrate] Unknown task ID '${taskId}' under ${modelEntry.name}/, skipping.`);
        continue;
      }

      const oldTaskDir = path.join(modelDir, taskId);

      // Check for arm subdirectories (vectorcode/ or traditional/)
      const armEntries = fs.readdirSync(oldTaskDir, { withFileTypes: true });
      for (const armEntry of armEntries) {
        if (!armEntry.isDirectory()) continue;
        if (armEntry.name !== 'vectorcode' && armEntry.name !== 'traditional') continue;

        const oldFile = path.join(oldTaskDir, armEntry.name, 'trajectory.jsonl');
        if (!fs.existsSync(oldFile)) continue;

        // New path: cache/<model>/<corpus>/<taskId>/<arm>/trajectory.jsonl
        const newDir = path.join(modelDir, corpus, taskId, armEntry.name);
        const newFile = path.join(newDir, 'trajectory.jsonl');

        if (fs.existsSync(newFile)) {
          console.log(`[Migrate] Skip (exists): ${path.relative(CACHE_DIR, newFile)}`);
          skippedCount++;
          continue;
        }

        // Create new directory and copy file
        fs.mkdirSync(newDir, { recursive: true });
        fs.copyFileSync(oldFile, newFile);
        console.log(`[Migrate] Copy: ${path.relative(CACHE_DIR, oldFile)} → ${path.relative(CACHE_DIR, newFile)}`);
        copiedCount++;
      }

      // Clean up old directory (only if all arm files were copied/skipped)
      try {
        fs.rmSync(oldTaskDir, { recursive: true, force: true });
        console.log(`[Migrate] Cleaned: ${path.relative(CACHE_DIR, oldTaskDir)}/`);
        cleanedCount++;
      } catch (e: any) {
        console.warn(`[Migrate] Failed to clean up ${oldTaskDir}: ${e.message}`);
      }
    }
  }

  console.log(`\n[Migrate] Done. Copied: ${copiedCount}, Skipped: ${skippedCount}, Cleaned: ${cleanedCount} dirs.`);
}

migrateCache();
