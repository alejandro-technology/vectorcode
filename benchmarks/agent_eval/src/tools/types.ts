export interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: Record<string, any>;
}

export interface ToolProvider {
  name: 'vectorcode' | 'traditional';
  initialize(): Promise<void>;
  listTools(): ToolDefinition[];
  callTool(name: string, args: Record<string, any>): Promise<string>;
  shutdown(): Promise<void>;
}
