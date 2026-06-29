import * as fs from 'fs';
import * as path from 'path';
import { execFile, execFileSync } from 'child_process';
import { promisify } from 'util';
import { CorpusConfig } from './types.js';

const execFileAsync = promisify(execFile);

/**
 * Simple TOML parser for the corpus.toml schema.
 * Handles the subset we need: [section], [[array.section]], and key = "value" / [...] pairs.
 */
interface TomlRepo {
  url: string;
  sparse_paths: string[];
  file_extensions: string[];
}

interface TomlCorpusSection {
  url?: string;
  file_extensions?: string[];
  sparse_paths?: string[];
  repos?: TomlRepo[];
}

interface ParsedCorpusToml {
  [corpusId: string]: TomlCorpusSection;
}

function parseCorpusToml(tomlPath: string): ParsedCorpusToml {
  const content = fs.readFileSync(tomlPath, 'utf8');
  const result: ParsedCorpusToml = {};

  let currentSection = '';
  let currentSubSection = ''; // for [[mini.repos]] etc.
  let currentKey = '';

  const lines = content.split('\n');
  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;

    // Array of tables: [[section.subsection]]
    const arrayTableMatch = line.match(/^\[\[([^\]]+)\]\]$/);
    if (arrayTableMatch) {
      const fullName = arrayTableMatch[1];
      const dotIdx = fullName.indexOf('.');
      if (dotIdx !== -1) {
        currentSection = fullName.substring(0, dotIdx);
        currentSubSection = fullName.substring(dotIdx + 1);
      } else {
        currentSection = fullName;
        currentSubSection = '';
      }
      if (!result[currentSection]) result[currentSection] = {};
      if (currentSubSection) {
        if (!result[currentSection].repos) result[currentSection].repos = [];
        result[currentSection].repos!.push({ url: '', sparse_paths: [], file_extensions: [] });
      }
      currentKey = '';
      continue;
    }

    // Table: [section]
    const tableMatch = line.match(/^\[([^\]]+)\]$/);
    if (tableMatch) {
      currentSection = tableMatch[1];
      currentSubSection = '';
      if (!result[currentSection]) result[currentSection] = {};
      currentKey = '';
      continue;
    }

    // Key = value
    const kvMatch = line.match(/^(\w+)\s*=\s*(.+)$/);
    if (kvMatch) {
      const key = kvMatch[1];
      const rawVal = kvMatch[2].trim();
      currentKey = key;

      const value = parseTomlValue(rawVal);

      if (currentSubSection && result[currentSection].repos) {
        const repo = result[currentSection].repos[result[currentSection].repos.length - 1];
        (repo as any)[key] = value;
      } else if (currentSection) {
        (result[currentSection] as any)[key] = value;
      }
      continue;
    }
  }

  return result;
}

function parseTomlValue(raw: string): any {
  // String: "value"
  if (raw.startsWith('"') && raw.endsWith('"')) {
    return raw.slice(1, -1);
  }
  // Array of strings: ["val1", "val2"]
  if (raw.startsWith('[') && raw.endsWith(']')) {
    const inner = raw.slice(1, -1);
    const items = inner.split(',').map(s => s.trim()).filter(Boolean);
    return items.map(item => {
      if (item.startsWith('"') && item.endsWith('"')) return item.slice(1, -1);
      return item;
    });
  }
  // Boolean
  if (raw === 'true') return true;
  if (raw === 'false') return false;
  // Number
  const num = Number(raw);
  if (!isNaN(num)) return num;
  return raw;
}

function getBinPath(): string {
  if (process.env.VECTORCODE_BIN) {
    return path.resolve(process.env.VECTORCODE_BIN);
  }
  return path.resolve(process.cwd(), '../../target/debug/vectorcode');
}

/**
 * CorpusManager handles the lifecycle of benchmark corpora:
 * - mock-mini: local fixtures at tests/fixtures/mini/
 * - mini: 3 small repos cloned into .bench-corpus/mini/
 * - vectorcode: the project root itself
 */
export class CorpusManager {
  private projectRoot: string;
  private workspaceDir: string = '';
  private corpusToml: ParsedCorpusToml;
  private createdCorpusDir = false;

  constructor() {
    this.projectRoot = path.resolve(process.cwd(), '../../');
    const tomlPath = path.resolve(process.cwd(), '../../benchmarks/corpus.toml');
    this.corpusToml = parseCorpusToml(tomlPath);
  }

  /**
   * Prepare the corpus workspace and return its absolute path.
   */
  async prepare(corpusId: string): Promise<string> {
    switch (corpusId) {
      case 'mock-mini':
        return this.prepareMockMini();
      case 'mini':
        return this.prepareMini();
      case 'vectorcode':
        return this.prepareVectorcode();
      default:
        throw new Error(`Unknown corpus: ${corpusId}`);
    }
  }

  getWorkspaceDir(): string {
    return this.workspaceDir;
  }

  /**
   * Remove .bench-corpus/ if it was created during this run.
   * Safe to call even if nothing was created (no-op).
   */
  async cleanup(): Promise<void> {
    if (!this.createdCorpusDir) return;
    const corpusDir = path.join(this.projectRoot, '.bench-corpus');
    if (fs.existsSync(corpusDir)) {
      fs.rmSync(corpusDir, { recursive: true, force: true });
      console.log(`[CorpusManager] Cleaned up ${corpusDir}`);
    }
    this.createdCorpusDir = false;
  }

  /**
   * Return index state for the current workspace.
   */
  async getIndexStatus(): Promise<{ indexed: boolean; chunks: number }> {
    const vcDir = path.join(this.workspaceDir, '.vectorcode');
    if (!fs.existsSync(vcDir)) {
      return { indexed: false, chunks: 0 };
    }
    const dbPath = path.join(vcDir, 'index.db');
    if (!fs.existsSync(dbPath)) {
      return { indexed: false, chunks: 0 };
    }
    // Count rows in chunks table (rough estimate via file size)
    try {
      const bin = getBinPath();
      const { stdout } = await execFileAsync(bin, ['query', '--count'], {
        cwd: this.workspaceDir,
        timeout: 10000,
      });
      const chunks = parseInt(stdout.trim(), 10) || 0;
      return { indexed: true, chunks };
    } catch {
      // If query --count fails, index exists but may be corrupt
      return { indexed: true, chunks: 0 };
    }
  }

  // ── Corpus-specific preparation ────────────────────────────────────────

  private async prepareMockMini(): Promise<string> {
    const fixturesDir = path.join(this.projectRoot, 'tests/fixtures/mini');
    if (!fs.existsSync(fixturesDir)) {
      throw new Error(`mock-mini fixtures not found at: ${fixturesDir}`);
    }

    this.workspaceDir = fixturesDir;
    const bin = getBinPath();

    // Check if already indexed
    const vcDir = path.join(fixturesDir, '.vectorcode');
    if (fs.existsSync(vcDir)) {
      console.log(`[CorpusManager] mock-mini already indexed at ${fixturesDir}`);
      return fixturesDir;
    }

    console.log(`[CorpusManager] Initializing mock-mini corpus at ${fixturesDir}...`);

    // Run vectorcode init --provider mock
    await execFileAsync(bin, ['init', '--provider', 'mock'], { cwd: fixturesDir });
    console.log(`[CorpusManager] vectorcode init --provider mock completed`);

    // Run vectorcode index
    await execFileAsync(bin, ['index'], { cwd: fixturesDir });
    console.log(`[CorpusManager] vectorcode index completed`);

    return fixturesDir;
  }

  private async prepareMini(): Promise<string> {
    const corpusDir = path.join(this.projectRoot, '.bench-corpus/mini');
    fs.mkdirSync(corpusDir, { recursive: true });
    this.createdCorpusDir = true;
    this.workspaceDir = corpusDir;

    const repos = this.corpusToml['mini']?.repos;
    if (!repos || repos.length === 0) {
      throw new Error('No repos defined in corpus.toml for mini corpus');
    }

    const bin = getBinPath();

    for (const repo of repos) {
      const repoName = repoUrlToName(repo.url);
      const repoDir = path.join(corpusDir, repoName);

      if (fs.existsSync(repoDir)) {
        console.log(`[CorpusManager] ${repoName} already cloned, skipping`);
      } else {
        console.log(`[CorpusManager] Cloning ${repo.url} into ${repoDir}...`);
        await this.cloneRepoWithRetry(repo.url, repoDir, repo.sparse_paths);
      }

      // Run vectorcode init + index in the repo workspace
      const vcDir = path.join(repoDir, '.vectorcode');
      if (!fs.existsSync(vcDir)) {
        await execFileAsync(bin, ['init'], { cwd: repoDir });
      }
      await execFileAsync(bin, ['index'], { cwd: repoDir });
      console.log(`[CorpusManager] ${repoName} indexed`);
    }

    return corpusDir;
  }

  private async prepareVectorcode(): Promise<string> {
    this.workspaceDir = this.projectRoot;
    const vcDir = path.join(this.projectRoot, '.vectorcode');

    if (!fs.existsSync(vcDir)) {
      console.error(`[CorpusManager] Warning: .vectorcode/ not found in project root. Index may need to be created.`);
      return this.projectRoot;
    }

    // Check staleness: compare index.db mtime vs last git commit
    const dbPath = path.join(vcDir, 'index.db');
    if (fs.existsSync(dbPath)) {
      try {
        const dbStat = fs.statSync(dbPath);
        const lastCommitMs = getLatestCommitMs(this.projectRoot);
        if (dbStat.mtimeMs < lastCommitMs) {
          console.log(`[CorpusManager] Index is stale (older than last commit). Re-indexing...`);
          const bin = getBinPath();
          await execFileAsync(bin, ['index'], { cwd: this.projectRoot });
          console.log(`[CorpusManager] Re-index completed`);
        } else {
          console.log(`[CorpusManager] Index is up to date`);
        }
      } catch (e) {
        console.warn(`[CorpusManager] Could not check index staleness:`, e);
      }
    }

    return this.projectRoot;
  }

  // ── Helpers ─────────────────────────────────────────────────────────────

  /**
   * Clone a repo with sparse checkout. Retries 3 times with exponential backoff.
   */
  private async cloneRepoWithRetry(
    url: string,
    targetDir: string,
    sparsePaths: string[],
    maxAttempts = 3,
  ): Promise<void> {
    for (let attempt = 1; attempt <= maxAttempts; attempt++) {
      try {
        // git clone --filter=blob:none --no-checkout
        await execFileAsync('git', ['clone', '--filter=blob:none', '--no-checkout', url, targetDir], {
          timeout: 60000,
        });
        // git sparse-checkout init --cone
        await execFileAsync('git', ['sparse-checkout', 'init', '--cone'], { cwd: targetDir });
        // git sparse-checkout set <paths>
        await execFileAsync('git', ['sparse-checkout', 'set', ...sparsePaths], { cwd: targetDir });
        // git checkout
        await execFileAsync('git', ['checkout'], { cwd: targetDir });
        return;
      } catch (e: any) {
        console.warn(`[CorpusManager] Clone attempt ${attempt}/${maxAttempts} failed: ${e.message}`);
        if (attempt < maxAttempts) {
          const delay = Math.pow(2, attempt) * 1000; // exponential backoff
          await new Promise(r => setTimeout(r, delay));
          // Clean up partial clone
          if (fs.existsSync(targetDir)) {
            fs.rmSync(targetDir, { recursive: true, force: true });
          }
        } else {
          throw new Error(`Failed to clone ${url} after ${maxAttempts} attempts: ${e.message}`);
        }
      }
    }
  }
}

/**
 * Extract a short name from a repo URL for directory naming.
 * "https://github.com/dtolnay/thiserror" → "thiserror"
 */
function repoUrlToName(url: string): string {
  const cleaned = url.replace(/\.git$/, '').replace(/\/$/, '');
  return path.basename(cleaned);
}

/**
 * Get the timestamp (ms) of the latest git commit in a directory.
 */
function getLatestCommitMs(dir: string): number {
  try {
    const result = execFileSync('git', ['log', '-1', '--format=%ct'], {
      cwd: dir,
      encoding: 'utf8',
      timeout: 5000,
    });
    return parseInt(result.trim(), 10) * 1000;
  } catch {
    return 0;
  }
}
