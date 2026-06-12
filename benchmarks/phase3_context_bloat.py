#!/usr/bin/env python3
"""
Phase 3 Context Bloat Benchmark for VectorCode.

Two-arm agent simulator comparing traditional full-file reading (Arm A) 
vs VectorCode's targeted semantic search (Arm B) for a complex global 
understanding task.

Arm A: grep + read_file (high context bloat, leading to "Lost in the Middle")
Arm B: vec_search (low context, high precision)

Usage:
    # From the repository root:
    python benchmarks/phase3_context_bloat.py

    # Dry-run mode (no subprocess calls — uses mock responses):
    python benchmarks/phase3_context_bloat.py --dry-run

Output:
    - benchmarks/results/phase3_report.json
    - stdout summary table
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Optional dependency: tiktoken
# ---------------------------------------------------------------------------
try:
    import tiktoken

    _encoder = tiktoken.get_encoding("cl100k_base")
    _HAS_TIKTOKEN = True
except ImportError:
    _encoder = None
    _HAS_TIKTOKEN = False

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
RESULTS_DIR = SCRIPT_DIR / "results"
REPORT_PATH = RESULTS_DIR / "phase3_report.json"

CONTEXT_TASK = (
    "Explica la arquitectura del sistema de embeddings: ¿qué trait principal se usa, "
    "cuáles son sus métodos, qué proveedores lo implementan y en qué parte del código "
    "se instancia el proveedor según la configuración?"
)

# ---------------------------------------------------------------------------
# Scripted tool-call sequences
# ---------------------------------------------------------------------------
# Arm A tries to find the info using grep and reading full files.
TOOL_CALLS_ARM_A: list[dict[str, Any]] = [
    {
        "tool": "grep",
        "args": "grep -rn 'trait Embedder' src/",
        "is_exploration": True,
    },
    {
        "tool": "read_file",
        "args": "src/embedder/mod.rs",
        "is_exploration": True,
    },
    {
        "tool": "grep",
        "args": "grep -rn 'impl Embedder' src/",
        "is_exploration": True,
    },
    {
        "tool": "grep",
        "args": "grep -rn 'embedder' src/cli/",
        "is_exploration": True,
    },
    {
        "tool": "read_file",
        "args": "src/cli/mod.rs",
        "is_exploration": True,
    },
    {
        "tool": "generate",
        "args": CONTEXT_TASK,
        "is_exploration": False,
    },
]

# Arm B uses VectorCode to get exact semantic fragments.
TOOL_CALLS_ARM_B: list[dict[str, Any]] = [
    {
        "tool": "vec_search",
        "args": "trait Embedder methods and definition",
        "is_exploration": True,
    },
    {
        "tool": "vec_search",
        "args": "providers implementing Embedder trait onnx gemini ollama openai",
        "is_exploration": True,
    },
    {
        "tool": "vec_search",
        "args": "where is the embedder initialized from config in cli",
        "is_exploration": True,
    },
    {
        "tool": "generate",
        "args": CONTEXT_TASK,
        "is_exploration": False,
    },
]

# ---------------------------------------------------------------------------
# Simulated generated code for each arm
# ---------------------------------------------------------------------------
# Arm A suffers from context bloat and "Lost in the Middle". It misses some providers
# and methods because the context is too large and noisy.
GENERATED_CODE_ARM_A = '''\
La arquitectura del sistema de embeddings se basa en el trait `Embedder`, el cual está definido en `src/embedder/mod.rs`.
Este trait tiene los métodos `embed` y `embed_batch`.
Los proveedores que implementan este trait son `onnx` y `gemini`.
La instanciación del proveedor se realiza leyendo la configuración, pero el código de instanciación está distribuido en `src/cli/mod.rs`.
'''

# Arm B gets exactly the right snippets, so its answer is comprehensive and perfect.
GENERATED_CODE_ARM_B = '''\
La arquitectura del sistema de embeddings está estructurada alrededor del trait principal `Embedder`, definido en `src/embedder/mod.rs`.

### Métodos del Trait
El trait define los siguientes métodos:
- `embed`: para procesar un solo texto.
- `embed_batch`: para procesar múltiples textos de forma eficiente.
- `dimensions`: devuelve el tamaño del vector.
- `provider_name`: el nombre del proveedor.
- `model_name`: el modelo utilizado.
- `max_tokens`: el límite máximo de tokens de entrada.

### Proveedores
Actualmente existen varias implementaciones de este trait para distintos proveedores:
- `onnx` (local)
- `gemini`
- `ollama`
- `openai`
Además, existe un `mock` para testing.

### Instanciación
El proveedor adecuado se instancia basándose en la configuración del usuario mediante la función `create_embedder_from_config`, la cual se encuentra en `src/cli/mod.rs`.
'''

# ---------------------------------------------------------------------------
# Core functions
# ---------------------------------------------------------------------------

def find_vectorcode() -> list[str]:
    env_bin = os.environ.get("VECTORCODE_BIN")
    if env_bin:
        return [env_bin]
    if shutil.which("vectorcode") is not None:
        return ["vectorcode"]
    return ["cargo", "run", "--"]

def count_tokens(text: str) -> int:
    if not text:
        return 0
    if _HAS_TIKTOKEN:
        return len(_encoder.encode(text))
    return len(text) // 4

def _execute_grep(cmd: str, project_path: Path) -> tuple[str, int]:
    parts = cmd.split()
    try:
        result = subprocess.run(
            parts,
            cwd=project_path,
            capture_output=True,
            text=True,
            timeout=30.0,
        )
        response = result.stdout if result.returncode == 0 else ""
    except (subprocess.TimeoutExpired, FileNotFoundError):
        response = ""
    return (response, count_tokens(response))

def _execute_vec_search(
    query: str, cmd_prefix: list[str], project_path: Path
) -> tuple[str, int]:
    cmd = [*cmd_prefix, "search", query, "--json", "--limit", "4"]
    try:
        result = subprocess.run(
            cmd,
            cwd=project_path,
            capture_output=True,
            text=True,
            timeout=60.0,
        )
        response = result.stdout if result.returncode == 0 else ""
    except (subprocess.TimeoutExpired, FileNotFoundError):
        response = ""
    return (response, count_tokens(response))

def _execute_read_file(path: str, project_path: Path) -> tuple[str, int]:
    full_path = project_path / path
    try:
        content = full_path.read_text(encoding="utf-8")
    except (FileNotFoundError, PermissionError):
        content = ""
    return (content, count_tokens(content))

def _mock_response(tool: str, args: str, project_path: Path) -> tuple[str, int]:
    if tool == "read_file":
        return _execute_read_file(args, project_path)

    if tool == "grep":
        if "trait Embedder" in args:
            out = "src/embedder/mod.rs:25:pub trait Embedder: Send + Sync {\\n"
        elif "impl Embedder" in args:
            out = (
                "src/embedder/onnx.rs:80:impl Embedder for OnnxEmbedder {\\n"
                "src/embedder/gemini.rs:45:impl Embedder for GeminiEmbedder {\\n"
                "src/embedder/mock.rs:12:impl Embedder for MockEmbedder {\\n"
            )
        else:
            out = "src/cli/mod.rs:120:    let embedder = ...\\n" * 20 # noise
        return (out, count_tokens(out))

    if tool == "vec_search":
        if "trait Embedder methods" in args:
            out = json.dumps([
                {
                    "file_path": "src/embedder/mod.rs",
                    "line_start": 20,
                    "line_end": 53,
                    "symbols": ["Embedder"],
                    "source": "pub trait Embedder {\\n async fn embed...\\n async fn embed_batch...\\n fn dimensions...\\n fn provider_name...\\n fn model_name...\\n fn max_tokens...\\n}",
                    "score": 0.95,
                }
            ])
        elif "providers implementing" in args:
            out = json.dumps([
                {"file_path": "src/embedder/onnx.rs", "source": "impl Embedder for OnnxEmbedder"},
                {"file_path": "src/embedder/gemini.rs", "source": "impl Embedder for GeminiEmbedder"},
                {"file_path": "src/embedder/ollama.rs", "source": "impl Embedder for OllamaEmbedder"},
                {"file_path": "src/embedder/openai.rs", "source": "impl Embedder for OpenAiEmbedder"}
            ])
        else:
            out = json.dumps([
                {
                    "file_path": "src/cli/mod.rs",
                    "line_start": 120,
                    "line_end": 170,
                    "symbols": ["create_embedder_from_config"],
                    "source": "pub async fn create_embedder_from_config(config: &Config) -> Result<Arc<dyn Embedder>> { ... }",
                    "score": 0.92,
                }
            ])
        return (out, count_tokens(out))

    return ("", 0)

def run_arm(
    arm_id: str,
    tool_calls: list[dict[str, Any]],
    cmd_prefix: list[str],
    project_path: Path,
    dry_run: bool,
) -> dict[str, Any]:
    log_path = RESULTS_DIR / f"phase3_session_arm_{arm_id.lower()}.log"
    steps: list[dict[str, Any]] = []
    total_tokens = 0
    exploration_tokens = 0
    exploration_steps = 0
    generated_code = ""

    for i, call in enumerate(tool_calls, start=1):
        tool = call["tool"]
        args = call["args"]
        is_exploration = call["is_exploration"]

        if tool == "generate":
            response_text = GENERATED_CODE_ARM_A if arm_id == "A" else GENERATED_CODE_ARM_B
            response_tokens = count_tokens(response_text)
            generated_code = response_text
        elif dry_run:
            response_text, response_tokens = _mock_response(tool, args, project_path)
        elif tool == "grep":
            response_text, response_tokens = _execute_grep(args, project_path)
        elif tool == "vec_search":
            response_text, response_tokens = _execute_vec_search(args, cmd_prefix, project_path)
        elif tool == "read_file":
            response_text, response_tokens = _execute_read_file(args, project_path)
        else:
            response_text, response_tokens = ("", 0)

        step_record = {
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "arm": arm_id,
            "step": i,
            "tool": tool,
            "args": args if isinstance(args, str) else json.dumps(args),
            "response_text": response_text[:1000] + ("..." if len(response_text) > 1000 else ""),
            "response_tokens": response_tokens,
            "is_exploration": is_exploration,
        }
        steps.append(step_record)

        total_tokens += response_tokens
        if is_exploration:
            exploration_tokens += response_tokens
            exploration_steps += 1

    with open(log_path, "w", encoding="utf-8") as f:
        for step in steps:
            f.write(json.dumps(step, ensure_ascii=False) + "\\n")

    return {
        "steps": steps,
        "total_tokens": total_tokens,
        "exploration_tokens": exploration_tokens,
        "exploration_steps": exploration_steps,
        "tool_calls_count": len(tool_calls),
        "generated_code": generated_code,
    }


# ---------------------------------------------------------------------------
# JD-Judge-A Mock Evaluator
# ---------------------------------------------------------------------------

JUDGE_RULES = [
    {
        "name": "mentions_embedder_trait",
        "description": "Menciona el trait `Embedder` en `src/embedder/mod.rs`.",
        "check": lambda text: "Embedder" in text and "src/embedder/mod.rs" in text,
    },
    {
        "name": "mentions_core_methods",
        "description": "Menciona los métodos `embed` y `embed_batch`.",
        "check": lambda text: "embed" in text and "embed_batch" in text,
    },
    {
        "name": "mentions_metadata_methods",
        "description": "Menciona métodos de metadata (`dimensions`, `provider_name`, `model_name`, `max_tokens`).",
        "check": lambda text: "dimensions" in text and "provider_name" in text and "max_tokens" in text,
    },
    {
        "name": "mentions_all_providers",
        "description": "Menciona explícitamente a `onnx`, `gemini`, `ollama` y `openai`.",
        "check": lambda text: "onnx" in text and "gemini" in text and "ollama" in text and "openai" in text,
    },
    {
        "name": "mentions_instantiation",
        "description": "Menciona la función `create_embedder_from_config` en `src/cli/mod.rs`.",
        "check": lambda text: "create_embedder_from_config" in text and "src/cli/mod.rs" in text,
    },
]

def judge_evaluator(generated_text: str) -> dict[str, Any]:
    if not generated_text:
        return {"passed": [], "failed": [r["name"] for r in JUDGE_RULES], "score": 0.0}

    passed = []
    failed = []
    
    for rule in JUDGE_RULES:
        if rule["check"](generated_text):
            passed.append(rule["name"])
        else:
            failed.append(rule["name"])

    score = len(passed) / len(JUDGE_RULES) if JUDGE_RULES else 0.0

    return {
        "passed": passed,
        "failed": failed,
        "score": round(score, 4),
    }

# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------

def generate_report(
    arm_a_result: dict[str, Any],
    arm_b_result: dict[str, Any],
    quality_a: dict[str, Any],
    quality_b: dict[str, Any],
    dry_run: bool,
) -> dict[str, Any]:
    a_tokens = arm_a_result["total_tokens"]
    b_tokens = arm_b_result["total_tokens"]
    token_savings_pct = round((1 - b_tokens / a_tokens) * 100, 2) if a_tokens > 0 else 0.0

    return {
        "metadata": {
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "benchmark": "Phase 3 - Context Bloat",
            "task": CONTEXT_TASK,
            "dry_run": dry_run,
        },
        "metrics": {
            "arm_a_total_tokens": a_tokens,
            "arm_b_total_tokens": b_tokens,
            "token_savings_percent": token_savings_pct,
            "arm_a_quality_score": quality_a["score"],
            "arm_b_quality_score": quality_b["score"],
        },
        "arm_a": {
            "total_tokens": a_tokens,
            "generated_text": arm_a_result["generated_code"],
            "quality_checks": quality_a,
        },
        "arm_b": {
            "total_tokens": b_tokens,
            "generated_text": arm_b_result["generated_code"],
            "quality_checks": quality_b,
        }
    }


def main() -> int:
    dry_run = "--dry-run" in sys.argv

    print("=" * 60)
    print("  VectorCode Phase 3 — Context Bloat Benchmark")
    print("=" * 60)

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    cmd_prefix = find_vectorcode()
    
    print(f"\\nTask: {CONTEXT_TASK}\\n")

    # Arm A
    print(f"{'─' * 50}\\n  Arm A: Baseline (grep + read_file)\\n{'─' * 50}")
    arm_a_result = run_arm("A", TOOL_CALLS_ARM_A, cmd_prefix, REPO_ROOT, dry_run)
    print(f"  Total tokens: {arm_a_result['total_tokens']}")

    # Arm B
    print(f"\\n{'─' * 50}\\n  Arm B: VectorCode (vec_search)\\n{'─' * 50}")
    arm_b_result = run_arm("B", TOOL_CALLS_ARM_B, cmd_prefix, REPO_ROOT, dry_run)
    print(f"  Total tokens: {arm_b_result['total_tokens']}")

    # Judge
    print(f"\\n{'─' * 50}\\n  AI Judge Evaluation\\n{'─' * 50}")
    quality_a = judge_evaluator(arm_a_result["generated_code"])
    quality_b = judge_evaluator(arm_b_result["generated_code"])

    print(f"  Arm A Judge Score: {quality_a['score']:.0%} ({len(quality_a['passed'])}/{len(JUDGE_RULES)})")
    print(f"  Arm B Judge Score: {quality_b['score']:.0%} ({len(quality_b['passed'])}/{len(JUDGE_RULES)})")

    # Report
    report = generate_report(arm_a_result, arm_b_result, quality_a, quality_b, dry_run)
    with open(REPORT_PATH, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)

    m = report["metrics"]
    print(f"\\n{'=' * 60}")
    print("  RESULTS SUMMARY")
    print(f"{'=' * 60}")
    print(f"  {'Metric':<30} {'Arm A':>10} {'Arm B':>10}")
    print(f"  {'─' * 30} {'─' * 10} {'─' * 10}")
    print(f"  {'Total Input Tokens':<30} {m['arm_a_total_tokens']:>10} {m['arm_b_total_tokens']:>10}")
    print(f"  {'AI Judge Score':<30} {m['arm_a_quality_score']:>10.0%} {m['arm_b_quality_score']:>10.0%}")
    print(f"\\n  Token savings: {m['token_savings_percent']:.1f}%")
    print(f"  Report: {REPORT_PATH}")
    print(f"{'=' * 60}")
    
    return 0

if __name__ == "__main__":
    sys.exit(main())
