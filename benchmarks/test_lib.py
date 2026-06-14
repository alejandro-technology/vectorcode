import unittest
from pathlib import Path
from dataclasses import dataclass
from typing import Any

from _lib.adapters import OpenAIAdapter, AnthropicAdapter, ToolDef, ToolCall
from _lib.report import PhaseReport
from _lib.tools import build_arm_a_tools, build_arm_b_tools
from _lib.evaluators import evaluate_quality_p2

class TestAdapters(unittest.TestCase):
    def test_openai_adapter_format_tools(self):
        adapter = OpenAIAdapter()
        tools = [ToolDef("test", "test desc", {"type": "object"})]
        formatted = adapter.format_tools(tools)
        self.assertEqual(formatted[0]["type"], "function")
        self.assertEqual(formatted[0]["function"]["name"], "test")

    def test_anthropic_adapter_format_tools(self):
        adapter = AnthropicAdapter()
        tools = [ToolDef("test", "test desc", {"type": "object"})]
        formatted = adapter.format_tools(tools)
        self.assertEqual(formatted[0]["name"], "test")
        self.assertEqual(formatted[0]["input_schema"]["type"], "object")

    def test_anthropic_merge_messages(self):
        messages = [
            {"role": "user", "content": "hello"},
            {"role": "user", "content": [{"type": "text", "text": "world"}]}
        ]
        merged = AnthropicAdapter._merge_messages(messages)
        self.assertEqual(len(merged), 1)
        self.assertEqual(len(merged[0]["content"]), 2)
        self.assertEqual(merged[0]["role"], "user")

class TestTools(unittest.TestCase):
    def test_build_arm_a_tools(self):
        tools = build_arm_a_tools(use_read_file=True)
        self.assertEqual(len(tools), 2)
        names = [t["function"]["name"] for t in tools]
        self.assertIn("execute_bash", names)
        self.assertIn("read_file", names)

        tools_no_read = build_arm_a_tools(use_read_file=False)
        self.assertEqual(len(tools_no_read), 1)

    def test_build_arm_b_tools(self):
        tools = build_arm_b_tools(use_read_file=True)
        self.assertEqual(len(tools), 3)
        names = [t["function"]["name"] for t in tools]
        self.assertIn("vec_search", names)
        self.assertIn("vec_read_lines", names)

class TestEvaluators(unittest.TestCase):
    def test_evaluate_quality_p2(self):
        @dataclass
        class DummyResult:
            generated_text: str
        
        # Perfect text
        good_text = """
        use anyhow::Result;
        use clap::Args;
        #[derive(Args)]
        pub struct StatusArgs {}
        pub fn execute() {}
        #[cfg(test)]
        mod tests {}
        """
        score = evaluate_quality_p2(good_text)
        self.assertEqual(score, 1.0)

        # Missing async and struct
        bad_text = "fn run() {}"
        bad_score = evaluate_quality_p2(bad_text)
        self.assertLess(bad_score, 1.0)

class TestReport(unittest.TestCase):
    def test_phase_report_creation(self):
        report = PhaseReport("model", 100, 5, 100.0, 50, 2, 80.0, 50.0)
        self.assertEqual(report.token_savings_percent, 50.0)

if __name__ == "__main__":
    unittest.main()
