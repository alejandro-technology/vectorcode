import os, subprocess, json, sys
from pathlib import Path
from anthropic import Anthropic

REPO_ROOT = Path(__file__).resolve().parent.parent

client = Anthropic(api_key=os.environ.get("OPENCODE_API_KEY"), base_url="https://opencode.ai/zen/go")

def _find_vectorcode() -> list[str]:
    env_bin = os.environ.get("VECTORCODE_BIN")
    if env_bin:
        return [env_bin]
    import shutil
    if shutil.which("vectorcode"):
        return ["vectorcode"]
    return ["cargo", "run", "--quiet", "--"]

def tool_execute_bash(command: str) -> str:
    print(f"    [Tool] bash: {command}")
    try:
        res = subprocess.run(command, cwd=REPO_ROOT, shell=True, capture_output=True, text=True, timeout=15.0)
        out = res.stdout
        if res.stderr:
            out += f"\nSTDERR:\n{res.stderr}"
        if not out:
            out = "(Command executed successfully with no output)"
        return out[:8000] # truncate
    except Exception as e:
        return f"Error executing bash: {e}"

def tool_vec_search(query: str) -> str:
    print(f"    [Tool] vec_search: {query}")
    cmd = [*_find_vectorcode(), "search", query, "--json", "--limit", "4"]
    try:
        res = subprocess.run(cmd, cwd=REPO_ROOT, capture_output=True, text=True, timeout=30.0)
        return res.stdout[:15000] # Increased truncation limit since chunks are bigger
    except Exception as e:
        return f"Error searching: {e}"

def tool_read_file(path: str) -> str:
    print(f"    [Tool] read_file: {path}")
    full_path = REPO_ROOT / path
    try:
        return full_path.read_text(encoding="utf-8")[:8000]
    except Exception as e:
        return f"Error reading file: {e}"

def tool_vec_read_lines(path: str, start: int, end: int) -> str:
    print(f"    [Tool] vec_read_lines: {path} ({start}-{end})")
    full_path = REPO_ROOT / path
    try:
        lines = full_path.read_text(encoding="utf-8").splitlines()
        s = max(0, start - 1)
        e = min(len(lines), end)
        if s >= e: return "Error: invalid range"
        extracted = "\\n".join(lines[s:e])
        return f"Lines {start}-{end} of {path}:\\n{extracted}"
    except Exception as e:
        return f"Error reading lines: {e}"


def run_agent(arm_id, model, system_prompt, task, use_read_file=True):
    tools = []
    if arm_id == "A":
        tools = [{"name": "execute_bash", "description": "Execute a bash command (e.g. grep, find)", "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}}]
        if use_read_file:
            tools.append({"name": "read_file", "description": "Read full contents of a file", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}})
    else:
        tools = [{"name": "vec_search", "description": "Semantic search over the codebase. Returns code snippets.", "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}]
        if use_read_file:
            tools.append({"name": "vec_read_lines", "description": "Read specific lines of a file", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}, "start_line": {"type": "integer"}, "end_line": {"type": "integer"}}, "required": ["path", "start_line", "end_line"]}})

    messages = [{"role": "user", "content": task}]
    
    total_tokens = 0
    final_text = ""
    
    print(f"\n--- Starting Arm {arm_id} ({model}) ---")
    for step in range(15):
        try:
            response = client.messages.create(
                model=model,
                max_tokens=2000,
                system=system_prompt,
                messages=messages,
                tools=tools,
                temperature=0.0
            )
        except Exception as e:
            print("API Error:", e)
            break
            
        total_tokens += response.usage.input_tokens
        messages.append({"role": "assistant", "content": response.content})
        
        tool_calls = [b for b in response.content if b.type == "tool_use"]
        if not tool_calls:
            text_blocks = [b for b in response.content if b.type == "text"]
            if text_blocks: final_text = text_blocks[0].text
            print(f"    [Done] Generated ({len(final_text)} chars)")
            break
            
        tool_results = []
        for tc in tool_calls:
            args = tc.input
            if tc.name == "execute_bash": res = tool_execute_bash(args.get("command", ""))
            elif tc.name == "vec_search": res = tool_vec_search(args.get("query", ""))
            elif tc.name == "read_file": res = tool_read_file(args.get("path", ""))
            elif tc.name == "vec_read_lines": res = tool_vec_read_lines(args.get("path", ""), args.get("start_line", 1), args.get("end_line", 1))
            else: res = "Unknown tool"
            tool_results.append({
                "type": "tool_result",
                "tool_use_id": tc.id,
                "content": res
            })
        messages.append({"role": "user", "content": tool_results})
        
    return total_tokens, final_text

if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default="minimax-m3")
    args = parser.parse_args()

    sys_p2 = "You are an expert Rust developer agent. Your task is to explore the codebase using the provided tools, understand the patterns, and generate the requested code. Once you have enough context, output ONLY the final Rust code."
    task_p2 = "Add a new CLI `status` subcommand that displays index health statistics, following the exact same conventions as the existing `install` CLI subcommand in `src/cli/install.rs`."

    sys_p3 = "You are an expert Rust developer agent. Your task is to explore the codebase using the provided tools, understand the global architecture, and answer the question. Once you have enough context, output ONLY your final answer."
    task_p3 = "Explica la arquitectura del sistema de embeddings de este proyecto: ¿qué trait principal se usa, cuáles son sus métodos, qué proveedores lo implementan y en qué parte del código se instancia el proveedor según la configuración?"

    print(f"Running P2 & P3 on {args.model}")
    
    a2_tok, a2_res = run_agent("A", args.model, sys_p2, task_p2, True)
    b2_tok, b2_res = run_agent("B", args.model, sys_p2, task_p2, True)
    
    a3_tok, a3_res = run_agent("A", args.model, sys_p3, task_p3, True)
    b3_tok, b3_res = run_agent("B", args.model, sys_p3, task_p3, False)

    print("\nFINAL RESULTS:")
    print(f"P2 Arm A: {a2_tok} tokens")
    print(f"P2 Arm B: {b2_tok} tokens")
    print(f"P3 Arm A: {a3_tok} tokens")
    print(f"P3 Arm B: {b3_tok} tokens")
