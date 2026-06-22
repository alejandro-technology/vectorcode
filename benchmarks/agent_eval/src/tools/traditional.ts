import { execFile } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { ToolProvider, ToolDefinition } from './types.js';

const workspaceRoot = path.resolve(process.cwd(), '../../');

function resolvePath(p?: string): string {
  if (!p) return workspaceRoot;
  if (path.isAbsolute(p)) return p;
  return path.resolve(workspaceRoot, p);
}

function execFileAsync(file: string, args: string[], options: { cwd?: string } = {}): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    execFile(file, args, options, (error, stdout, stderr) => {
      if (error && (error as any).code !== 1) { // ripgrep exit code 1 means no matches found, not a hard failure
        reject(error);
      } else {
        resolve({ stdout, stderr });
      }
    });
  });
}

const TOOLS: ToolDefinition[] = [
  {
    name: 'grep',
    description: 'Search for query patterns inside files recursively using ripgrep (rg).',
    inputSchema: {
      type: 'object',
      properties: {
        query: { type: 'string', description: 'Query pattern or regex to search for.' },
        path: { type: 'string', description: 'Optional path to search (relative to workspace root).' },
        flags: { type: 'string', description: 'Optional ripgrep flags (e.g. "-i" for case-insensitive).' }
      },
      required: ['query']
    }
  },
  {
    name: 'find_files',
    description: 'Find files in the workspace matching a pattern name using the find command.',
    inputSchema: {
      type: 'object',
      properties: {
        pattern: { type: 'string', description: 'Pattern/name to search for (glob or substring).' },
        path: { type: 'string', description: 'Optional directory path to start search from (relative to workspace root).' }
      },
      required: ['pattern']
    }
  },
  {
    name: 'read_file',
    description: 'Read the contents of a file in the workspace.',
    inputSchema: {
      type: 'object',
      properties: {
        file_path: { type: 'string', description: 'Path to the file to read (relative to workspace root).' },
        start_line: { type: 'number', description: 'Optional 1-indexed start line (inclusive).' },
        end_line: { type: 'number', description: 'Optional 1-indexed end line (inclusive).' }
      },
      required: ['file_path']
    }
  },
  {
    name: 'list_dir',
    description: 'List contents of a directory (files and subdirectories) with sizes.',
    inputSchema: {
      type: 'object',
      properties: {
        path: { type: 'string', description: 'Optional directory path to list (relative to workspace root).' }
      }
    }
  }
];

export class TraditionalProvider implements ToolProvider {
  readonly name = 'traditional' as const;

  async initialize(): Promise<void> {
    // No-op for traditional provider
    return Promise.resolve();
  }

  listTools(): ToolDefinition[] {
    return TOOLS;
  }

  async callTool(name: string, args: Record<string, any>): Promise<string> {
    switch (name) {
      case 'grep':
        return this.handleGrep(args);
      case 'find_files':
        return this.handleFindFiles(args);
      case 'read_file':
        return this.handleReadFile(args);
      case 'list_dir':
        return this.handleListDir(args);
      default:
        throw new Error(`Tool ${name} not supported by traditional provider`);
    }
  }

  async shutdown(): Promise<void> {
    // No-op for traditional provider
    return Promise.resolve();
  }

  private async handleGrep(args: Record<string, any>): Promise<string> {
    const query = args.query;
    if (!query) {
      throw new Error('grep tool requires a query parameter');
    }
    const searchPath = resolvePath(args.path);
    const flagArgs = args.flags ? args.flags.trim().split(/\s+/) : [];
    
    const cmdArgs = [
      '--color=never',
      '--line-number',
      '--with-filename',
      '--no-heading',
      ...flagArgs,
      query,
      searchPath
    ];

    try {
      const { stdout } = await execFileAsync('rg', cmdArgs, { cwd: workspaceRoot });
      const lines = stdout.split('\n').map(l => l.trim()).filter(Boolean);
      if (lines.length === 0) {
        return `No results found for query: '${query}'`;
      }

      const limit = 50;
      const results = lines.slice(0, limit);
      let out = `Found ${lines.length} results for '${query}':\n\n`;
      for (let i = 0; i < results.length; i++) {
        const line = results[i];
        const firstColon = line.indexOf(':');
        const secondColon = line.indexOf(':', firstColon + 1);
        if (firstColon !== -1 && secondColon !== -1) {
          const filePath = line.substring(0, firstColon);
          const relativeFilePath = path.relative(workspaceRoot, filePath);
          const lineNum = line.substring(firstColon + 1, secondColon);
          const content = line.substring(secondColon + 1);
          out += `${i + 1}. ${relativeFilePath}:L${lineNum}\n`;
          out += `   ---\n`;
          out += `   | ${content}\n`;
          out += `   ---\n\n`;
        } else {
          out += `${i + 1}. ${line}\n\n`;
        }
      }
      if (lines.length > limit) {
        out += `... and ${lines.length - limit} more matches truncated.\n`;
      }
      return out;
    } catch (e: any) {
      return `Error executing ripgrep: ${e.message}`;
    }
  }

  private async handleFindFiles(args: Record<string, any>): Promise<string> {
    const pattern = args.pattern;
    if (!pattern) {
      throw new Error('find_files tool requires a pattern parameter');
    }
    const searchDir = resolvePath(args.path);
    
    // Construct standard find arguments:
    // exclude .git (hidden files/folders), node_modules, and target
    const cmdArgs = [
      searchDir,
      '-type', 'f',
      '-not', '-path', '*/.*',
      '-not', '-path', '*/node_modules/*',
      '-not', '-path', '*/target/*',
      '-iname', `*${pattern}*`
    ];

    try {
      const { stdout } = await execFileAsync('find', cmdArgs, { cwd: workspaceRoot });
      const lines = stdout.split('\n').map(l => l.trim()).filter(Boolean);
      if (lines.length === 0) {
        return `No files found matching pattern: '${pattern}'`;
      }

      const limit = 50;
      const results = lines.slice(0, limit);
      let out = `Found ${lines.length} files matching '${pattern}':\n`;
      for (const file of results) {
        const relFile = path.relative(workspaceRoot, file);
        out += `${relFile}\n`;
      }
      if (lines.length > limit) {
        out += `... and ${lines.length - limit} more files truncated.\n`;
      }
      return out;
    } catch (e: any) {
      return `Error executing find: ${e.message}`;
    }
  }

  private handleReadFile(args: Record<string, any>): Promise<string> {
    const relativePath = args.file_path;
    if (!relativePath) {
      throw new Error('read_file tool requires a file_path parameter');
    }
    const filePath = resolvePath(relativePath);
    if (!fs.existsSync(filePath)) {
      return Promise.resolve(`Error: File not found at ${relativePath}`);
    }

    try {
      const content = fs.readFileSync(filePath, 'utf8');
      const lines = content.split('\n');
      const startLine = args.start_line ? Math.max(1, args.start_line) : 1;
      const endLine = args.end_line ? Math.min(lines.length, args.end_line) : lines.length;

      const slicedLines = lines.slice(startLine - 1, endLine);
      let out = `File: ${path.relative(workspaceRoot, filePath)} (lines ${startLine}-${endLine} of ${lines.length})\n`;
      out += `--------------------------------------------------\n`;
      for (let i = 0; i < slicedLines.length; i++) {
        const lineNum = startLine + i;
        out += `${lineNum.toString().padStart(5, ' ')} | ${slicedLines[i]}\n`;
      }
      out += `--------------------------------------------------\n`;
      return Promise.resolve(out);
    } catch (e: any) {
      return Promise.resolve(`Error reading file: ${e.message}`);
    }
  }

  private handleListDir(args: Record<string, any>): Promise<string> {
    const dirPath = resolvePath(args.path);
    if (!fs.existsSync(dirPath)) {
      return Promise.resolve(`Error: Directory not found at ${args.path || '.'}`);
    }

    try {
      const stat = fs.statSync(dirPath);
      if (!stat.isDirectory()) {
        return Promise.resolve(`Error: Path ${args.path || '.'} is a file, not a directory`);
      }

      const entries = fs.readdirSync(dirPath);
      let out = `Directory: ${path.relative(workspaceRoot, dirPath) || '.'}\n\n`;
      out += `Name`.padEnd(40) + ` | ` + `Type`.padEnd(10) + ` | ` + `Size (bytes)\n`;
      out += `-`.repeat(70) + `\n`;
      for (const entry of entries) {
        if (entry === '.git' || entry === 'node_modules' || entry === 'target') {
          continue;
        }
        const fullPath = path.join(dirPath, entry);
        try {
          const s = fs.statSync(fullPath);
          const typeStr = s.isDirectory() ? 'DIR' : 'FILE';
          const sizeStr = s.isDirectory() ? '-' : s.size.toString();
          out += `${entry.padEnd(40)} | ${typeStr.padEnd(10)} | ${sizeStr}\n`;
        } catch (e) {
          out += `${entry.padEnd(40)} | UNKNOWN    | -\n`;
        }
      }
      return Promise.resolve(out);
    } catch (e: any) {
      return Promise.resolve(`Error listing directory: ${e.message}`);
    }
  }
}
