import * as fs from 'fs';
import * as path from 'path';
import * as crypto from 'crypto';
import { execSync } from 'child_process';

export interface LLMResponse {
  text: string;
  toolCalls: { name: string; args: Record<string, any>; id: string }[];
  tokens: { input: number; output: number };
  stopReason: 'end_turn' | 'tool_use' | 'max_tokens';
}

export interface CacheEntry {
  stepIndex: number;
  requestHash: string; // SHA-256 of the messages array
  response: LLMResponse;
  tokens: { input: number; output: number };
  timestamp: string;
}

export interface TrajectoryMetadata {
  workspaceSha: string; // git rev-parse HEAD at recording time
  model: string;
  taskId: string;
  arm: 'vectorcode' | 'traditional';
  recordedAt: string;
  totalSteps: number;
}

export type CacheMode = 'cached' | 'live' | 'update-cache' | 'dry-run';

const workspaceRoot = path.resolve(process.cwd(), '../../');
const cacheDir = path.resolve(process.cwd(), 'cache');

export function getGitSha(): string {
  try {
    return execSync('git rev-parse HEAD', { cwd: workspaceRoot, encoding: 'utf8' }).trim();
  } catch (e) {
    return 'unknown-sha';
  }
}

export function isGitDirty(): boolean {
  try {
    const status = execSync('git status --porcelain', { cwd: workspaceRoot, encoding: 'utf8' }).trim();
    if (!status) return false;
    
    // Only care about changes under src/, benchmarks/agent_eval/src/, or Cargo.toml
    const lines = status.split('\n').filter(Boolean);
    const codeChanges = lines.filter(line => {
      const filePath = line.slice(3).trim();
      return filePath.startsWith('src/') || 
             filePath.startsWith('benchmarks/agent_eval/src/') || 
             filePath === 'Cargo.toml';
    });
    return codeChanges.length > 0;
  } catch (e) {
    return false;
  }
}

export function computeRequestHash(messages: any[]): string {
  const serialized = JSON.stringify(messages);
  return crypto.createHash('sha256').update(serialized).digest('hex');
}

function getAllFiles(dir: string): string[] {
  const results: string[] = [];
  if (!fs.existsSync(dir)) return results;
  const list = fs.readdirSync(dir);
  for (const file of list) {
    if (file === 'node_modules' || file === '.git' || file === 'target' || file === 'dist' || file === 'cache' || file === 'results') {
      continue;
    }
    const fullPath = path.join(dir, file);
    const stat = fs.statSync(fullPath);
    if (stat && stat.isDirectory()) {
      results.push(...getAllFiles(fullPath));
    } else {
      results.push(fullPath);
    }
  }
  return results;
}

function computeFileHash(filePath: string): string {
  const content = fs.readFileSync(filePath);
  return crypto.createHash('sha256').update(content).digest('hex');
}

export function createSnapshotManifest(workspaceDir: string = workspaceRoot): void {
  const gitSha = getGitSha();
  const manifestPath = path.join(workspaceDir, 'snapshots/manifest.json');
  
  const srcDir = path.join(workspaceDir, 'src');
  const agentEvalSrcDir = path.join(workspaceDir, 'benchmarks/agent_eval/src');
  const cargoToml = path.join(workspaceDir, 'Cargo.toml');

  const filesToHash: string[] = [];
  if (fs.existsSync(srcDir)) filesToHash.push(...getAllFiles(srcDir));
  if (fs.existsSync(agentEvalSrcDir)) filesToHash.push(...getAllFiles(agentEvalSrcDir));
  if (fs.existsSync(cargoToml)) filesToHash.push(cargoToml);

  const files: Record<string, string> = {};
  for (const file of filesToHash) {
    const relativePath = path.relative(workspaceDir, file);
    files[relativePath] = computeFileHash(file);
  }

  const manifest = {
    gitSha,
    timestamp: new Date().toISOString(),
    files
  };

  const snapshotsDir = path.join(workspaceDir, 'snapshots');
  if (!fs.existsSync(snapshotsDir)) {
    fs.mkdirSync(snapshotsDir, { recursive: true });
  }
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2), 'utf8');
  console.log(`[Cache] Snapshot manifest created at: ${manifestPath}`);
}

export function verifySnapshotManifest(workspaceDir: string = workspaceRoot): { matches: boolean; mismatchFiles: string[] } {
  const manifestPath = path.join(workspaceDir, 'snapshots/manifest.json');
  if (!fs.existsSync(manifestPath)) {
    return { matches: true, mismatchFiles: [] };
  }

  try {
    const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    const mismatchFiles: string[] = [];

    for (const [relPath, expectedHash] of Object.entries(manifest.files)) {
      const fullPath = path.join(workspaceDir, relPath);
      if (!fs.existsSync(fullPath)) {
        mismatchFiles.push(`${relPath} (missing)`);
        continue;
      }
      const actualHash = computeFileHash(fullPath);
      if (actualHash !== expectedHash) {
        mismatchFiles.push(`${relPath} (modified)`);
      }
    }

    return {
      matches: mismatchFiles.length === 0,
      mismatchFiles
    };
  } catch (e) {
    console.warn(`[Cache] Warning: Failed to read/parse snapshot manifest:`, e);
    return { matches: false, mismatchFiles: ['manifest.json (corrupt)'] };
  }
}

export function parseCacheMode(args: string[]): CacheMode {
  if (args.includes('--dry-run')) {
    return 'dry-run';
  }
  if (args.includes('--live')) {
    if (args.includes('--update-cache')) {
      return 'update-cache';
    }
    return 'live';
  }
  return 'cached';
}

export class CacheManager {
  private getTrajectoryPath(model: string, taskId: string, arm: 'vectorcode' | 'traditional'): string {
    return path.join(cacheDir, model, taskId, arm, 'trajectory.jsonl');
  }

  loadTrajectory(model: string, taskId: string, arm: 'vectorcode' | 'traditional'): { metadata: TrajectoryMetadata; entries: CacheEntry[] } | null {
    const filePath = this.getTrajectoryPath(model, taskId, arm);
    if (!fs.existsSync(filePath)) {
      return null;
    }

    try {
      const content = fs.readFileSync(filePath, 'utf8');
      const lines = content.split('\n').map(l => l.trim()).filter(Boolean);
      if (lines.length === 0) {
        return null;
      }

      const metadata: TrajectoryMetadata = JSON.parse(lines[0]);
      const entries: CacheEntry[] = [];
      for (let i = 1; i < lines.length; i++) {
        entries.push(JSON.parse(lines[i]));
      }

      return { metadata, entries };
    } catch (e) {
      console.warn(`[Cache] Warning: Failed to load trajectory from ${filePath}:`, e);
      return null;
    }
  }

  saveTrajectory(
    model: string,
    taskId: string,
    arm: 'vectorcode' | 'traditional',
    metadata: Omit<TrajectoryMetadata, 'recordedAt' | 'totalSteps'>,
    entries: CacheEntry[]
  ): void {
    const filePath = this.getTrajectoryPath(model, taskId, arm);
    const dir = path.dirname(filePath);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }

    const fullMetadata: TrajectoryMetadata = {
      ...metadata,
      recordedAt: new Date().toISOString(),
      totalSteps: entries.length
    };

    const lines = [
      JSON.stringify(fullMetadata),
      ...entries.map(e => JSON.stringify(e))
    ];

    fs.writeFileSync(filePath, lines.join('\n') + '\n', 'utf8');
    console.log(`[Cache] Trajectory saved successfully to ${filePath}`);
  }
}
