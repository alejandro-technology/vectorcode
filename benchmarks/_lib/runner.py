"""Agent runner — SDK-agnostic agentic loop shared by all benchmark scripts."""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from .adapters import SDKAdapter, ToolCall
from .tools import TOOL_REGISTRY


# ---------------------------------------------------------------------------
# Result container
# ---------------------------------------------------------------------------


@dataclass
class AgentResult:
    """Outcome of a single agent arm run."""

    tokens: int
    tool_calls: int
    generated_text: str
    steps: list[dict] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Core agent loop
# ---------------------------------------------------------------------------


def run_agent(
    arm_id: str,
    adapter: SDKAdapter,
    client: Any,
    model: str,
    system_prompt: str,
    task: str,
    tools: list[dict],
    effort: str | None = None,
    temperature: float = 0.0,
    max_steps: int = 15,
) -> AgentResult:
    """Run a single agent arm.

    Executes the agentic loop: send request → extract tool calls → execute
    tools → feed results back → repeat until the model stops calling tools
    or *max_steps* is reached.

    Args:
        arm_id: Label for logging (e.g. "A" or "B").
        adapter: SDK-specific adapter implementing :class:`SDKAdapter`.
        client: Pre-authenticated SDK client.
        model: Model identifier.
        system_prompt: System-level instruction.
        task: The user task prompt.
        tools: Tool definitions already formatted for the target SDK.
        effort: Optional reasoning-effort hint (OpenAI o-series only).
        temperature: Sampling temperature (ignored when *effort* is set).
        max_steps: Safety cap on agentic turns.

    Returns:
        :class:`AgentResult` with token count, tool-call count, generated
        text, and a step log.
    """
    messages: list[dict] = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": task},
    ]

    total_tokens = 0
    tool_calls_count = 0
    generated_text = ""
    steps: list[dict] = []

    print(f"\n--- Starting Agent (Arm {arm_id}) ---")

    for _ in range(max_steps):
        response = adapter.send_request(
            client, model, messages, tools, system_prompt, effort, temperature
        )
        total_tokens += adapter.extract_tokens(response)

        tool_calls = adapter.extract_tool_calls(response)

        if not tool_calls:
            generated_text = adapter.extract_final_text(response)
            print(f"    [Done] Generated text ({len(generated_text)} chars)")
            break

        adapter.append_assistant(messages, response)

        for tc in tool_calls:
            tool_calls_count += 1
            func = TOOL_REGISTRY.get(tc.name)
            if func:
                res = func(**tc.args) if isinstance(tc.args, dict) else func(tc.args)
            else:
                res = f"Unknown tool: {tc.name}"

            result_msg = adapter.format_tool_result(tc.id, tc.name, res[:8000])
            messages.append(result_msg)
            steps.append(
                {"tool": tc.name, "args": tc.args, "response_chars": len(res)}
            )

    return AgentResult(
        tokens=total_tokens,
        tool_calls=tool_calls_count,
        generated_text=generated_text,
        steps=steps,
    )


# ---------------------------------------------------------------------------
# Phase-level runner
# ---------------------------------------------------------------------------


def run_phase(
    phase: str,
    model: str,
    adapter: SDKAdapter,
    client: Any,
    effort: str | None = None,
) -> tuple[AgentResult, AgentResult, float, float]:
    """Run both arms for a phase and evaluate quality.

    Args:
        phase: ``"p2"`` or ``"p3"``.
        model: Model identifier.
        adapter: SDK-specific adapter.
        client: Pre-authenticated SDK client.
        effort: Optional reasoning-effort hint.

    Returns:
        Tuple of ``(result_a, result_b, quality_a, quality_b)``.
    """
    from .evaluators import evaluate
    from .prompts import get as get_prompt
    from .tools import build_arm_a_tools, build_arm_b_tools

    sys_prompt, task = get_prompt(phase)

    use_read_file_a = True
    use_read_file_b = phase == "p2"

    tools_a_defs = build_arm_a_tools(use_read_file_a)
    tools_b_defs = build_arm_b_tools(use_read_file_b)

    tools_a = adapter.format_tools(tools_a_defs)
    tools_b = adapter.format_tools(tools_b_defs)

    result_a = run_agent("A", adapter, client, model, sys_prompt, task, tools_a, effort)
    result_b = run_agent("B", adapter, client, model, sys_prompt, task, tools_b, effort)

    qa, qb = evaluate(phase, result_a, result_b, client, model)

    return result_a, result_b, qa, qb
