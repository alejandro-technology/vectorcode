import { Anthropic } from '@anthropic-ai/sdk';
import { OpenAI } from 'openai';
import { Task, AgentConfig, ToolCallRecord, EvalResult } from './types.js';

export interface AgentRunner {
  run: (
    task: Task,
    config: AgentConfig,
    mcpTools: any[],
    callMcpTool: (name: string, args: any) => Promise<string>
  ) => Promise<{
    success: boolean;
    steps: number;
    tokens: { input: number; output: number; total: number };
    toolCalls: ToolCallRecord[];
    finalAnswer: string;
    error?: string;
  }>;
}

export const runAgent: AgentRunner['run'] = async (task, config, mcpTools, callMcpTool) => {
  const maxSteps = config.maxSteps || 10;
  const toolCalls: ToolCallRecord[] = [];
  let inputTokens = 0;
  let outputTokens = 0;
  let steps = 0;
  let finalAnswer = '';

  const apiKey = process.env.OPENCODE_API_KEY || process.env.OPENAI_API_KEY || process.env.ANTHROPIC_API_KEY || 'dummy';
  const baseURL = 'https://opencode.ai/zen/go/v1';

  // Dry-run mode for testing plumbing without API keys
  if (config.provider === 'dry-run') {
    console.log('[DryRun] Starting dry-run execution');
    // Simulate a vec_search tool call
    const start = Date.now();
    const searchResult = await callMcpTool('vec_search', { query: 'VectorCodeError', limit: 2 });
    toolCalls.push({
      toolName: 'vec_search',
      input: { query: 'VectorCodeError', limit: 2 },
      output: searchResult.substring(0, 100) + '...',
      durationMs: Date.now() - start
    });

    // Simulate creating a file for task-2
    if (task.id === 'task-2-write') {
      const fs = await import('fs');
      const path = await import('path');
      const mockFile = path.resolve(process.cwd(), '../../src/cli/status_mock.rs');
      fs.writeFileSync(mockFile, `
        pub fn run_status() -> String {
          "Mock Status: OK".to_string()
        }
      `);
    }

    const finalAnswer = task.id === 'task-2-write'
      ? 'pub fn run_status() -> String { "Mock Status: OK".to_string() }'
      : 'Dry-run execution completed successfully. VectorCodeError is defined in src/error.rs and sanitize_fts_query is in src/store/fts.rs.';

    return {
      success: true,
      steps: 1,
      tokens: { input: 150, output: 50, total: 200 },
      toolCalls,
      finalAnswer
    };
  }

  if (config.provider === 'anthropic') {
    // Anthropic SDK / Client
    const client = new Anthropic({
      apiKey,
      baseURL: 'https://opencode.ai/zen/go'
    });

    // Map MCP Tools to Anthropic Format
    const anthropicTools: Anthropic.Tool[] = mcpTools.map(t => ({
      name: t.name,
      description: t.description,
      input_schema: t.inputSchema
    }));

    let messages: Anthropic.MessageParam[] = [
      { role: 'user', content: task.prompt }
    ];

    while (steps < maxSteps) {
      steps++;
      console.log(`[Anthropic] Step ${steps}/${maxSteps}...`);

      const startMs = Date.now();
      const response = await client.messages.create({
        model: config.model,
        max_tokens: 4000,
        system: 'You are an elite coding agent with access to VectorCode MCP tools. Solve the task step-by-step. Use tools when needed.',
        messages,
        tools: anthropicTools
      });

      if (response.usage) {
        inputTokens += response.usage.input_tokens;
        outputTokens += response.usage.output_tokens;
      }

      // Collect text output
      let textContent = '';
      const toolUseBlocks: Anthropic.ToolUseBlock[] = [];

      for (const block of response.content) {
        if (block.type === 'text') {
          textContent += block.text;
        } else if (block.type === 'tool_use') {
          toolUseBlocks.push(block);
        }
      }

      console.log(`[Anthropic] Model text response: ${textContent.substring(0, 100)}...`);

      // Add assistant response to messages history
      messages.push({
        role: 'assistant',
        content: response.content
      });

      if (toolUseBlocks.length === 0) {
        // No more tool calls, we are done
        finalAnswer = textContent;
        break;
      }

      // Execute tool calls
      const toolResultBlocks: Anthropic.ToolResultBlockParam[] = [];
      for (const toolUse of toolUseBlocks) {
        const name = toolUse.name;
        const input = toolUse.input;
        const id = toolUse.id;

        console.log(`[Anthropic] Calling tool: ${name} with args:`, JSON.stringify(input));
        const toolStart = Date.now();
        let output = '';
        try {
          output = await callMcpTool(name, input);
        } catch (e: any) {
          output = `Error calling tool: ${e.message}`;
        }
        const duration = Date.now() - toolStart;

        toolCalls.push({
          toolName: name,
          input,
          output: output.substring(0, 500) + (output.length > 500 ? '...' : ''),
          durationMs: duration
        });

        toolResultBlocks.push({
          type: 'tool_result',
          tool_use_id: id,
          content: output
        });
      }

      messages.push({
        role: 'user',
        content: toolResultBlocks
      });
    }

    return {
      success: true,
      steps,
      tokens: { input: inputTokens, output: outputTokens, total: inputTokens + outputTokens },
      toolCalls,
      finalAnswer
    };
  }

  if (config.provider === 'openai') {
    // OpenAI SDK / Client
    const client = new OpenAI({
      apiKey,
      baseURL: 'https://opencode.ai/zen/go/v1'
    });

    // Map MCP Tools to OpenAI format
    const openaiTools: OpenAI.ChatCompletionTool[] = mcpTools.map(t => ({
      type: 'function',
      function: {
        name: t.name,
        description: t.description,
        parameters: t.inputSchema
      }
    }));

    let messages: OpenAI.ChatCompletionMessageParam[] = [
      { role: 'system', content: 'You are an elite coding agent with access to VectorCode MCP tools. Solve the task step-by-step. Use tools when needed.' },
      { role: 'user', content: task.prompt }
    ];

    while (steps < maxSteps) {
      steps++;
      console.log(`[OpenAI] Step ${steps}/${maxSteps}...`);

      const response = await client.chat.completions.create({
        model: config.model,
        messages,
        tools: openaiTools,
        tool_choice: 'auto'
      });

      if (response.usage) {
        inputTokens += response.usage.prompt_tokens;
        outputTokens += response.usage.completion_tokens;
      }

      const choice = response.choices[0];
      const assistantMessage = choice.message;

      // Add response to context history
      messages.push(assistantMessage);

      if (assistantMessage.content) {
        console.log(`[OpenAI] Model text response: ${assistantMessage.content.substring(0, 100)}...`);
      }

      if (!assistantMessage.tool_calls || assistantMessage.tool_calls.length === 0) {
        finalAnswer = assistantMessage.content || '';
        break;
      }

      // Execute tool calls
      for (const toolCall of assistantMessage.tool_calls) {
        const name = toolCall.function.name;
        let args = {};
        try {
          args = JSON.parse(toolCall.function.arguments);
        } catch (e) {}

        console.log(`[OpenAI] Calling tool: ${name} with args:`, toolCall.function.arguments);
        const toolStart = Date.now();
        let output = '';
        try {
          output = await callMcpTool(name, args);
        } catch (e: any) {
          output = `Error calling tool: ${e.message}`;
        }
        const duration = Date.now() - toolStart;

        toolCalls.push({
          toolName: name,
          input: args,
          output: output.substring(0, 500) + (output.length > 500 ? '...' : ''),
          durationMs: duration
        });

        messages.push({
          role: 'tool',
          tool_call_id: toolCall.id,
          content: output
        });
      }
    }

    return {
      success: true,
      steps,
      tokens: { input: inputTokens, output: outputTokens, total: inputTokens + outputTokens },
      toolCalls,
      finalAnswer
    };
  }

  throw new Error(`Unsupported provider: ${config.provider}`);
};
