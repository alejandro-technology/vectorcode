#!/usr/bin/env node
/**
 * Phase 2 Session Parser — analyzes JSONL session logs from the agent simulator.
 *
 * Reads a JSONL session log, computes token totals, exploration metrics,
 * and outputs a structured JSON summary to stdout.
 *
 * Usage:
 *   node benchmarks/agent-eval/parse-session.mjs <path-to-jsonl>
 *   node benchmarks/agent-eval/parse-session.mjs benchmarks/results/session_arm_a.log
 *
 * Output (JSON to stdout):
 *   {
 *     "total_input_tokens": 1234,
 *     "exploration_tokens": 987,
 *     "exploration_steps_before_generation": 5,
 *     "step_count": 6,
 *     "tools_used": ["grep", "read_file"]
 *   }
 *
 * Exit codes:
 *   0 — success
 *   1 — file not found or parse error
 */

import { readFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";
import { strict as assert } from "node:assert";

/**
 * Parse a JSONL file into an array of step objects.
 * Skips malformed lines with a warning to stderr.
 *
 * @param {string} filePath - Path to the JSONL file.
 * @returns {Array<object>} Parsed step objects.
 */
export function parseJsonl(filePath) {
  const content = readFileSync(filePath, "utf-8");
  const lines = content.split("\n");
  const entries = [];
  let warnings = 0;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (!line) continue;

    try {
      const entry = JSON.parse(line);
      entries.push(entry);
    } catch {
      warnings++;
      console.warn(`WARNING: malformed JSON at line ${i + 1}, skipping`);
    }
  }

  return { entries, warnings };
}

/**
 * Compute metrics from parsed session entries.
 *
 * @param {Array<object>} entries - Parsed JSONL entries.
 * @returns {object} Metrics summary.
 */
export function computeMetrics(entries) {
  let totalInputTokens = 0;
  let explorationTokens = 0;
  let explorationStepsBeforeGeneration = 0;
  const toolsUsed = new Set();
  let foundFirstNonExploration = false;

  for (const entry of entries) {
    // Sum tokens
    const tokens = entry.response_tokens ?? entry.token_count ?? 0;
    totalInputTokens += tokens;

    // Track tools
    if (entry.tool) {
      toolsUsed.add(entry.tool);
    }

    // Count exploration steps before first generation
    if (entry.is_exploration === true && !foundFirstNonExploration) {
      explorationStepsBeforeGeneration++;
      explorationTokens += tokens;
    } else if (entry.is_exploration === true && foundFirstNonExploration) {
      // Exploration step after generation — still counts for exploration tokens
      explorationTokens += tokens;
    } else {
      foundFirstNonExploration = true;
    }
  }

  return {
    total_input_tokens: totalInputTokens,
    exploration_tokens: explorationTokens,
    exploration_steps_before_generation: explorationStepsBeforeGeneration,
    step_count: entries.length,
    tools_used: [...toolsUsed].sort(),
  };
}

/**
 * Main CLI entry point.
 */
function main() {
  const filePath = process.argv[2];

  if (!filePath) {
    console.error("Usage: node parse-session.mjs <path-to-jsonl>");
    process.exit(1);
  }

  const resolvedPath = resolve(filePath);

  if (!existsSync(resolvedPath)) {
    console.error(`Error: file not found: ${resolvedPath}`);
    process.exit(1);
  }

  const { entries, warnings } = parseJsonl(resolvedPath);
  const metrics = computeMetrics(entries);

  if (warnings > 0) {
    metrics.parser_warnings = warnings;
  }

  console.log(JSON.stringify(metrics, null, 2));
}

// ---------------------------------------------------------------------------
// Inline tests (run with --test flag)
// ---------------------------------------------------------------------------

function runTests() {
  console.log("Running parse-session.mjs tests...\n");
  let passed = 0;

  // Test 1: computeMetrics with known entries
  {
    const entries = [
      { step: 1, tool: "grep", response_tokens: 42, is_exploration: true },
      { step: 2, tool: "read_file", response_tokens: 150, is_exploration: true },
      { step: 3, tool: "generate", response_tokens: 88, is_exploration: false },
    ];
    const m = computeMetrics(entries);
    assert.equal(m.total_input_tokens, 280, "total tokens should be 280");
    assert.equal(m.exploration_tokens, 192, "exploration tokens should be 192");
    assert.equal(m.exploration_steps_before_generation, 2, "should have 2 exploration steps");
    assert.equal(m.step_count, 3, "should have 3 steps");
    assert.deepEqual(m.tools_used, ["grep", "read_file", "generate"].sort(), "tools should match");
    passed++;
    console.log("  PASS: computeMetrics sums tokens correctly");
  }

  // Test 2: empty entries
  {
    const m = computeMetrics([]);
    assert.equal(m.total_input_tokens, 0);
    assert.equal(m.exploration_tokens, 0);
    assert.equal(m.exploration_steps_before_generation, 0);
    assert.equal(m.step_count, 0);
    assert.deepEqual(m.tools_used, []);
    passed++;
    console.log("  PASS: computeMetrics handles empty entries");
  }

  // Test 3: all exploration (no generation step)
  {
    const entries = [
      { step: 1, tool: "grep", response_tokens: 10, is_exploration: true },
      { step: 2, tool: "grep", response_tokens: 20, is_exploration: true },
    ];
    const m = computeMetrics(entries);
    assert.equal(m.exploration_steps_before_generation, 2);
    assert.equal(m.exploration_tokens, 30);
    assert.equal(m.total_input_tokens, 30);
    passed++;
    console.log("  PASS: computeMetrics handles all-exploration session");
  }

  // Test 4: exploration efficiency ratio
  {
    const armA = { exploration_tokens: 5000 };
    const armB = { exploration_tokens: 2000 };
    const efficiency = armB.exploration_tokens / armA.exploration_tokens;
    assert.equal(efficiency, 0.4, "efficiency should be 0.4");
    passed++;
    console.log("  PASS: exploration efficiency ratio = 0.4");
  }

  // Test 5: missing token_count field defaults to 0
  {
    const entries = [
      { step: 1, tool: "grep", is_exploration: true },
    ];
    const m = computeMetrics(entries);
    assert.equal(m.total_input_tokens, 0, "missing tokens should default to 0");
    passed++;
    console.log("  PASS: missing token field defaults to 0");
  }

  console.log(`\nAll ${passed} tests passed!\n`);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

if (process.argv.includes("--test")) {
  runTests();
} else {
  main();
}
