#!/usr/bin/env python3
"""Run Phase 2 and/or Phase 3 benchmarks using Anthropic models."""

import argparse
import os
import sys

from anthropic import Anthropic

from _lib.adapters import AnthropicAdapter
from _lib.report import PhaseReport, write_phase2_report, write_phase3_report
from _lib.runner import run_phase


def main():
    parser = argparse.ArgumentParser(description="Run Anthropic benchmarks.")
    parser.add_argument("--model", default="claude-3-5-sonnet-20241022", help="Model identifier")
    parser.add_argument(
        "--phase", choices=["p2", "p3"], required=True, help="Phase to run (p2 or p3)"
    )
    args = parser.parse_args()

    api_key = os.environ.get("ANTHROPIC_API_KEY")
    if not api_key:
        print("Error: ANTHROPIC_API_KEY is not set.", file=sys.stderr)
        sys.exit(1)

    client = Anthropic(api_key=api_key)
    adapter = AnthropicAdapter()

    print(f"Running Phase {args.phase} on {args.model}")

    if args.phase == "p2":
        print("\n--- PHASE 2: Code Generation ---")
        a2, b2, qa2, qb2 = run_phase("p2", args.model, adapter, client, None)
        savings2 = (a2.tokens - b2.tokens) / a2.tokens * 100 if a2.tokens > 0 else 0
        report2 = PhaseReport(
            args.model, a2.tokens, a2.tool_calls, qa2, b2.tokens, b2.tool_calls, qb2, savings2
        )
        write_phase2_report(report2)
        print(f"\nPhase 2 Complete. Token savings: {savings2:.1f}%")
    elif args.phase == "p3":
        print("\n--- PHASE 3: Global Context Answering ---")
        a3, b3, qa3, qb3 = run_phase("p3", args.model, adapter, client, None)
        savings3 = (a3.tokens - b3.tokens) / a3.tokens * 100 if a3.tokens > 0 else 0
        report3 = PhaseReport(
            args.model, a3.tokens, None, qa3, b3.tokens, None, qb3, savings3
        )
        write_phase3_report(report3)
        print(f"\nPhase 3 Complete. Token savings: {savings3:.1f}%")


if __name__ == "__main__":
    main()
