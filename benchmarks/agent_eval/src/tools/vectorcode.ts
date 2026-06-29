import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import * as fs from 'fs';
import * as path from 'path';
import { ToolProvider, ToolDefinition } from './types.js';

export class VectorCodeProvider implements ToolProvider {
  readonly name = 'vectorcode' as const;
  private client: Client | null = null;
  private transport: StdioClientTransport | null = null;
  private tools: ToolDefinition[] = [];

  constructor(private binPath?: string, private workspaceDir?: string) {}

  async initialize(): Promise<void> {
    const resolvedBin = this.binPath || process.env.VECTORCODE_BIN || path.resolve('../../target/release/vectorcode');
    if (!fs.existsSync(resolvedBin)) {
      throw new Error(`VectorCode binary not found at: ${resolvedBin}. Please compile it using 'cargo build' first.`);
    }

    const args = ['serve', '--mcp'];
    if (this.workspaceDir) {
      args.push('--project-path', this.workspaceDir);
    }

    this.transport = new StdioClientTransport({
      command: resolvedBin,
      args,
      env: { ...process.env } as any,
    });

    this.client = new Client(
      { name: 'vectorcode-eval-harness', version: '1.0.0' },
      { capabilities: {} }
    );

    await this.client.connect(this.transport);
    const toolsResponse = await this.client.listTools();
    this.tools = toolsResponse.tools.map((t: any) => ({
      name: t.name,
      description: t.description,
      inputSchema: t.inputSchema || {}
    }));
  }

  listTools(): ToolDefinition[] {
    return this.tools;
  }

  async callTool(name: string, args: Record<string, any>): Promise<string> {
    if (!this.client) {
      throw new Error('VectorCodeProvider not initialized');
    }
    // Coerce workspaces from stringified JSON array to actual array.
    // LLMs sometimes pass workspaces as "[\"repo\"]" (string) instead of ["repo"] (array).
    if (args.workspaces && typeof args.workspaces === 'string') {
      try {
        const parsed = JSON.parse(args.workspaces);
        if (Array.isArray(parsed)) {
          args = { ...args, workspaces: parsed };
        }
      } catch {
        // Not valid JSON — leave as-is, let server handle it
      }
    }
    const response = await this.client.callTool({
      name,
      arguments: args
    });
    const content = (response.content as any[]) || [];
    return content
      .filter((c: any) => c.type === 'text')
      .map((c: any) => c.text)
      .join('\n');
  }

  async shutdown(): Promise<void> {
    if (this.client) {
      await this.client.close();
      this.client = null;
    }
    this.transport = null;
  }
}
