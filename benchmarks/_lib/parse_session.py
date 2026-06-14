"""Parse JSONL session logs and compute exploration metrics.

Python rewrite of agent-eval/parse-session.mjs.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any


def parse_jsonl(file_path: str) -> tuple[list[dict], int]:
    """Parse a JSONL file into a list of step dicts.

    Skips malformed lines with a warning to stderr.

    Args:
        file_path: Path to the JSONL file.

    Returns:
        Tuple of (entries, warnings_count).
    """
    content = Path(file_path).read_text(encoding="utf-8")
    lines = content.split("\n")
    entries: list[dict] = []
    warnings = 0

    for i, line in enumerate(lines):
        line = line.strip()
        if not line:
            continue
        try:
            entries.append(json.loads(line))
        except json.JSONDecodeError:
            warnings += 1
            print(f"WARNING: malformed JSON at line {i + 1}, skipping", file=sys.stderr)

    return entries, warnings


def compute_metrics(entries: list[dict]) -> dict[str, Any]:
    """Compute metrics from parsed session entries.

    Metrics:
    - total_input_tokens: sum of response_tokens / token_count across all entries
    - exploration_tokens: tokens from entries marked is_exploration=true
    - exploration_steps_before_generation: consecutive exploration steps before
      the first non-exploration entry
    - step_count: total number of entries
    - tools_used: sorted list of unique tool names
    """
    total_input_tokens = 0
    exploration_tokens = 0
    exploration_steps_before_generation = 0
    tools_used: set[str] = set()
    found_first_non_exploration = False

    for entry in entries:
        tokens = entry.get("response_tokens") or entry.get("token_count") or 0
        total_input_tokens += tokens

        if "tool" in entry:
            tools_used.add(entry["tool"])

        if entry.get("is_exploration") is True and not found_first_non_exploration:
            exploration_steps_before_generation += 1
            exploration_tokens += tokens
        elif entry.get("is_exploration") is True and found_first_non_exploration:
            exploration_tokens += tokens
        else:
            found_first_non_exploration = True

    return {
        "total_input_tokens": total_input_tokens,
        "exploration_tokens": exploration_tokens,
        "exploration_steps_before_generation": exploration_steps_before_generation,
        "step_count": len(entries),
        "tools_used": sorted(tools_used),
    }


# ---------------------------------------------------------------------------
# Self-tests (run with --test flag)
# ---------------------------------------------------------------------------


def _run_tests() -> None:
    """Run inline tests matching the .mjs version."""
    print("Running parse_session.py tests...\n")
    passed = 0

    # Test 1: computeMetrics with known entries
    entries = [
        {"step": 1, "tool": "grep", "response_tokens": 42, "is_exploration": True},
        {"step": 2, "tool": "read_file", "response_tokens": 150, "is_exploration": True},
        {"step": 3, "tool": "generate", "response_tokens": 88, "is_exploration": False},
    ]
    m = compute_metrics(entries)
    assert m["total_input_tokens"] == 280, f"total tokens should be 280, got {m['total_input_tokens']}"
    assert m["exploration_tokens"] == 192, f"exploration tokens should be 192, got {m['exploration_tokens']}"
    assert m["exploration_steps_before_generation"] == 2
    assert m["step_count"] == 3
    assert m["tools_used"] == sorted(["grep", "read_file", "generate"])
    passed += 1
    print("  PASS: compute_metrics sums tokens correctly")

    # Test 2: empty entries
    m = compute_metrics([])
    assert m["total_input_tokens"] == 0
    assert m["exploration_tokens"] == 0
    assert m["exploration_steps_before_generation"] == 0
    assert m["step_count"] == 0
    assert m["tools_used"] == []
    passed += 1
    print("  PASS: compute_metrics handles empty entries")

    # Test 3: all exploration (no generation step)
    entries = [
        {"step": 1, "tool": "grep", "response_tokens": 10, "is_exploration": True},
        {"step": 2, "tool": "grep", "response_tokens": 20, "is_exploration": True},
    ]
    m = compute_metrics(entries)
    assert m["exploration_steps_before_generation"] == 2
    assert m["exploration_tokens"] == 30
    assert m["total_input_tokens"] == 30
    passed += 1
    print("  PASS: compute_metrics handles all-exploration session")

    # Test 4: exploration efficiency ratio
    arm_a = {"exploration_tokens": 5000}
    arm_b = {"exploration_tokens": 2000}
    efficiency = arm_b["exploration_tokens"] / arm_a["exploration_tokens"]
    assert efficiency == 0.4
    passed += 1
    print("  PASS: exploration efficiency ratio = 0.4")

    # Test 5: missing token_count field defaults to 0
    entries = [
        {"step": 1, "tool": "grep", "is_exploration": True},
    ]
    m = compute_metrics(entries)
    assert m["total_input_tokens"] == 0
    passed += 1
    print("  PASS: missing token field defaults to 0")

    print(f"\nAll {passed} tests passed!\n")


if __name__ == "__main__":
    if "--test" in sys.argv:
        _run_tests()
    else:
        if len(sys.argv) < 2:
            print("Usage: python parse_session.py [--test | <path-to-jsonl>]")
            sys.exit(1)

        file_path = sys.argv[1]
        if not Path(file_path).exists():
            print(f"Error: file not found: {file_path}", file=sys.stderr)
            sys.exit(1)

        entries, warnings = parse_jsonl(file_path)
        metrics = compute_metrics(entries)

        if warnings > 0:
            metrics["parser_warnings"] = warnings

        print(json.dumps(metrics, indent=2))
