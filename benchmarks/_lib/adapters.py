"""SDK abstraction layer — normalises OpenAI and Anthropic API differences.

Runner.py talks exclusively through SDKAdapter so it never branches on
provider-specific response shapes.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Protocol


# ---------------------------------------------------------------------------
# Shared data classes
# ---------------------------------------------------------------------------


@dataclass
class ToolDef:
    """Canonical tool definition (SDK-agnostic)."""

    name: str
    description: str
    params: dict  # JSON Schema dict


@dataclass
class ToolCall:
    """A single tool invocation extracted from a response."""

    id: str
    name: str
    args: dict  # Already parsed


# ---------------------------------------------------------------------------
# Protocol
# ---------------------------------------------------------------------------


class SDKAdapter(Protocol):
    def format_tools(self, tools: list[ToolDef]) -> list[dict]: ...
    def send_request(
        self,
        client: Any,
        model: str,
        messages: list[dict],
        tools: list[dict],
        system: str,
        effort: str | None,
        temperature: float,
    ) -> Any: ...
    def extract_tool_calls(self, response: Any) -> list[ToolCall]: ...
    def extract_tokens(self, response: Any) -> int: ...
    def format_tool_result(self, tool_call_id: str, name: str, content: str) -> dict: ...
    def append_assistant(self, messages: list[dict], response: Any) -> None: ...
    def extract_final_text(self, response: Any) -> str: ...


# ---------------------------------------------------------------------------
# OpenAI adapter
# ---------------------------------------------------------------------------


class OpenAIAdapter:
    """Adapter for OpenAI-compatible chat completions API."""

    def format_tools(self, tools: list[ToolDef]) -> list[dict]:
        return [
            {
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.params,
                },
            }
            for t in tools
        ]

    def send_request(
        self,
        client: Any,
        model: str,
        messages: list[dict],
        tools: list[dict],
        system: str,
        effort: str | None,
        temperature: float,
    ) -> Any:
        kwargs: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "tools": tools or None,
        }
        if effort:
            kwargs["extra_body"] = {"reasoning_effort": effort}
        else:
            kwargs["temperature"] = temperature
        return client.chat.completions.create(**kwargs)

    def extract_tool_calls(self, response: Any) -> list[ToolCall]:
        msg = response.choices[0].message
        if not msg.tool_calls:
            return []
        return [
            ToolCall(
                id=tc.id,
                name=tc.function.name,
                args=json.loads(tc.function.arguments),
            )
            for tc in msg.tool_calls
        ]

    def extract_tokens(self, response: Any) -> int:
        if response.usage:
            return response.usage.prompt_tokens
        return 0

    def format_tool_result(self, tool_call_id: str, name: str, content: str) -> dict:
        return {
            "role": "tool",
            "tool_call_id": tool_call_id,
            "name": name,
            "content": content,
        }

    def append_assistant(self, messages: list[dict], response: Any) -> None:
        messages.append(response.choices[0].message)

    def extract_final_text(self, response: Any) -> str:
        return response.choices[0].message.content or ""


# ---------------------------------------------------------------------------
# Anthropic adapter
# ---------------------------------------------------------------------------


class AnthropicAdapter:
    """Adapter for the Anthropic Messages API."""

    def format_tools(self, tools: list[ToolDef]) -> list[dict]:
        return [
            {
                "name": t.name,
                "description": t.description,
                "input_schema": t.params,
            }
            for t in tools
        ]

    def send_request(
        self,
        client: Any,
        model: str,
        messages: list[dict],
        tools: list[dict],
        system: str,
        effort: str | None,
        temperature: float,
    ) -> Any:
        anthropic_msgs = self._merge_messages(messages)
        return client.messages.create(
            model=model,
            max_tokens=2000,
            system=system,
            messages=anthropic_msgs,
            tools=tools or None,
            temperature=temperature,
        )

    def extract_tool_calls(self, response: Any) -> list[ToolCall]:
        calls: list[ToolCall] = []
        for block in response.content:
            if getattr(block, "type", None) == "tool_use":
                calls.append(
                    ToolCall(
                        id=block.id,
                        name=block.name,
                        args=block.input if isinstance(block.input, dict) else {},
                    )
                )
        return calls

    def extract_tokens(self, response: Any) -> int:
        return response.usage.input_tokens

    def format_tool_result(self, tool_call_id: str, name: str, content: str) -> dict:
        return {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content,
                }
            ],
        }

    def append_assistant(self, messages: list[dict], response: Any) -> None:
        messages.append({"role": "assistant", "content": response.content})

    def extract_final_text(self, response: Any) -> str:
        parts: list[str] = []
        for block in response.content:
            if getattr(block, "type", None) == "text":
                parts.append(block.text)
        return "".join(parts)

    # -- internal helpers ---------------------------------------------------

    @staticmethod
    def _merge_messages(messages: list[dict]) -> list[dict]:
        """Collapse consecutive same-role messages for the Anthropic API.

        The runner appends one dict per tool-result; Anthropic requires them
        batched inside a single ``user`` message.
        """
        merged: list[dict] = []
        for msg in messages:
            role = msg.get("role")
            if role == "system":
                continue  # system is passed as a top-level parameter

            content = msg.get("content")

            if merged and merged[-1]["role"] == role:
                prev = merged[-1]["content"]
                if isinstance(content, list) and isinstance(prev, list):
                    prev.extend(content)
                elif isinstance(content, list):
                    merged[-1]["content"] = [{"type": "text", "text": str(prev)}] + content  # type: ignore[arg-type]
                elif isinstance(prev, list):
                    prev.append({"type": "text", "text": str(content)})
                else:
                    merged[-1]["content"] = str(prev) + "\n" + str(content)
            else:
                merged.append({"role": role, "content": content})
        return merged
