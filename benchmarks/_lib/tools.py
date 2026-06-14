"""Tool functions and tool definitions for benchmark agent runs."""
from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path
from typing import Callable

from .adapters import ToolDef

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
REPO_ROOT: Path = Path(__file__).resolve().parent.parent.parent

# ---------------------------------------------------------------------------
# VectorCode binary resolution
# ---------------------------------------------------------------------------


def find_vectorcode() -> list[str]:
    """Resolve the vectorcode binary for subprocess calls.

    Priority: VECTORCODE_BIN env var > PATH lookup > cargo run fallback.
    """
    env_bin = os.environ.get("VECTORCODE_BIN")
    if env_bin:
        return [env_bin]
    if shutil.which("vectorcode"):
        return ["vectorcode"]
    return ["cargo", "run", "--quiet", "--"]


# ---------------------------------------------------------------------------
# Tool functions
# ---------------------------------------------------------------------------


def tool_execute_bash(command: str) -> str:
    """Execute a bash command in the repository root."""
    print(f"    [Tool] bash: {command}")
    try:
        res = subprocess.run(
            command,
            cwd=REPO_ROOT,
            shell=True,
            capture_output=True,
            text=True,
            timeout=15.0,
        )
        out = res.stdout
        if res.stderr:
            out += f"\nSTDERR:\n{res.stderr}"
        if not out:
            out = "(Command executed successfully with no output)"
        return out[:8000]
    except Exception as e:
        return f"Error executing bash: {e}"


def tool_vec_search(query: str) -> str:
    """Semantic search over the codebase via vectorcode CLI."""
    print(f"    [Tool] vec_search: {query}")
    cmd = [*find_vectorcode(), "search", query, "--json", "--limit", "4"]
    try:
        res = subprocess.run(
            cmd, cwd=REPO_ROOT, capture_output=True, text=True, timeout=30.0
        )
        return res.stdout[:15000]
    except Exception as e:
        return f"Error searching: {e}"


def tool_read_file(path: str) -> str:
    """Read full contents of a file relative to REPO_ROOT."""
    print(f"    [Tool] read_file: {path}")
    full_path = REPO_ROOT / path
    try:
        return full_path.read_text(encoding="utf-8")[:8000]
    except Exception as e:
        return f"Error reading file: {e}"


def tool_vec_read_lines(path: str, start_line: int, end_line: int) -> str:
    """Read a specific line range (1-indexed) from a file relative to REPO_ROOT."""
    print(f"    [Tool] vec_read_lines: {path} ({start_line}-{end_line})")
    full_path = REPO_ROOT / path
    try:
        lines = full_path.read_text(encoding="utf-8").splitlines()
        s = max(0, start_line - 1)
        e = min(len(lines), end_line)
        if s >= e:
            return "Error: invalid range"
        extracted = "\n".join(lines[s:e])
        return f"Lines {start_line}-{end_line} of {path}:\n{extracted}"
    except Exception as e:
        return f"Error reading lines: {e}"


def tool_vec_outline(path: str) -> str:
    """Get a structural outline of a source file via vectorcode CLI."""
    print(f"    [Tool] vec_outline: {path}")
    cmd = [*find_vectorcode(), "outline", path]
    try:
        res = subprocess.run(
            cmd, cwd=REPO_ROOT, capture_output=True, text=True, timeout=15.0
        )
        return res.stdout[:8000]
    except Exception as e:
        return f"Error outlining: {e}"


# ---------------------------------------------------------------------------
# Tool registry — maps tool name to callable
# ---------------------------------------------------------------------------

TOOL_REGISTRY: dict[str, Callable] = {
    "execute_bash": tool_execute_bash,
    "vec_search": tool_vec_search,
    "read_file": tool_read_file,
    "vec_read_lines": tool_vec_read_lines,
    "vec_outline": tool_vec_outline,
}

# ---------------------------------------------------------------------------
# Canonical tool definitions for SDK adapters
# ---------------------------------------------------------------------------


def build_arm_a_tools(use_read_file: bool) -> list[dict]:
    """Build canonical tool definitions for Arm A (bash-based discovery).

    Returns OpenAI-shaped tool defs. Adapters transform as needed.
    """
    tools: list[dict] = [
        {
            "type": "function",
            "function": {
                "name": "execute_bash",
                "description": "Execute a bash command (e.g. grep, find)",
                "parameters": {
                    "type": "object",
                    "properties": {"command": {"type": "string"}},
                    "required": ["command"],
                },
            },
        }
    ]
    if use_read_file:
        tools.append(
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read full contents of a file",
                    "parameters": {
                        "type": "object",
                        "properties": {"path": {"type": "string"}},
                        "required": ["path"],
                    },
                },
            }
        )
    return tools


def build_arm_b_tools(use_read_file: bool) -> list[dict]:
    """Build canonical tool definitions for Arm B (vectorcode-based discovery).

    Returns OpenAI-shaped tool defs. Adapters transform as needed.
    """
    tools: list[dict] = [
        {
            "type": "function",
            "function": {
                "name": "vec_search",
                "description": "Semantic search over the codebase. Returns code snippets.",
                "parameters": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"],
                },
            },
        }
    ]
    if use_read_file:
        tools.append(
            {
                "type": "function",
                "function": {
                    "name": "vec_read_lines",
                    "description": "Read specific lines of a file",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "start_line": {"type": "integer"},
                            "end_line": {"type": "integer"},
                        },
                        "required": ["path", "start_line", "end_line"],
                    },
                },
            }
        )
        tools.append(
            {
                "type": "function",
                "function": {
                    "name": "vec_outline",
                    "description": (
                        "Get a structural outline of a source file \u2014 top-level functions, "
                        "classes, structs, interfaces, and traits with their signatures. "
                        "Useful for understanding file structure without reading the entire file."
                    ),
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "file_path": {
                                "type": "string",
                                "description": "The file path to outline (relative to project root)",
                            }
                        },
                        "required": ["file_path"],
                    },
                },
            }
        )
    return tools
