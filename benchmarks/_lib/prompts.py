"""Prompt constants for Phase 2 and Phase 3 benchmarks."""
from __future__ import annotations

SYS_P2: str = (
    "You are an expert Rust developer agent. "
    "Your task is to explore the codebase using the provided tools, understand the patterns, "
    "and generate the requested code. Do not guess the code. Use the tools to find existing conventions. "
    "Once you have enough context, output ONLY the final Rust code. "
    "Do not wrap it in markdown block if possible, or just provide the final code without conversational filler."
)

SYS_P3: str = (
    "You are an expert Rust developer agent. "
    "Your task is to explore the codebase using the provided tools, understand the global architecture, "
    "and answer the question. Do not guess the answer. Use the tools to find the actual code. "
    "Once you have enough context, output ONLY your final answer."
)

TASK_P2: str = (
    "Add a new CLI `status` subcommand that displays index health statistics, "
    "following the exact same conventions as the existing `install` CLI subcommand "
    "in `src/cli/install.rs`."
)

TASK_P3: str = (
    "Explica la arquitectura del sistema de embeddings de este proyecto: "
    "\u00bfqu\u00e9 trait principal se usa, cu\u00e1les son sus m\u00e9todos, "
    "qu\u00e9 proveedores lo implementan y en qu\u00e9 parte del c\u00f3digo "
    "se instancia el proveedor seg\u00fan la configuraci\u00f3n?"
)


def get(phase: str) -> tuple[str, str]:
    """Return (system_prompt, task) tuple for the given phase.

    Args:
        phase: "p2" or "p3"

    Returns:
        Tuple of (system_prompt, task_prompt)

    Raises:
        ValueError: If phase is not "p2" or "p3".
    """
    if phase == "p2":
        return SYS_P2, TASK_P2
    elif phase == "p3":
        return SYS_P3, TASK_P3
    else:
        raise ValueError(f"Unknown phase {phase!r}. Expected 'p2' or 'p3'.")
