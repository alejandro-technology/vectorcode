import { OpenAI } from 'openai';
import * as fs from 'fs';
import * as path from 'path';
import * as crypto from 'crypto';
import { TaskRubric, JudgeResult } from './types.js';

function computeHash(taskId: string, answer: string, rubricStr: string): string {
  const content = `${taskId}:${answer}:${rubricStr}`;
  return crypto.createHash('sha256').update(content).digest('hex');
}

export async function judge(
  taskId: string,
  prompt: string,
  agentAnswer: string,
  rubric: TaskRubric,
  judgeModel: string = 'mimo-v2.5'
): Promise<JudgeResult> {
  const rubricStr = JSON.stringify(rubric);
  const hash = computeHash(taskId, agentAnswer, rubricStr);
  
  // Resolve cache directory
  const cacheDir = path.resolve(process.cwd(), 'cache/judge');
  const cachePath = path.join(cacheDir, `${taskId}_${hash}.json`);

  // Check cache first
  if (fs.existsSync(cachePath)) {
    try {
      const cachedContent = fs.readFileSync(cachePath, 'utf8');
      console.log(`[Judge] Cache hit for task ${taskId}`);
      return JSON.parse(cachedContent);
    } catch (e) {
      console.warn(`[Judge] Failed to read judge cache for task ${taskId}:`, e);
    }
  }

  // Check if API key is mock/missing or running dry-run
  const apiKey = process.env.OPENCODE_API_KEY || process.env.OPENAI_API_KEY || process.env.ANTHROPIC_API_KEY;
  if (!apiKey || process.argv.includes('--dry-run') || judgeModel === 'dry-run-model') {
    console.log(`[Judge] Dry-run/Mock mode: skipping LLM call for task ${taskId}`);
    const mockResult: JudgeResult = {
      score: 1.0,
      criteriaScores: {},
      overallReasoning: 'Mock evaluation passed (Dry-Run)'
    };
    for (const c of rubric.criteria) {
      mockResult.criteriaScores[c.name] = {
        score: 1.0,
        reasoning: 'Mock criterion passed (Dry-Run)'
      };
    }
    return mockResult;
  }

  console.log(`[Judge] Calling judge model ${judgeModel} for task ${taskId}...`);

  const client = new OpenAI({
    apiKey,
    baseURL: 'https://opencode.ai/zen/go/v1'
  });

  const systemPrompt = `You are an expert AI code reviewer. Your job is to grade an agent's answer against a set of criteria and ground truth.
You must respond with a JSON object containing:
{
  "criteria": {
    "criterion_name": {
      "score": 1.0, // float between 0.0 and 1.0
      "reasoning": "Reasoning for this specific score..."
    }
  },
  "overallReasoning": "Summarize overall grading..."
}
Do NOT return any other text, only the raw JSON.`;

  const userPrompt = `## Task ID
${taskId}

## Task Prompt
${prompt}

## Agent's Answer
${agentAnswer}

## Evaluation Criteria
${rubric.criteria.map(c => `- Name: ${c.name}\n  Description: ${c.description}\n  Ground Truth: ${c.groundTruth}`).join('\n')}`;

  try {
    const response = await client.chat.completions.create({
      model: judgeModel,
      messages: [
        { role: 'system', content: systemPrompt },
        { role: 'user', content: userPrompt }
      ],
      response_format: { type: 'json_object' }
    });

    const responseText = response.choices[0].message.content || '{}';
    let parsedJson: any;
    try {
      parsedJson = JSON.parse(responseText);
    } catch (e) {
      // Fallback for models that markdown-wrap json even when response_format is set
      const jsonMatch = responseText.match(/\{[\s\S]*\}/);
      if (jsonMatch) {
        parsedJson = JSON.parse(jsonMatch[0]);
      } else {
        throw new Error(`Failed to parse judge JSON response: ${responseText}`);
      }
    }

    // Compute weighted score
    let totalWeight = 0;
    let weightedScore = 0;
    const criteriaScores: Record<string, { score: number; reasoning: string }> = {};

    for (const criterion of rubric.criteria) {
      const graded = parsedJson.criteria?.[criterion.name];
      const score = typeof graded?.score === 'number' ? graded.score : 0.0;
      const reasoning = graded?.reasoning || 'No reasoning provided';
      
      criteriaScores[criterion.name] = { score, reasoning };
      weightedScore += score * criterion.weight;
      totalWeight += criterion.weight;
    }

    const finalScore = totalWeight > 0 ? (weightedScore / totalWeight) : 0.0;

    const result: JudgeResult = {
      score: parseFloat(finalScore.toFixed(4)),
      criteriaScores,
      overallReasoning: parsedJson.overallReasoning || 'Grading completed'
    };

    // Save to cache
    if (!fs.existsSync(cacheDir)) {
      fs.mkdirSync(cacheDir, { recursive: true });
    }
    fs.writeFileSync(cachePath, JSON.stringify(result, null, 2), 'utf8');

    return result;
  } catch (err: any) {
    console.error(`[Judge] LLM-as-Judge call failed for task ${taskId}:`, err);
    throw err;
  }
}
