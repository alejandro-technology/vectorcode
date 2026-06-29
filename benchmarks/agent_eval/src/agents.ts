import { Anthropic } from '@anthropic-ai/sdk';
import { OpenAI } from 'openai';
import * as fs from 'fs';
import * as path from 'path';
import { Task, AgentConfig, ToolCallRecord } from './types.js';
import { ToolProvider, ToolDefinition } from './tools/types.js';
import { CacheManager, CacheEntry, CacheMode, computeRequestHash, getGitSha } from './cache.js';

export interface ChatMessage {
  role: 'user' | 'assistant';
  content: string | ChatContentBlock[];
}

export type ChatContentBlock =
  | { type: 'text'; text: string }
  | { type: 'tool_use'; id: string; name: string; input: any }
  | { type: 'tool_result'; tool_use_id: string; content: string };

export interface LLMResponse {
  text: string;
  toolCalls: { name: string; args: Record<string, any>; id: string }[];
  tokens: { input: number; output: number };
  stopReason: 'end_turn' | 'tool_use' | 'max_tokens';
}

export type LLMCallFn = (messages: ChatMessage[], systemPrompt?: string) => Promise<LLMResponse>;

function mapMessagesToOpenAI(messages: ChatMessage[], systemPrompt?: string): OpenAI.ChatCompletionMessageParam[] {
  const result: OpenAI.ChatCompletionMessageParam[] = [];

  result.push({
    role: 'system',
    content: systemPrompt || 'You are an elite coding agent with access to VectorCode MCP tools. Solve the task step-by-step. Use tools when needed.'
  });
  
  for (const msg of messages) {
    if (msg.role === 'user') {
      if (typeof msg.content === 'string') {
        result.push({ role: 'user', content: msg.content });
      } else {
        for (const block of msg.content) {
          if (block.type === 'tool_result') {
            result.push({
              role: 'tool',
              tool_call_id: block.tool_use_id,
              content: block.content
            });
          } else if (block.type === 'text') {
            result.push({ role: 'user', content: block.text });
          }
        }
      }
    } else if (msg.role === 'assistant') {
      if (typeof msg.content === 'string') {
        result.push({ role: 'assistant', content: msg.content });
      } else {
        let textContent = '';
        const toolCalls: OpenAI.ChatCompletionMessageToolCall[] = [];
        
        for (const block of msg.content) {
          if (block.type === 'text') {
            textContent += block.text;
          } else if (block.type === 'tool_use') {
            toolCalls.push({
              id: block.id,
              type: 'function',
              function: {
                name: block.name,
                arguments: JSON.stringify(block.input)
              }
            });
          }
        }
        
        result.push({
          role: 'assistant',
          content: textContent || null,
          tool_calls: toolCalls.length > 0 ? toolCalls : undefined
        });
      }
    }
  }
  return result;
}

function mapMessagesToAnthropic(messages: ChatMessage[]): Anthropic.MessageParam[] {
  const result: Anthropic.MessageParam[] = [];
  
  for (const msg of messages) {
    if (msg.role === 'user') {
      if (typeof msg.content === 'string') {
        result.push({ role: 'user', content: msg.content });
      } else {
        const content: any[] = msg.content.map(block => {
          if (block.type === 'tool_result') {
            return {
              type: 'tool_result',
              tool_use_id: block.tool_use_id,
              content: block.content
            };
          } else {
            return {
              type: 'text',
              text: (block as any).text
            };
          }
        });
        result.push({ role: 'user', content });
      }
    } else if (msg.role === 'assistant') {
      if (typeof msg.content === 'string') {
        result.push({ role: 'assistant', content: msg.content });
      } else {
        const content: any[] = msg.content.map(block => {
          if (block.type === 'tool_use') {
            return {
              type: 'tool_use',
              id: block.id,
              name: block.name,
              input: block.input
            };
          } else {
            return {
              type: 'text',
              text: (block as any).text
            };
          }
        });
        result.push({ role: 'assistant', content });
      }
    }
  }
  return result;
}

async function openaiCall(
  messages: ChatMessage[],
  config: AgentConfig,
  tools: ToolDefinition[],
  systemPrompt?: string
): Promise<LLMResponse> {
  const apiKey = process.env.OPENCODE_API_KEY || process.env.OPENAI_API_KEY || 'dummy';
  const client = new OpenAI({
    apiKey,
    baseURL: 'https://opencode.ai/zen/go/v1'
  });

  const openaiTools: OpenAI.ChatCompletionTool[] = tools.map(t => ({
    type: 'function',
    function: {
      name: t.name,
      description: t.description,
      parameters: t.inputSchema
    }
  }));

  const openaiMessages = mapMessagesToOpenAI(messages, systemPrompt);
  
  const response = await client.chat.completions.create({
    model: config.model,
    messages: openaiMessages,
    temperature: config.temperature ?? 0,
    tools: openaiTools.length > 0 ? openaiTools : undefined,
    tool_choice: openaiTools.length > 0 ? 'auto' : undefined
  });
  
  const choice = response.choices[0];
  const assistantMessage = choice.message;
  
  const text = assistantMessage.content || '';
  const toolCalls = (assistantMessage.tool_calls || []).map(tc => {
    let args = {};
    try {
      args = JSON.parse(tc.function.arguments);
    } catch (e) {
      console.warn(`[OpenAI Adapter] Failed to parse tool arguments: ${tc.function.arguments}`);
    }
    return {
      id: tc.id,
      name: tc.function.name,
      args
    };
  });
  
  const tokens = {
    input: response.usage?.prompt_tokens || 0,
    output: response.usage?.completion_tokens || 0
  };
  
  let stopReason: 'end_turn' | 'tool_use' | 'max_tokens' = 'end_turn';
  if (choice.finish_reason === 'tool_calls') {
    stopReason = 'tool_use';
  } else if (choice.finish_reason === 'length') {
    stopReason = 'max_tokens';
  }
  
  return {
    text,
    toolCalls,
    tokens,
    stopReason
  };
}

async function anthropicCall(
  messages: ChatMessage[],
  config: AgentConfig,
  tools: ToolDefinition[],
  systemPrompt?: string
): Promise<LLMResponse> {
  const apiKey = process.env.OPENCODE_API_KEY || process.env.ANTHROPIC_API_KEY || 'dummy';
  const client = new Anthropic({
    apiKey,
    baseURL: 'https://opencode.ai/zen/go'
  });

  const anthropicTools: Anthropic.Tool[] = tools.map(t => ({
    name: t.name,
    description: t.description,
    input_schema: t.inputSchema as any
  }));

  const anthropicMessages = mapMessagesToAnthropic(messages);

  const response = await client.messages.create({
    model: config.model,
    max_tokens: 4000,
    temperature: config.temperature ?? 0,
    system: systemPrompt || 'You are an elite coding agent with access to VectorCode MCP tools. Solve the task step-by-step. Use tools when needed.',
    messages: anthropicMessages,
    tools: anthropicTools.length > 0 ? anthropicTools : undefined
  });
  
  let text = '';
  const toolCalls: { name: string; args: Record<string, any>; id: string }[] = [];
  
  for (const block of response.content) {
    if (block.type === 'text') {
      text += block.text;
    } else if (block.type === 'tool_use') {
      toolCalls.push({
        id: block.id,
        name: block.name,
        args: block.input as any
      });
    }
  }
  
  const tokens = {
    input: response.usage?.input_tokens || 0,
    output: response.usage?.output_tokens || 0
  };
  
  let stopReason: 'end_turn' | 'tool_use' | 'max_tokens' = 'end_turn';
  if (response.stop_reason === 'tool_use') {
    stopReason = 'tool_use';
  } else if (response.stop_reason === 'max_tokens') {
    stopReason = 'max_tokens';
  }
  
  return {
    text,
    toolCalls,
    tokens,
    stopReason
  };
}

function simulateDryRunResponse(task: Task, steps: number, toolsList: ToolDefinition[]): LLMResponse {
  if (steps === 1) {
    const hasSearch = toolsList.some(t => t.name === 'vec_search');
    const toolName = hasSearch ? 'vec_search' : 'grep';
    const args = hasSearch
      ? { query: task.prompt.substring(0, 50), limit: 2 }
      : { query: task.prompt.substring(0, 50) };

    return {
      text: "I will use search tools to find relevant code.",
      toolCalls: [{ name: toolName, args, id: 'call_dryrun_1' }],
      tokens: { input: 100, output: 50 },
      stopReason: 'tool_use'
    };
  } else {
    // For write tasks: return a stub implementation
    // For read tasks: return a generic placeholder answer
    const finalAnswer = task.type === 'write'
      ? '// Stub implementation generated by dry-run simulator\nexport function stub() { return "placeholder"; }'
      : 'Dry-run execution completed. The relevant code structures were identified via search tools.';

    return {
      text: finalAnswer,
      toolCalls: [],
      tokens: { input: 150, output: 50 },
      stopReason: 'end_turn'
    };
  }
}

export async function reactLoop(
  task: Task,
  provider: ToolProvider,
  llmCall: LLMCallFn,
  cacheManager: CacheManager,
  config: AgentConfig & { arm: 'vectorcode' | 'traditional'; cacheMode: CacheMode; corpus?: string; repetition?: number }
) {
  const maxSteps = config.maxSteps || 10;
  const timeoutMs = config.timeoutMs ?? 120000;
  const corpus = config.corpus || task.corpus || 'unknown';
  const repetition = config.repetition ?? 1;
  const toolCalls: ToolCallRecord[] = [];
  let inputTokens = 0;
  let outputTokens = 0;
  let steps = 0;
  let finalAnswer = '';

  const { cacheMode, model, arm } = config;
  const loaded = cacheMode !== 'dry-run' ? cacheManager.loadTrajectory(model, corpus, task.id, arm) : null;
  const cachedEntries = loaded?.entries || [];

  if (cacheMode === 'cached' && !loaded) {
    throw new Error(`No cached trajectory found for model ${model}, corpus ${corpus}, task ${task.id}, arm ${arm}`);
  }

  let messages: ChatMessage[] = [
    { role: 'user', content: task.prompt }
  ];

  // Build system prompt with optional targetRepos hint
  let systemPrompt: string | undefined;
  if (task.targetRepos && task.targetRepos.length > 0) {
    const repos = JSON.stringify(task.targetRepos);
    if (task.targetRepos.length >= 3) {
      // Cross-repo task: guide the agent to search each repo individually
      systemPrompt = `You are an elite coding agent with access to VectorCode MCP tools. Solve the task step-by-step. Use tools when needed.

IMPORTANT — CROSS-REPO SEARCH STRATEGY:
The task involves ${task.targetRepos.length} repositories: ${repos}.
When using vec_search, results are merged from ALL repos unless you scope them. To get useful results for each repository:

1. Make SEPARATE vec_search calls for EACH repository using the "workspaces" parameter. For example:
   - vec_search({ query: "public API entry point", workspaces: ["thiserror"] })
   - vec_search({ query: "public API entry point", workspaces: ["defu"] })
   - vec_search({ query: "public API entry point", workspaces: ["itsdangerous"] })
2. After gathering information per-repo, synthesize your findings into a comparison or cross-repo analysis.
3. Be efficient — you have limited steps. Make parallel calls when independent.
4. After gathering enough information, STOP exploring and produce your final answer. Do not use all available steps on exploration.`;
    } else {
      systemPrompt = `You are an elite coding agent with access to VectorCode MCP tools. Solve the task step-by-step. Use tools when needed.

IMPORTANT: When using vec_search, always pass the "workspaces" parameter with value ${repos} to scope your search to the relevant repositories.`;
    }
  }

  let isReplaying = cacheMode === 'cached' || (cacheMode === 'live' && cachedEntries.length > 0);
  const newEntries: CacheEntry[] = [];

  const loopPromise = (async () => {
    while (steps < maxSteps) {
      steps++;
      console.log(`[reactLoop] Step ${steps}/${maxSteps}... (Replay: ${isReplaying})`);

      const requestHash = computeRequestHash(messages);

      let response: LLMResponse = { text: '', toolCalls: [], tokens: { input: 0, output: 0 }, stopReason: 'end_turn' };
      let tokens: { input: number; output: number } = { input: 0, output: 0 };

      if (isReplaying) {
        const cachedEntry = cachedEntries[steps - 1];
        if (cachedEntry && cachedEntry.requestHash === requestHash) {
          console.log(`[Cache] Replaying step ${steps} from cache.`);
          response = cachedEntry.response;
          tokens = cachedEntry.tokens;
        } else {
          if (cacheMode === 'cached') {
            if (!cachedEntry) {
              throw new Error(`Cache divergence at step ${steps} in cached-only mode: no more cached entries to replay.`);
            }
            console.warn(`[Cache] Warning: Cache request hash diverged at step ${steps} (expected ${cachedEntry.requestHash} but got ${requestHash}). Continuing replay anyway.`);
            response = cachedEntry.response;
            tokens = cachedEntry.tokens;
          } else {
            console.log(`[Cache] Cache diverged at step ${steps}. Switching to live execution.`);
            isReplaying = false;
          }
        }
      }

      if (!isReplaying) {
        if (cacheMode === 'dry-run') {
          response = simulateDryRunResponse(task, steps, provider.listTools());
          tokens = { input: 150, output: 50 };
        } else {
          response = await llmCall(messages, systemPrompt);
          tokens = response.tokens;
        }

        if (cacheMode === 'live' || cacheMode === 'update-cache') {
          newEntries.push({
            stepIndex: steps,
            requestHash,
            response,
            tokens,
            timestamp: new Date().toISOString()
          });
        }
      } else {
        tokens = cachedEntries[steps - 1].tokens;
      }

      inputTokens += tokens.input;
      outputTokens += tokens.output;

      messages.push({
        role: 'assistant',
        content: [
          { type: 'text', text: response.text },
          ...response.toolCalls.map(tc => ({
            type: 'tool_use' as const,
            id: tc.id,
            name: tc.name,
            input: tc.args
          }))
        ]
      });

      if (response.toolCalls.length === 0) {
        finalAnswer = response.text;
        break;
      }

      const toolResults: ChatContentBlock[] = [];
      for (const toolCall of response.toolCalls) {
        const name = toolCall.name;
        const args = toolCall.args;
        const id = toolCall.id;

        console.log(`[reactLoop] Calling tool: ${name} with args:`, JSON.stringify(args));
        const toolStart = Date.now();
        let output = '';
        try {
          output = await provider.callTool(name, args);
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

        toolResults.push({
          type: 'tool_result',
          tool_use_id: id,
          content: output
        });
      }

      messages.push({
        role: 'user',
        content: toolResults
      });
    }

    // If the agent exhausted all steps without producing a final answer,
    // force one more LLM call (without tools) to synthesize findings.
    if (!finalAnswer && steps >= maxSteps) {
      console.log(`[reactLoop] Agent used all ${maxSteps} steps without final answer. Forcing synthesis...`);
      messages.push({
        role: 'user',
        content: [{ type: 'text', text: 'You have used all available steps. Stop using tools and synthesize everything you have found into your final answer now.' }]
      });
      const forcedResponse = await llmCall(messages, systemPrompt);
      finalAnswer = forcedResponse.text;
      inputTokens += forcedResponse.tokens.input;
      outputTokens += forcedResponse.tokens.output;
      steps++;
    }

    return { steps, finalAnswer, inputTokens, outputTokens, toolCalls };
  })();

  const timeoutPromise = new Promise<never>((_, reject) =>
    setTimeout(() => reject(new Error('TIMEOUT')), timeoutMs)
  );

  let timedOut = false;
  try {
    const result = await Promise.race([loopPromise, timeoutPromise]);
    steps = result.steps;
    finalAnswer = result.finalAnswer;
    inputTokens = result.inputTokens;
    outputTokens = result.outputTokens;
    // toolCalls already accumulated via closure
  } catch (e: any) {
    if (e.message === 'TIMEOUT') {
      timedOut = true;
      console.warn(`[reactLoop] TIMEOUT after ${timeoutMs}ms at step ${steps}`);
    } else {
      throw e;
    }
  }

  if (cacheMode === 'live' || cacheMode === 'update-cache') {
    const finalEntries = [
      ...cachedEntries.slice(0, steps - newEntries.length),
      ...newEntries
    ];

    cacheManager.saveTrajectory(model, corpus, task.id, arm, {
      workspaceSha: getGitSha(),
      model,
      taskId: task.id,
      arm,
      corpus,
      repetition,
      experimentConfig: {
        maxSteps,
        timeoutMs,
        temperature: config.temperature ?? 0,
      }
    }, finalEntries);
  }

  return {
    success: !timedOut,
    steps,
    tokens: { input: inputTokens, output: outputTokens, total: inputTokens + outputTokens },
    toolCalls,
    finalAnswer,
    timedOut
  };
}

export async function runAgent(
  task: Task,
  config: AgentConfig & { arm: 'vectorcode' | 'traditional'; cacheMode: CacheMode; corpus?: string; repetition?: number },
  provider: ToolProvider
) {
  const cacheManager = new CacheManager();
  const tools = provider.listTools();

  const llmCall = async (messages: ChatMessage[], systemPrompt?: string): Promise<LLMResponse> => {
    if (config.provider === 'openai') {
      return openaiCall(messages, config, tools, systemPrompt);
    } else if (config.provider === 'anthropic') {
      return anthropicCall(messages, config, tools, systemPrompt);
    } else {
      throw new Error(`Unsupported provider: ${config.provider}`);
    }
  };

  return reactLoop(task, provider, llmCall, cacheManager, config);
}

