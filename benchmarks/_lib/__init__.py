"""Shared library for VectorCode benchmark scripts."""
from __future__ import annotations

# AgentResult is defined in runner.py (Work Unit 2).
# Re-export here so consumers can do: from benchmarks._lib import AgentResult
# The import is deferred to avoid circular imports until runner exists.


def __getattr__(name: str):
    if name == "AgentResult":
        from benchmarks._lib.runner import AgentResult
        return AgentResult
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
