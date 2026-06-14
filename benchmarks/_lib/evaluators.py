"""Quality evaluators for Phase 2 (regex heuristic) and Phase 3 (LLM judge)."""
from __future__ import annotations

from typing import Any


def evaluate_quality_p2(code: str) -> float:
    """Regex heuristic scorer for Phase 2 code generation.

    Awards 1 point for each of 5 indicators (max score = 1.0):
    1. Uses anyhow::Result
    2. Uses clap::Args or clap::
    3. Has #[derive(Args...)] or #[derive(Debug, Args...)]
    4. Has pub fn execute
    5. Has #[cfg(test)]
    """
    score = 0
    if "use anyhow::Result" in code:
        score += 1
    if "use clap::Args" in code or "use clap::" in code:
        score += 1
    if "#[derive(Args" in code or "#[derive(Debug, Args" in code:
        score += 1
    if "pub fn execute" in code:
        score += 1
    if "#[cfg(test)]" in code:
        score += 1
    return score / 5.0


def evaluate_quality_p3(answer: str, client: Any, model: str) -> float:
    """LLM-judge scorer for Phase 3 architecture Q&A.

    Uses the provided client (OpenAI-compatible) to ask the model to score
    the answer from 0-5 based on coverage of key architecture concepts.
    """
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
            temperature=0.0,
        )
        score_text = response.choices[0].message.content.strip()
        return float(score_text) / 5.0
    except Exception as e:
        print(f"Error evaluating: {e}")
        return 0.0


def evaluate(
    phase: str,
    result_a: Any,
    result_b: Any,
    client: Any,
    model: str,
) -> tuple[float, float]:
    """Dispatch evaluation for the given phase.

    Args:
        phase: "p2" or "p3"
        result_a: AgentResult for Arm A (must have .generated_text)
        result_b: AgentResult for Arm B (must have .generated_text)
        client: OpenAI-compatible client (used by P3 LLM judge)
        model: Model name for LLM judge

    Returns:
        Tuple of (score_a, score_b) each in [0.0, 1.0]

    Raises:
        ValueError: If phase is not "p2" or "p3".
    """
    if phase == "p2":
        return evaluate_quality_p2(result_a.generated_text), evaluate_quality_p2(
            result_b.generated_text
        )
    elif phase == "p3":
        return evaluate_quality_p3(
            result_a.generated_text, client, model
        ), evaluate_quality_p3(result_b.generated_text, client, model)
    else:
        raise ValueError(f"Unknown phase {phase!r}. Expected 'p2' or 'p3'.")
