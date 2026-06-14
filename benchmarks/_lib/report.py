"""Report generation for Phase 2 and Phase 3 benchmarks."""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

RESULTS_DIR: Path = Path(__file__).resolve().parent.parent / "results"


@dataclass
class PhaseReport:
    """Result summary for a two-arm benchmark run."""

    model: str
    arm_a_tokens: int
    arm_a_tool_calls: int | None  # None for Phase 3
    arm_a_quality: float
    arm_b_tokens: int
    arm_b_tool_calls: int | None
    arm_b_quality: float
    token_savings_percent: float


# ---------------------------------------------------------------------------
# Phase 2 report (includes tool_calls)
# ---------------------------------------------------------------------------


def write_phase2_report(report: PhaseReport, path: Path | None = None) -> Path:
    """Write ``phase2_report.json`` matching the legacy format.

    Returns the path the report was written to.
    """
    dest = path or (RESULTS_DIR / "phase2_report.json")
    dest.parent.mkdir(parents=True, exist_ok=True)

    payload = {
        "model": report.model,
        "arm_a": {
            "tokens": report.arm_a_tokens,
            "tool_calls": report.arm_a_tool_calls,
            "quality": report.arm_a_quality,
        },
        "arm_b": {
            "tokens": report.arm_b_tokens,
            "tool_calls": report.arm_b_tool_calls,
            "quality": report.arm_b_quality,
        },
        "metrics": {
            "token_savings_percent": round(report.token_savings_percent, 2),
            "arm_a_quality_score": report.arm_a_quality,
            "arm_b_quality_score": report.arm_b_quality,
        },
    }

    dest.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return dest


# ---------------------------------------------------------------------------
# Phase 3 report (no tool_calls)
# ---------------------------------------------------------------------------


def write_phase3_report(report: PhaseReport, path: Path | None = None) -> Path:
    """Write ``phase3_report.json`` matching the legacy format.

    Returns the path the report was written to.
    """
    dest = path or (RESULTS_DIR / "phase3_report.json")
    dest.parent.mkdir(parents=True, exist_ok=True)

    payload = {
        "model": report.model,
        "arm_a": {
            "tokens": report.arm_a_tokens,
            "quality": report.arm_a_quality,
        },
        "arm_b": {
            "tokens": report.arm_b_tokens,
            "quality": report.arm_b_quality,
        },
        "metrics": {
            "token_savings_percent": round(report.token_savings_percent, 2),
            "arm_a_quality_score": report.arm_a_quality,
            "arm_b_quality_score": report.arm_b_quality,
        },
    }

    dest.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return dest
