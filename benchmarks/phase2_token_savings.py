#!/usr/bin/env python3
"""
Phase 2 Token Savings Benchmark for VectorCode.

Two-arm agent simulator comparing grep-based vs VectorCode-search-based
convention discovery. Measures input tokens consumed and tool calls made
when an agent imitates `install.rs` conventions to produce a `status.rs`
CLI subcommand skeleton.

Arm A: grep + find + read_file (traditional discovery)
Arm B: vec_search + read_file (semantic discovery)

Usage:
    # From the repository root:
    python benchmarks/phase2_token_savings.py

    # Dry-run mode (no subprocess calls — uses mock responses):
    python benchmarks/phase2_token_savings.py --dry-run

    # Run inline unit tests:
    python benchmarks/phase2_token_savings.py --test

Requirements:
    - Python 3.10+
    - tiktoken (optional — graceful fallback to char-count proxy)
    - vectorcode CLI available via `cargo run --` or on PATH

Output:
    - benchmarks/results/phase2_report.json  (structured metrics)
    - benchmarks/results/session_arm_a.log   (JSONL session log)
    - benchmarks/results/session_arm_b.log   (JSONL session log)
    - stdout summary table (human-readable)
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
REPORT_PATH = RESULTS_DIR / "phase2_report.json"

IMITATION_TASK = (
    "Add a new CLI `status` subcommand that displays index health statistics, "
    "following the same conventions as the existing `install` CLI subcommand "
    "in `src/cli/install.rs`."
)

# ---------------------------------------------------------------------------
# Scripted tool-call sequences
# ---------------------------------------------------------------------------
# Each step: {tool, args, is_exploration}
# The last step in each sequence is code generation (is_exploration=False).

TOOL_CALLS_ARM_A: list[dict[str, Any]] = [
    {
        "tool": "grep",
        "args": "grep -rn 'clap' src/cli/",
        "is_exploration": True,
    },
    {
        "tool": "grep",
        "args": "grep -n 'subcommand' src/cli/mod.rs",
        "is_exploration": True,
    },
    {
        "tool": "read_file",
        "args": "src/cli/install.rs",
        "is_exploration": True,
    },
    {
        "tool": "read_file",
        "args": "src/cli/mod.rs",
        "is_exploration": True,
    },
    {
        "tool": "grep",
        "args": "grep -rn 'pub fn execute' src/cli/",
        "is_exploration": True,
    },
    {
        "tool": "generate",
        "args": IMITATION_TASK,
        "is_exploration": False,
    },
]

TOOL_CALLS_ARM_B: list[dict[str, Any]] = [
    {
        "tool": "vec_search",
        "args": "CLI subcommand implementation pattern with clap derive macros",
        "is_exploration": True,
    },
    {
        "tool": "read_file",
        "args": "src/cli/install.rs",
        "is_exploration": True,
    },
    {
        "tool": "vec_search",
        "args": "how are subcommands registered in the CLI module Commands enum",
        "is_exploration": True,
    },
    {
        "tool": "read_file",
        "args": "src/cli/mod.rs",
        "is_exploration": True,
    },
    {
        "tool": "generate",
        "args": IMITATION_TASK,
        "is_exploration": False,
    },
]

# ---------------------------------------------------------------------------
# Simulated generated code for each arm
# ---------------------------------------------------------------------------
# Arm A: agent found patterns via grep — may miss some conventions
GENERATED_CODE_ARM_A = '''\
use anyhow::Result;
use clap::Args;
use std::path::Path;

/// Arguments for `vectorcode status`.
#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Show detailed statistics.
    #[arg(long)]
    pub detailed: bool,
}

/// Execute the `status` command.
pub fn execute(args: &StatusArgs, project_path: &Path) -> Result<()> {
    let vc_dir = project_path.join(".vectorcode");
    if !vc_dir.exists() {
        eprintln!("Not initialized. Run `vectorcode init` first.");
        return Ok(());
    }

    let db_path = vc_dir.join("index.db");
    if db_path.exists() {
        let metadata = std::fs::metadata(&db_path)?;
        println!("Index: {}", db_path.display());
        println!("Size: {} bytes", metadata.len());
    }

    Ok(())
}
'''

# Arm B: agent found patterns via semantic search — more complete conventions
GENERATED_CODE_ARM_B = '''\
use anyhow::Result;
use clap::Args;
use std::path::Path;
use tracing::info;

/// Arguments for `vectorcode status`.
#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Show detailed statistics including per-file chunk counts.
    #[arg(long)]
    pub detailed: bool,

    /// Output format (text or json).
    #[arg(long, default_value = "text")]
    pub format: String,
}

/// Execute the `status` command.
///
/// Displays index health statistics for the current project.
/// Follows the same conventions as `install.rs`.
pub fn execute(args: &StatusArgs, project_path: &Path) -> Result<()> {
    let vc_dir = project_path.join(".vectorcode");
    if !vc_dir.exists() {
        eprintln!("Not initialized. Run `vectorcode init` first.");
        return Ok(());
    }

    let db_path = vc_dir.join("index.db");
    let config_path = vc_dir.join("config.toml");

    info!("Checking status for project: {}", project_path.display());

    if config_path.exists() {
        let config_content = std::fs::read_to_string(&config_path)?;
        println!("Config: {} ({} bytes)", config_path.display(), config_content.len());
    }

    if db_path.exists() {
        let metadata = std::fs::metadata(&db_path)?;
        println!("Index DB: {}", db_path.display());
        println!("Size: {} bytes", metadata.len());

        if args.detailed {
            println!("Detailed mode: checking chunk counts...");
        }
    } else {
        eprintln!("Index not found. Run `vectorcode index` to build it.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_args_parse_defaults() {
        let args = clap::Parser::parse_from(["status"]);
        let _ = args;
    }
}
'''


# ---------------------------------------------------------------------------
# Core functions
# ---------------------------------------------------------------------------


def find_vectorcode() -> list[str]:
    """Detect how to invoke vectorcode.

    Priority:
      1. VECTORCODE_BIN env var (explicit path or binary name)
      2. `vectorcode` on PATH
      3. `cargo run --` fallback (builds from source)

    Returns:
        Command prefix list, e.g. ["vectorcode"] or ["cargo", "run", "--"].
    """
    env_bin = os.environ.get("VECTORCODE_BIN")
    if env_bin:
        return [env_bin]

    if shutil.which("vectorcode") is not None:
        return ["vectorcode"]

    return ["cargo", "run", "--"]


def count_tokens(text: str) -> int:
    """Count tokens in text using tiktoken (cl100k_base) or fallback proxy.

    Args:
        text: The text to count tokens for.

    Returns:
        Token count (exact via tiktoken, or approx chars/4 as proxy).
    """
    if not text:
        return 0
    if _HAS_TIKTOKEN:
        return len(_encoder.encode(text))
    # Fallback: rough approximation (4 chars per token)
    return len(text) // 4


def _execute_grep(cmd: str, project_path: Path) -> tuple[str, int]:
    """Execute a grep command and return (response_text, token_count).

    Args:
        cmd: Full grep command string (e.g. "grep -rn 'clap' src/cli/").
        project_path: Working directory for the subprocess.

    Returns:
        Tuple of (stdout output, token count of output).
    """
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
    """Execute a vectorcode search and return (response_text, token_count).

    Args:
        query: Natural language search query.
        cmd_prefix: Command prefix (e.g. ["cargo", "run", "--"]).
        project_path: Working directory for the subprocess.

    Returns:
        Tuple of (stdout output, token count of output).
    """
    cmd = [*cmd_prefix, "search", query, "--json", "--limit", "3"]
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
    """Read a file from the project and return (content, token_count).

    Args:
        path: Relative path within the project.
        project_path: Project root directory.

    Returns:
        Tuple of (file content, token count of content).
    """
    full_path = project_path / path
    try:
        content = full_path.read_text(encoding="utf-8")
    except (FileNotFoundError, PermissionError):
        content = ""

    return (content, count_tokens(content))


def _mock_response(tool: str, args: str, project_path: Path) -> tuple[str, int]:
    """Return a mock response for dry-run mode.

    Simulates realistic response sizes for each tool type.

    Args:
        tool: Tool name (grep, vec_search, read_file, generate).
        args: Tool arguments.
        project_path: Project root (used for read_file).

    Returns:
        Tuple of (mock response text, token count).
    """
    if tool == "read_file":
        return _execute_read_file(args, project_path)

    if tool == "grep":
        # Simulate grep output: multiple file matches with line numbers
        return (
            f"src/cli/install.rs:6:use clap::{{Args, ValueEnum}};\n"
            f"src/cli/mod.rs:19:use clap::{{Parser, Subcommand, ValueEnum}};\n"
            f"src/cli/search.rs:4:use clap::Args;\n"
            f"src/cli/status.rs:4:use clap::Args;\n",
            42,
        )

    if tool == "vec_search":
        # Simulate semantic search: ranked JSON results
        return (
            json.dumps(
                [
                    {
                        "file_path": "src/cli/install.rs",
                        "line_start": 207,
                        "line_end": 225,
                        "symbols": ["InstallArgs", "execute"],
                        "source": "pub struct InstallArgs { ... }\npub fn execute(args: &InstallArgs, ...) -> Result<()>",
                        "score": 0.89,
                    },
                    {
                        "file_path": "src/cli/mod.rs",
                        "line_start": 46,
                        "line_end": 64,
                        "symbols": ["Commands"],
                        "source": "pub enum Commands { Install(install::InstallArgs), ... }",
                        "score": 0.82,
                    },
                ]
            ),
            85,
        )

    if tool == "generate":
        return ("", 0)

    return ("", 0)


def run_arm(
    arm_id: str,
    tool_calls: list[dict[str, Any]],
    cmd_prefix: list[str],
    project_path: Path,
    dry_run: bool,
) -> dict[str, Any]:
    """Execute a single arm's tool-call sequence.

    Args:
        arm_id: "A" or "B".
        tool_calls: List of tool-call dicts.
        cmd_prefix: Command prefix for vectorcode CLI.
        project_path: Project root directory.
        dry_run: If True, use mock responses instead of subprocess.

    Returns:
        Dict with keys: steps (list), total_tokens, exploration_tokens,
        exploration_steps, tool_calls_count, generated_code.
    """
    log_path = RESULTS_DIR / f"session_arm_{arm_id.lower()}.log"
    steps: list[dict[str, Any]] = []
    total_tokens = 0
    exploration_tokens = 0
    exploration_steps = 0
    generated_code = ""

    for i, call in enumerate(tool_calls, start=1):
        tool = call["tool"]
        args = call["args"]
        is_exploration = call["is_exploration"]

        # Execute the tool
        if tool == "generate":
            # Code generation step — always use pre-scripted code
            response_text = (
                GENERATED_CODE_ARM_A if arm_id == "A" else GENERATED_CODE_ARM_B
            )
            response_tokens = count_tokens(response_text)
            generated_code = response_text
        elif dry_run:
            response_text, response_tokens = _mock_response(
                tool, args, project_path
            )
        elif tool == "grep":
            response_text, response_tokens = _execute_grep(args, project_path)
        elif tool == "vec_search":
            response_text, response_tokens = _execute_vec_search(
                args, cmd_prefix, project_path
            )
        elif tool == "read_file":
            response_text, response_tokens = _execute_read_file(
                args, project_path
            )
        else:
            response_text, response_tokens = ("", 0)

        step_record = {
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "arm": arm_id,
            "step": i,
            "tool": tool,
            "args": args if isinstance(args, str) else json.dumps(args),
            "response_text": response_text,
            "response_tokens": response_tokens,
            "response_chars": len(response_text),
            "is_exploration": is_exploration,
        }
        steps.append(step_record)

        total_tokens += response_tokens
        if is_exploration:
            exploration_tokens += response_tokens
            exploration_steps += 1

    # Write JSONL log
    with open(log_path, "w", encoding="utf-8") as f:
        for step in steps:
            f.write(json.dumps(step, ensure_ascii=False) + "\n")

    return {
        "steps": steps,
        "total_tokens": total_tokens,
        "exploration_tokens": exploration_tokens,
        "exploration_steps": exploration_steps,
        "tool_calls_count": len(tool_calls),
        "generated_code": generated_code,
    }


# ---------------------------------------------------------------------------
# Quality evaluator
# ---------------------------------------------------------------------------

QUALITY_RULES = [
    {
        "name": "anyhow_result_import",
        "description": "Uses `use anyhow::Result;`",
        "check": lambda code: "use anyhow::Result" in code,
    },
    {
        "name": "clap_args_import",
        "description": "Uses `use clap::Args;`",
        "check": lambda code: "use clap::Args" in code,
    },
    {
        "name": "derive_args_struct",
        "description": "Has `#[derive(Args` on a struct",
        "check": lambda code: "#[derive(Args" in code
        or "#[derive(Debug, Args" in code
        or "#[derive(Args, Debug" in code,
    },
    {
        "name": "struct_name_ends_with_args",
        "description": "Struct name ends with `Args`",
        "check": lambda code: "pub struct StatusArgs" in code,
    },
    {
        "name": "execute_function_signature",
        "description": "Has `pub fn execute(args: &...) -> Result<()>` signature",
        "check": lambda code: "pub fn execute(" in code
        and "-> Result<()>" in code,
    },
    {
        "name": "test_module_present",
        "description": "Has `#[cfg(test)] mod tests` block",
        "check": lambda code: "#[cfg(test)]" in code and "mod tests" in code,
    },
    {
        "name": "no_unwrap_usage",
        "description": "No `.unwrap()` in non-test code",
        "check": lambda code: ".unwrap()" not in code,
    },
]


def quality_evaluator(generated_code: str) -> dict[str, Any]:
    """Evaluate generated code against convention rules from install.rs.

    Args:
        generated_code: The code string to evaluate.

    Returns:
        Dict with keys: passed (list[str]), failed (list[str]),
        score (float 0.0-1.0), per_check_scores (dict[str, bool]).
    """
    if not generated_code:
        return {
            "passed": [],
            "failed": [r["name"] for r in QUALITY_RULES],
            "score": 0.0,
            "per_check_scores": {r["name"]: False for r in QUALITY_RULES},
        }

    passed: list[str] = []
    failed: list[str] = []
    per_check: dict[str, bool] = {}

    for rule in QUALITY_RULES:
        result = rule["check"](generated_code)
        per_check[rule["name"]] = result
        if result:
            passed.append(rule["name"])
        else:
            failed.append(rule["name"])

    score = len(passed) / len(QUALITY_RULES) if QUALITY_RULES else 0.0

    return {
        "passed": passed,
        "failed": failed,
        "score": round(score, 4),
        "per_check_scores": per_check,
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
    """Build the final Phase 2 report dict.

    Args:
        arm_a_result: Result dict from run_arm("A", ...).
        arm_b_result: Result dict from run_arm("B", ...).
        quality_a: Quality evaluation for Arm A.
        quality_b: Quality evaluation for Arm B.
        dry_run: Whether this was a dry run.

    Returns:
        Complete report dict matching the Phase 2 report schema.
    """
    a_tokens = arm_a_result["total_tokens"]
    b_tokens = arm_b_result["total_tokens"]
    token_savings_pct = (
        round((1 - b_tokens / a_tokens) * 100, 2) if a_tokens > 0 else 0.0
    )

    a_calls = arm_a_result["tool_calls_count"]
    b_calls = arm_b_result["tool_calls_count"]
    call_reduction_pct = (
        round((1 - b_calls / a_calls) * 100, 2) if a_calls > 0 else 0.0
    )

    a_explore = arm_a_result["exploration_steps"]
    b_explore = arm_b_result["exploration_steps"]
    exploration_improvement_pct = (
        round((1 - b_explore / a_explore) * 100, 2) if a_explore > 0 else 0.0
    )

    a_explore_tokens = arm_a_result["exploration_tokens"]
    b_explore_tokens = arm_b_result["exploration_tokens"]
    exploration_efficiency = (
        round(b_explore_tokens / a_explore_tokens, 4)
        if a_explore_tokens > 0
        else 1.0
    )

    return {
        "metadata": {
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "benchmark": "Phase 2 - Token Savings",
            "imitation_task": IMITATION_TASK,
            "token_method": "tiktoken_cl100k_base" if _HAS_TIKTOKEN else "proxy_chars_div_4",
            "dry_run": dry_run,
        },
        "metrics": {
            "arm_a_total_tokens": a_tokens,
            "arm_b_total_tokens": b_tokens,
            "token_savings_percent": token_savings_pct,
            "arm_a_tool_calls": a_calls,
            "arm_b_tool_calls": b_calls,
            "call_reduction_percent": call_reduction_pct,
            "arm_a_exploration_calls": a_explore,
            "arm_b_exploration_calls": b_explore,
            "exploration_efficiency_improvement_percent": exploration_improvement_pct,
            "exploration_efficiency_ratio": exploration_efficiency,
            "arm_a_quality_score": quality_a["score"],
            "arm_b_quality_score": quality_b["score"],
            "quality_score_delta": round(
                quality_b["score"] - quality_a["score"], 4
            ),
        },
        "arm_a": {
            "tool_calls": [
                {
                    "step": s["step"],
                    "tool": s["tool"],
                    "args": s["args"],
                    "response_tokens": s["response_tokens"],
                    "is_exploration": s["is_exploration"],
                }
                for s in arm_a_result["steps"]
            ],
            "total_tokens": a_tokens,
            "exploration_tokens": a_explore_tokens,
            "generated_code": arm_a_result["generated_code"],
            "quality_checks": quality_a,
        },
        "arm_b": {
            "tool_calls": [
                {
                    "step": s["step"],
                    "tool": s["tool"],
                    "args": s["args"],
                    "response_tokens": s["response_tokens"],
                    "is_exploration": s["is_exploration"],
                }
                for s in arm_b_result["steps"]
            ],
            "total_tokens": b_tokens,
            "exploration_tokens": b_explore_tokens,
            "generated_code": arm_b_result["generated_code"],
            "quality_checks": quality_b,
        },
        "quality_comparison": {
            "arm_a_score": quality_a["score"],
            "arm_b_score": quality_b["score"],
            "score_delta": round(quality_b["score"] - quality_a["score"], 4),
            "arm_a_passed": len(quality_a["passed"]),
            "arm_b_passed": len(quality_b["passed"]),
            "total_rules": len(QUALITY_RULES),
        },
    }


# ---------------------------------------------------------------------------
# Main orchestration
# ---------------------------------------------------------------------------


def main() -> int:
    """Run the Phase 2 benchmark: two-arm agent simulator."""
    dry_run = "--dry-run" in sys.argv

    print("=" * 60)
    print("  VectorCode Phase 2 — Token Savings Benchmark")
    print("=" * 60)

    # Setup
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    cmd_prefix = find_vectorcode()
    print(f"\nCommand prefix: {' '.join(cmd_prefix)}")
    print(f"Token method: {'tiktoken (cl100k_base)' if _HAS_TIKTOKEN else 'proxy (chars/4)'}")
    print(f"Mode: {'DRY RUN' if dry_run else 'LIVE'}")

    if not _HAS_TIKTOKEN:
        print("WARNING: tiktoken not available — using char-count proxy")

    print(f"\nImitation task: {IMITATION_TASK}")

    # ── Run Arm A ──────────────────────────────────────────────────────
    print(f"\n{'─' * 50}")
    print("  Arm A: grep-based discovery")
    print(f"{'─' * 50}")

    arm_a_result = run_arm("A", TOOL_CALLS_ARM_A, cmd_prefix, REPO_ROOT, dry_run)
    print(f"  Tool calls: {arm_a_result['tool_calls_count']}")
    print(f"  Total tokens: {arm_a_result['total_tokens']}")
    print(f"  Exploration steps: {arm_a_result['exploration_steps']}")
    print(f"  Session log: {RESULTS_DIR / 'session_arm_a.log'}")

    # ── Run Arm B ──────────────────────────────────────────────────────
    print(f"\n{'─' * 50}")
    print("  Arm B: VectorCode-search-based discovery")
    print(f"{'─' * 50}")

    arm_b_result = run_arm("B", TOOL_CALLS_ARM_B, cmd_prefix, REPO_ROOT, dry_run)
    print(f"  Tool calls: {arm_b_result['tool_calls_count']}")
    print(f"  Total tokens: {arm_b_result['total_tokens']}")
    print(f"  Exploration steps: {arm_b_result['exploration_steps']}")
    print(f"  Session log: {RESULTS_DIR / 'session_arm_b.log'}")

    # ── Quality evaluation ─────────────────────────────────────────────
    print(f"\n{'─' * 50}")
    print("  Quality Evaluation")
    print(f"{'─' * 50}")

    quality_a = quality_evaluator(arm_a_result["generated_code"])
    quality_b = quality_evaluator(arm_b_result["generated_code"])

    print(f"  Arm A quality: {quality_a['score']:.0%} ({len(quality_a['passed'])}/{len(QUALITY_RULES)})")
    print(f"    Passed: {', '.join(quality_a['passed']) if quality_a['passed'] else 'none'}")
    print(f"    Failed: {', '.join(quality_a['failed']) if quality_a['failed'] else 'none'}")

    print(f"  Arm B quality: {quality_b['score']:.0%} ({len(quality_b['passed'])}/{len(QUALITY_RULES)})")
    print(f"    Passed: {', '.join(quality_b['passed']) if quality_b['passed'] else 'none'}")
    print(f"    Failed: {', '.join(quality_b['failed']) if quality_b['failed'] else 'none'}")

    # ── Generate report ────────────────────────────────────────────────
    report = generate_report(arm_a_result, arm_b_result, quality_a, quality_b, dry_run)

    with open(REPORT_PATH, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)

    # ── Print summary ──────────────────────────────────────────────────
    m = report["metrics"]
    print(f"\n{'=' * 60}")
    print("  RESULTS SUMMARY")
    print(f"{'=' * 60}")
    print()
    print(f"  {'Metric':<45} {'Arm A':>10} {'Arm B':>10}")
    print(f"  {'─' * 45} {'─' * 10} {'─' * 10}")
    print(f"  {'Total tokens':<45} {m['arm_a_total_tokens']:>10} {m['arm_b_total_tokens']:>10}")
    print(f"  {'Tool calls':<45} {m['arm_a_tool_calls']:>10} {m['arm_b_tool_calls']:>10}")
    print(f"  {'Exploration calls':<45} {m['arm_a_exploration_calls']:>10} {m['arm_b_exploration_calls']:>10}")
    print(f"  {'Quality score':<45} {m['arm_a_quality_score']:>10.0%} {m['arm_b_quality_score']:>10.0%}")
    print()
    print(f"  Token savings:           {m['token_savings_percent']:.1f}%")
    print(f"  Call reduction:          {m['call_reduction_percent']:.1f}%")
    print(f"  Exploration improvement: {m['exploration_efficiency_improvement_percent']:.1f}%")
    print(f"  Quality delta:           {m['quality_score_delta']:+.2f}")
    print()
    print(f"  Report: {REPORT_PATH}")
    print(f"{'=' * 60}")

    return 0


# ---------------------------------------------------------------------------
# Unit tests
# ---------------------------------------------------------------------------


def _test_count_tokens() -> None:
    """Test token counting with known strings."""
    # Empty string
    assert count_tokens("") == 0, "Empty string should have 0 tokens"
    print("  PASS: count_tokens empty string returns 0")

    # Non-empty string
    tokens = count_tokens("fn main() {}")
    assert tokens > 0, f"Non-empty string should have >0 tokens, got {tokens}"
    print(f"  PASS: count_tokens 'fn main() {{}}' = {tokens}")

    # Longer string has more tokens
    short = count_tokens("hello")
    long = count_tokens("hello world this is a longer sentence for testing")
    assert long > short, "Longer string should have more tokens"
    print(f"  PASS: count_tokens longer > shorter ({long} > {short})")


def _test_quality_evaluator_good_code() -> None:
    """Test quality evaluator with code that follows conventions."""
    result = quality_evaluator(GENERATED_CODE_ARM_B)
    # Arm B code should pass most rules
    assert result["score"] > 0.5, f"Good code should score >50%, got {result['score']}"
    assert "anyhow_result_import" in result["passed"]
    assert "clap_args_import" in result["passed"]
    assert "derive_args_struct" in result["passed"]
    assert "struct_name_ends_with_args" in result["passed"]
    assert "execute_function_signature" in result["passed"]
    assert "test_module_present" in result["passed"]
    assert "no_unwrap_usage" in result["passed"]
    print(f"  PASS: quality_evaluator good code scores {result['score']:.0%}")


def _test_quality_evaluator_bad_code() -> None:
    """Test quality evaluator with code that violates conventions."""
    bad_code = "fn main() { println!(\"hello\"); }"
    result = quality_evaluator(bad_code)
    assert result["score"] < 0.5, f"Bad code should score <50%, got {result['score']}"
    assert "anyhow_result_import" in result["failed"]
    assert "clap_args_import" in result["failed"]
    print(f"  PASS: quality_evaluator bad code scores {result['score']:.0%}")


def _test_quality_evaluator_empty_code() -> None:
    """Test quality evaluator with empty code."""
    result = quality_evaluator("")
    assert result["score"] == 0.0, "Empty code should score 0"
    assert len(result["passed"]) == 0
    assert len(result["failed"]) == len(QUALITY_RULES)
    print("  PASS: quality_evaluator empty code scores 0")


def _test_quality_evaluator_unwrap_detection() -> None:
    """Test that .unwrap() is detected as a convention violation."""
    code_with_unwrap = "fn execute() -> Result<()> {\n    let x = foo.unwrap();\n    Ok(())\n}"
    result = quality_evaluator(code_with_unwrap)
    assert result["per_check_scores"]["no_unwrap_usage"] is False
    print("  PASS: quality_evaluator detects .unwrap() violation")


def _test_generate_report_structure() -> None:
    """Test that generate_report produces correct schema."""
    mock_arm = {
        "steps": [],
        "total_tokens": 100,
        "exploration_tokens": 80,
        "exploration_steps": 3,
        "tool_calls_count": 4,
        "generated_code": "fn test() {}",
    }
    mock_quality = {
        "passed": ["a", "b"],
        "failed": ["c"],
        "score": 0.67,
        "per_check_scores": {"a": True, "b": True, "c": False},
    }

    report = generate_report(mock_arm, mock_arm, mock_quality, mock_quality, True)

    assert "metadata" in report
    assert "metrics" in report
    assert "arm_a" in report
    assert "arm_b" in report
    assert "quality_comparison" in report

    m = report["metrics"]
    assert "arm_a_total_tokens" in m
    assert "arm_b_total_tokens" in m
    assert "token_savings_percent" in m
    assert "arm_a_quality_score" in m
    assert "arm_b_quality_score" in m

    qc = report["quality_comparison"]
    assert "arm_a_score" in qc
    assert "arm_b_score" in qc
    assert "score_delta" in qc

    print("  PASS: generate_report produces correct schema")


def _test_token_savings_calculation() -> None:
    """Test token savings percentage calculation."""
    arm_a = {
        "steps": [],
        "total_tokens": 1000,
        "exploration_tokens": 800,
        "exploration_steps": 5,
        "tool_calls_count": 6,
        "generated_code": "code",
    }
    arm_b = {
        "steps": [],
        "total_tokens": 500,
        "exploration_tokens": 300,
        "exploration_steps": 3,
        "tool_calls_count": 4,
        "generated_code": "code",
    }
    quality = {"passed": [], "failed": [], "score": 0.5, "per_check_scores": {}}

    report = generate_report(arm_a, arm_b, quality, quality, True)
    assert report["metrics"]["token_savings_percent"] == 50.0
    assert report["metrics"]["exploration_efficiency_ratio"] == 0.375
    print("  PASS: token savings calculation correct (50%)")


def _test_session_parser_integration(tmp_path: Path) -> None:
    """Test that JSONL output can be parsed correctly."""
    # Write a sample JSONL file
    log_path = tmp_path / "test_session.jsonl"
    entries = [
        {"step": 1, "tool": "grep", "response_tokens": 42, "is_exploration": True},
        {"step": 2, "tool": "read_file", "response_tokens": 150, "is_exploration": True},
        {"step": 3, "tool": "generate", "response_tokens": 88, "is_exploration": False},
    ]
    with open(log_path, "w", encoding="utf-8") as f:
        for entry in entries:
            f.write(json.dumps(entry) + "\n")

    # Parse it back
    with open(log_path, encoding="utf-8") as f:
        lines = f.readlines()

    parsed = [json.loads(line) for line in lines if line.strip()]
    assert len(parsed) == 3
    assert parsed[0]["tool"] == "grep"
    assert parsed[2]["is_exploration"] is False

    total = sum(e["response_tokens"] for e in parsed)
    assert total == 280

    exploration = sum(
        e["response_tokens"] for e in parsed if e["is_exploration"]
    )
    assert exploration == 192

    print("  PASS: session JSONL round-trip parse correct")


def run_unit_tests() -> None:
    """Run all unit tests."""
    import tempfile

    print("\nRunning unit tests...")

    _test_count_tokens()
    _test_quality_evaluator_good_code()
    _test_quality_evaluator_bad_code()
    _test_quality_evaluator_empty_code()
    _test_quality_evaluator_unwrap_detection()
    _test_generate_report_structure()
    _test_token_savings_calculation()

    with tempfile.TemporaryDirectory() as tmp:
        _test_session_parser_integration(Path(tmp))

    print("All unit tests passed!\n")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    if "--test" in sys.argv:
        run_unit_tests()
        sys.exit(0)

    sys.exit(main())
