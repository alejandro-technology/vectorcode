#!/usr/bin/env python3
"""
Phase 2 Token Savings Benchmark for VectorCode (OpenCode Go API edition).

Two-arm real agent simulator comparing bash-based vs VectorCode-search-based
discovery. Measures ACTUAL input tokens consumed and tool calls made when
a real LLM (kimi-k2.6) explores the repository to generate code.

Arm A: execute_bash (grep, find, cat)
Arm B: vec_search + read_file

Usage:
    export OPENCODE_API_KEY="your_api_key_here"
    python benchmarks/phase2_opencode.py --model kimi-k2.6
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Tuple

try:
    from openai import OpenAI
except ImportError:
    print("Please install openai: pip install openai>=1.0.0")
    sys.exit(1)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
RESULTS_DIR = SCRIPT_DIR / "results"
REPORT_PATH = RESULTS_DIR / "phase2_report.json"

DEFAULT_MODEL = "kimi-k2.6"
BASE_URL = "https://opencode.ai/zen/go/v1"

IMITATION_TASK = (
    "Add a new CLI `status` subcommand that displays index health statistics, "
    "following the exact same conventions as the existing `install` CLI subcommand "
    "in `src/cli/install.rs`."
)

SYSTEM_PROMPT = """You are an expert Rust developer agent.
Your task is to explore the codebase using the provided tools, understand the patterns, and generate the requested code.
Do not guess the code. Use the tools to find existing conventions.
Once you have enough context, output ONLY the final Rust code. Do not wrap it in markdown block if possible, or just provide the final code without conversational filler.
"""

# ---------------------------------------------------------------------------
# Tools Definitions
# ---------------------------------------------------------------------------
def _find_vectorcode() -> list[str]:
    env_bin = os.environ.get("VECTORCODE_BIN")
    if env_bin:
        return [env_bin]
    import shutil
    if shutil.which("vectorcode"):
        return ["vectorcode"]
    return ["cargo", "run", "--quiet", "--"]

def tool_execute_bash(command: str) -> str:
    print(f"    [Tool] bash: {command}")
    try:
        res = subprocess.run(command, cwd=REPO_ROOT, shell=True, capture_output=True, text=True, timeout=15.0)
        out = res.stdout
        if res.stderr:
            out += f"\nSTDERR:\n{res.stderr}"
        if not out:
            out = "(Command executed successfully with no output)"
        return out[:8000] # truncate to avoid blowing up context
    except Exception as e:
        return f"Error executing bash: {e}"

def tool_vec_search(query: str) -> str:
    print(f"    [Tool] vec_search: {query}")
    cmd = [*_find_vectorcode(), "search", query, "--json", "--limit", "3"]
    try:
        res = subprocess.run(cmd, cwd=REPO_ROOT, capture_output=True, text=True, timeout=30.0)
        return res.stdout[:8000]
    except Exception as e:
        return f"Error searching: {e}"

def tool_read_file(path: str) -> str:
    print(f"    [Tool] read_file: {path}")
    full_path = REPO_ROOT / path
    try:
        return full_path.read_text(encoding="utf-8")[:8000]
    except Exception as e:
        return f"Error reading file: {e}"

# ---------------------------------------------------------------------------
# Agent Runner
# ---------------------------------------------------------------------------
def run_agent(arm_id: str, model: str, client: OpenAI) -> Dict[str, Any]:
    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": IMITATION_TASK}
    ]

    tools = []
    if arm_id == "A":
        tools = [
            {
                "type": "function",
                "function": {
                    "name": "execute_bash",
                    "description": "Execute a bash command in the repository root (e.g., grep, cat, find).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {"type": "string", "description": "The bash command to run"}
                        },
                        "required": ["command"]
                    }
                }
            }
        ]
    else:
        tools = [
            {
                "type": "function",
                "function": {
                    "name": "vec_search",
                    "description": "Semantic search over the codebase. Returns relevant code snippets and paths.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string", "description": "Natural language query"}
                        },
                        "required": ["query"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read the full contents of a file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string", "description": "Relative file path"}
                        },
                        "required": ["path"]
                    }
                }
            }
        ]

    total_prompt_tokens = 0
    total_completion_tokens = 0
    tool_calls_count = 0
    generated_code = ""
    step_log = []

    print(f"\n--- Starting Agent (Arm {arm_id}) ---")
    
    for step in range(15): # Max 15 turns
        response = client.chat.completions.create(
            model=model,
            messages=messages,
            tools=tools,
            temperature=0.0
        )
        
        msg = response.choices[0].message
        
        if response.usage:
            total_prompt_tokens += response.usage.prompt_tokens
            total_completion_tokens += response.usage.completion_tokens
        
        if msg.tool_calls:
            messages.append(msg)
            for tc in msg.tool_calls:
                tool_calls_count += 1
                args = json.loads(tc.function.arguments)
                if tc.function.name == "execute_bash":
                    res = tool_execute_bash(args.get("command", ""))
                elif tc.function.name == "vec_search":
                    res = tool_vec_search(args.get("query", ""))
                elif tc.function.name == "read_file":
                    res = tool_read_file(args.get("path", ""))
                else:
                    res = "Unknown tool."
                
                messages.append({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "name": tc.function.name,
                    "content": res
                })
                step_log.append({"tool": tc.function.name, "args": args, "response_chars": len(res)})
        else:
            # Got final answer
            generated_code = msg.content or ""
            print(f"    [Done] Generated code ({len(generated_code)} chars)")
            break
            
    return {
        "steps": step_log,
        "total_tokens": total_prompt_tokens,
        "total_completion_tokens": total_completion_tokens,
        "tool_calls_count": tool_calls_count,
        "generated_code": generated_code,
    }

# ---------------------------------------------------------------------------
# Evaluator
# ---------------------------------------------------------------------------
def evaluate_quality(code: str) -> float:
    # Basic proxy evaluation. A real test would compile it.
    score = 0.0
    if "use anyhow::Result" in code: score += 1
    if "use clap::Args" in code or "use clap::" in code: score += 1
    if "#[derive(Args" in code or "#[derive(Debug, Args" in code: score += 1
    if "pub fn execute" in code: score += 1
    if "#[cfg(test)]" in code: score += 1
    return score / 5.0

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=DEFAULT_MODEL, help="Model to use")
    args = parser.parse_args()

    api_key = os.environ.get("OPENCODE_API_KEY")
    if not api_key:
        print("ERROR: OPENCODE_API_KEY environment variable is required.")
        sys.exit(1)

    client = OpenAI(base_url=BASE_URL, api_key=api_key)
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    print(f"Running Phase 2 Benchmark on model: {args.model}")

    res_a = run_agent("A", args.model, client)
    res_b = run_agent("B", args.model, client)
    
    qa = evaluate_quality(res_a["generated_code"])
    qb = evaluate_quality(res_b["generated_code"])

    a_tokens = res_a["total_tokens"]
    b_tokens = res_b["total_tokens"]
    savings = (1 - b_tokens / a_tokens) * 100 if a_tokens else 0.0

    report = {
        "model": args.model,
        "arm_a": {
            "tokens": a_tokens,
            "tool_calls": res_a["tool_calls_count"],
            "quality": qa
        },
        "arm_b": {
            "tokens": b_tokens,
            "tool_calls": res_b["tool_calls_count"],
            "quality": qb
        },
        "metrics": {
            "token_savings_percent": round(savings, 2),
            "arm_a_quality_score": qa,
            "arm_b_quality_score": qb,
        }
    }

    with open(REPORT_PATH, "w") as f:
        json.dump(report, f, indent=2)

    print(f"\nRESULTS:")
    print(f"Arm A (Grep): {a_tokens} tokens, {res_a['tool_calls_count']} tools, {qa:.0%} quality")
    print(f"Arm B (Vec):  {b_tokens} tokens, {res_b['tool_calls_count']} tools, {qb:.0%} quality")
    print(f"Token Savings: {savings:.1f}%")

if __name__ == "__main__":
    main()
