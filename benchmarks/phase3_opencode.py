#!/usr/bin/env python3
"""
Phase 3 Context Bloat Benchmark for VectorCode (OpenCode Go API edition).

Two-arm real agent simulator comparing grep+cat (Arm A) vs vec_search (Arm B)
on a global understanding task. Evaluates if the agent suffers from context bloat
and "Lost in the Middle".

Usage:
    export OPENCODE_API_KEY="your_api_key_here"
    python benchmarks/phase3_opencode.py --model kimi-k2.6
"""

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict

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
REPORT_PATH = RESULTS_DIR / "phase3_report.json"

DEFAULT_MODEL = "kimi-k2.6"
BASE_URL = "https://opencode.ai/zen/go/v1"

CONTEXT_TASK = (
    "Explica la arquitectura del sistema de embeddings de este proyecto: "
    "¿qué trait principal se usa, cuáles son sus métodos, qué proveedores lo implementan "
    "y en qué parte del código se instancia el proveedor según la configuración?"
)

SYSTEM_PROMPT = """You are an expert Rust developer agent.
Your task is to explore the codebase using the provided tools, understand the global architecture, and answer the question.
Do not guess the answer. Use the tools to find the actual code.
Once you have enough context, output ONLY your final answer.
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
        return out[:8000] # truncate
    except Exception as e:
        return f"Error executing bash: {e}"

def tool_vec_search(query: str) -> str:
    print(f"    [Tool] vec_search: {query}")
    cmd = [*_find_vectorcode(), "search", query, "--json", "--limit", "4"]
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
        {"role": "user", "content": CONTEXT_TASK}
    ]

    tools = []
    if arm_id == "A":
        tools = [
            {
                "type": "function",
                "function": {
                    "name": "execute_bash",
                    "description": "Execute a bash command (e.g. grep, find)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {"type": "string"}
                        },
                        "required": ["command"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read full contents of a file",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"}
                        },
                        "required": ["path"]
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
                    "description": "Semantic search over the codebase. Returns code snippets.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"}
                        },
                        "required": ["query"]
                    }
                }
            }
        ]

    total_prompt_tokens = 0
    generated_text = ""

    print(f"\n--- Starting Agent (Arm {arm_id}) ---")
    
    for step in range(15):
        response = client.chat.completions.create(
            model=model,
            messages=messages,
            tools=tools,
            temperature=0.0
        )
        
        msg = response.choices[0].message
        
        if response.usage:
            total_prompt_tokens += response.usage.prompt_tokens
        
        if msg.tool_calls:
            messages.append(msg)
            for tc in msg.tool_calls:
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
        else:
            generated_text = msg.content or ""
            print(f"    [Done] Generated answer ({len(generated_text)} chars)")
            break
            
    return {
        "total_tokens": total_prompt_tokens,
        "generated_text": generated_text,
    }

# ---------------------------------------------------------------------------
# Evaluator (AI Judge using the same LLM for simplicity)
# ---------------------------------------------------------------------------
def evaluate_quality(answer: str, client: OpenAI, model: str) -> float:
    judge_prompt = f"""Evaluate the following answer about the architecture of VectorCode.
    Award 1 point for each of the following (max 5 points):
    1. Mentions the trait `Embedder`.
    2. Mentions `src/embedder/mod.rs`.
    3. Mentions methods `embed` and `embed_batch`.
    4. Mentions providers `onnx`, `gemini`, `ollama`, `openai`.
    5. Mentions instantiation via `create_embedder_from_config` (or config file).
    
    Answer to evaluate:
    {answer}
    
    Respond with ONLY a number from 0 to 5.
    """
    try:
        response = client.chat.completions.create(
            model=model,
            messages=[{"role": "user", "content": judge_prompt}],
            temperature=0.0
        )
        score_text = response.choices[0].message.content.strip()
        return float(score_text) / 5.0
    except Exception as e:
        print(f"Error evaluating: {e}")
        return 0.0

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

    print(f"Running Phase 3 Benchmark on model: {args.model}")

    res_a = run_agent("A", args.model, client)
    res_b = run_agent("B", args.model, client)
    
    qa = evaluate_quality(res_a["generated_text"], client, args.model)
    qb = evaluate_quality(res_b["generated_text"], client, args.model)

    a_tokens = res_a["total_tokens"]
    b_tokens = res_b["total_tokens"]
    savings = (1 - b_tokens / a_tokens) * 100 if a_tokens else 0.0

    report = {
        "model": args.model,
        "arm_a": {
            "tokens": a_tokens,
            "quality": qa
        },
        "arm_b": {
            "tokens": b_tokens,
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
    print(f"Arm A (Grep): {a_tokens} tokens, {qa:.0%} quality")
    print(f"Arm B (Vec):  {b_tokens} tokens, {qb:.0%} quality")
    print(f"Token Savings: {savings:.1f}%")

if __name__ == "__main__":
    main()
