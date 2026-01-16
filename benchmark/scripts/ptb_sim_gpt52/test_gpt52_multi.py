#!/usr/bin/env python3
"""
Test GPT-5.2 on multiple PTBs with batched tool calls.

The LLM has only 3 iterations per PTB to:
1. Explore modules
2. Compile any needed code
3. Execute PTB and submit

Usage:
    OPENROUTER_API_KEY=... python benchmark/scripts/ptb_sim_gpt52/test_gpt52_multi.py
"""

import base64
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

import httpx

# Load environment from .env file
ENV_PATH = Path(__file__).parent.parent.parent / ".env"
if ENV_PATH.exists():
    for line in ENV_PATH.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            os.environ.setdefault(key, value)

# Configuration
MODEL = os.environ.get("SMI_MODEL", "openai/gpt-5.2")
BASE_URL = "https://openrouter.ai/api/v1"
MAX_ITERATIONS = 3
RUST_BIN = Path("target/release/sui_move_interface_extractor")

# Test transactions - moderately complex DeFi operations
TEST_TRANSACTIONS = [
    {
        "name": "DeepBook Order Placement",
        "digest": "23CGb43kq21r6w5VK4zR9zMCpcYBmvrvsyuDmWXaRhRB",
        "description": "Places a limit order on DeepBook DEX.",
    },
    {
        "name": "Flashloan Swap",
        "digest": "6adrRRJ7WbHm1kyp2mNPMwAnt8uk8EyBxhvLi6ypv36X",
        "description": "Borrows via flashloan, swaps through DeepBook, confirms via router.",
    },
]

TOOL_DESCRIPTIONS = """
Available tools. You may call MULTIPLE tools in a single response by returning a JSON array.

Return either:
- A single tool call: {"tool": "...", "args": {...}}
- Multiple tool calls: [{"tool": "...", "args": {...}}, {"tool": "...", "args": {...}}, ...]

## Introspection Tools
1. list_modules - List all loaded Move modules
   Args: none

2. list_cached_objects - List all pre-loaded objects available for use in PTB execution
   Args: none
   Returns: Object IDs, types, shared status, and byte sizes

3. list_functions - List all functions in a module
   Args: {"module_path": "0x...::module_name"}

4. list_structs - List all struct types in a module
   Args: {"module_path": "0x...::module_name"}

5. get_function_info - Get function signature details
   Args: {"module_path": "...", "function_name": "..."}

6. get_struct_info - Get struct type definition
   Args: {"type_path": "0x...::module::TypeName"}

7. find_constructors - Find functions that return a given type
   Args: {"type_path": "TypeName"}

8. search_functions - Search for functions matching a pattern
   Args: {"pattern": "*swap*", "entry_only": false}

9. disassemble_function - Get bytecode disassembly
   Args: {"module_path": "...", "function_name": "..."}

## Compilation Tools
10. compile_move - Compile Move source code and deploy to sandbox
    Args: {"package_name": "my_pkg", "module_name": "my_module", "source": "module my_pkg::my_module { ... }"}

## Execution Tools
11. execute_ptb - Execute a programmable transaction block
    Args: {
      "inputs": [{"type": "pure", "value": ..., "value_type": "u64"}, {"type": "object", "object_id": "0x..."}, ...],
      "commands": [{"type": "move_call", "package": "0x...", "module": "...", "function": "...", "type_args": [], "args": [0, 1]}]
    }
    Note: args are integer indices into the inputs array, or {"cmd": N, "idx": M} for command results

12. submit_solution - Submit when you've successfully simulated the PTB
    Args: {"success": true, "summary": "..."}
"""


class SandboxProcess:
    """Wrapper for the sandbox subprocess."""

    def __init__(self):
        self.process = None

    def start(self):
        cmd = [str(RUST_BIN), "sandbox-exec", "--interactive"]
        self.process = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )

    def execute(self, action: str, **kwargs) -> dict:
        request = {"action": action, **kwargs}
        req_json = json.dumps(request) + "\n"
        self.process.stdin.write(req_json)
        self.process.stdin.flush()
        resp = self.process.stdout.readline()
        if not resp:
            return {"error": "No response from sandbox"}
        return json.loads(resp)

    def load_packages(self, packages: dict) -> int:
        """Load packages from cached PTB data."""
        count = 0
        for pkg_id, pkg_data in packages.items():
            if isinstance(pkg_data, list):
                for module_name, b64_bytecode in pkg_data:
                    bytecode = base64.b64decode(b64_bytecode)
                    with tempfile.NamedTemporaryFile(suffix=".mv", delete=False) as f:
                        f.write(bytecode)
                        temp_path = f.name
                    result = self.execute("load_module", bytecode_path=temp_path)
                    os.unlink(temp_path)
                    if result.get("success"):
                        count += 1
        return count

    def load_cached_objects(self, data: dict) -> int:
        """Load cached objects with proper shared status and type info from transaction cache."""
        objects = data.get("objects", {})
        object_types = data.get("object_types", {})
        inputs = data.get("transaction", {}).get("inputs", [])

        # Build set of shared object IDs from transaction inputs
        shared_object_ids = []
        for inp in inputs:
            if inp.get("type") == "SharedObject":
                obj_id = inp.get("object_id", "")
                if obj_id:
                    shared_object_ids.append(obj_id)

        # Load all objects via the batch action
        result = self.execute(
            "load_cached_objects",
            objects=objects,
            object_types=object_types,
            shared_object_ids=shared_object_ids,
        )
        return result.get("data", {}).get("loaded", 0) if result.get("success") else 0

    def close(self):
        if self.process:
            self.process.terminate()
            self.process.wait(timeout=5)


def get_api_key():
    key = os.environ.get("OPENROUTER_API_KEY") or os.environ.get("SMI_API_KEY")
    if not key:
        print("ERROR: Set OPENROUTER_API_KEY environment variable")
        sys.exit(1)
    return key


def call_llm(api_key: str, messages: list[dict], temperature: float = 0.0) -> str:
    """Call LLM via OpenRouter."""
    url = f"{BASE_URL}/chat/completions"
    headers = {
        "Authorization": f"Bearer {api_key}",
        "HTTP-Referer": "https://github.com/anthropics/sui-sandbox",
        "X-Title": "Sui Move PTB Test",
    }
    payload = {
        "model": MODEL,
        "temperature": temperature,
        "messages": messages,
        "max_tokens": 4096,
    }

    with httpx.Client(timeout=180) as client:
        r = client.post(url, headers=headers, json=payload)
        r.raise_for_status()
        data = r.json()
        return data["choices"][0]["message"]["content"]


def load_transaction_context(digest: str) -> dict | None:
    """Load transaction data from cache."""
    cache_file = f".tx-cache/{digest}.json"
    if not os.path.exists(cache_file):
        print(f"  Cache file not found: {cache_file}")
        return None

    with open(cache_file) as f:
        return json.load(f)


def extract_transaction_summary(data: dict) -> str:
    """Extract a summary of the transaction for the prompt."""
    tx = data.get("transaction", {})
    commands = tx.get("commands", [])

    summary_lines = [
        f"Transaction digest: {tx.get('digest', 'unknown')}",
        f"Sender: {tx.get('sender', 'unknown')}",
        f"Total commands: {len(commands)}",
        "",
        "Commands:",
    ]

    for i, cmd in enumerate(commands):
        if cmd.get("type") == "MoveCall":
            pkg = cmd.get("package", "")[:16] + "..."
            mod = cmd.get("module", "")
            func = cmd.get("function", "")
            summary_lines.append(f"  {i}. MoveCall: {mod}::{func}")
        else:
            summary_lines.append(f"  {i}. {cmd.get('type', 'Unknown')}")

    return "\n".join(summary_lines)


def parse_tool_calls(response: str) -> list[dict]:
    """Parse tool calls from LLM response. Supports single or batched calls."""
    # Strip markdown code blocks if present
    if "```json" in response:
        response = response.split("```json")[1].split("```")[0]
    elif "```" in response:
        parts = response.split("```")
        if len(parts) >= 2:
            response = parts[1]

    response = response.strip()

    # Try to find JSON in the response - be more careful with matching
    json_match = re.search(r"(\[[\s\S]*?\]|\{[\s\S]*?\})(?=\s*$|\s*\[|\s*\{)", response)
    if json_match:
        response = json_match.group(1)

    try:
        parsed = json.loads(response)
        if isinstance(parsed, dict):
            return [parsed]
        elif isinstance(parsed, list):
            return parsed
        else:
            return []
    except json.JSONDecodeError:
        return []


def execute_tool(sandbox: SandboxProcess, tool: str, args: dict) -> dict:
    """Execute a single tool call against the sandbox."""
    tool_lower = tool.lower().replace("_", "")

    if tool_lower in ("listmodules", "list_modules"):
        return sandbox.execute("list_modules")

    elif tool_lower in ("listfunctions", "list_functions"):
        module_path = args.get("module_path", "")
        return sandbox.execute("list_functions", module_path=module_path)

    elif tool_lower in ("liststructs", "list_structs"):
        module_path = args.get("module_path", "")
        return sandbox.execute("list_structs", module_path=module_path)

    elif tool_lower in ("getfunctioninfo", "get_function_info"):
        module_path = args.get("module_path", "")
        function_name = args.get("function_name", "")
        return sandbox.execute("get_function_info", module_path=module_path, function_name=function_name)

    elif tool_lower in ("getstructinfo", "get_struct_info"):
        type_path = args.get("type_path", "")
        return sandbox.execute("get_struct_info", type_path=type_path)

    elif tool_lower in ("findconstructors", "find_constructors"):
        type_path = args.get("type_path", "")
        return sandbox.execute("find_constructors", type_path=type_path)

    elif tool_lower in ("searchfunctions", "search_functions"):
        pattern = args.get("pattern", "*")
        entry_only = args.get("entry_only", False)
        return sandbox.execute("search_functions", pattern=pattern, entry_only=entry_only)

    elif tool_lower in ("disassemblefunction", "disassemble_function"):
        module_path = args.get("module_path", "")
        function_name = args.get("function_name", "")
        return sandbox.execute("disassemble_function", module_path=module_path, function_name=function_name)

    elif tool_lower in ("compilemove", "compile_move"):
        package_name = args.get("package_name", "")
        module_name = args.get("module_name", "")
        source = args.get("source", "")
        return sandbox.execute("compile_move", package_name=package_name, module_name=module_name, source=source)

    elif tool_lower in ("executeptb", "execute_ptb"):
        inputs = args.get("inputs", [])
        commands = args.get("commands", [])
        return sandbox.execute("execute_ptb", inputs=inputs, commands=commands)

    elif tool_lower in ("submitsolution", "submit_solution"):
        return {"submitted": True, "success": args.get("success", False), "summary": args.get("summary", "")}

    elif tool_lower in ("listcachedobjects", "list_cached_objects"):
        return sandbox.execute("list_cached_objects")

    else:
        return {"error": f"Unknown tool: {tool}"}


def test_transaction(api_key: str, tx_config: dict) -> dict:
    """Test LLM on a single transaction with batched tool calls."""
    print(f"\n{'=' * 60}")
    print(f"Testing: {tx_config['name']}")
    print(f"Digest: {tx_config['digest']}")
    print(f"Description: {tx_config['description']}")
    print("=" * 60)

    # Load transaction data
    data = load_transaction_context(tx_config["digest"])
    if not data:
        return {"error": "Transaction not found in cache"}

    # Start sandbox and load packages
    sandbox = SandboxProcess()
    sandbox.start()

    packages = data.get("packages", {})
    module_count = sandbox.load_packages(packages)
    print(f"\nLoaded {module_count} modules into sandbox")

    # Load cached objects with proper shared status and type info
    object_count = sandbox.load_cached_objects(data)
    print(f"Loaded {object_count} objects into sandbox")

    tx_summary = extract_transaction_summary(data)
    print(f"\n{tx_summary}")

    # Build initial system prompt with clear iteration constraint
    system_prompt = f"""You are simulating a Sui PTB in a sandbox environment.

CRITICAL: You have only {MAX_ITERATIONS} iterations total to complete this task. You must:
1. Quickly explore the loaded modules (iteration 1)
2. Write and compile any needed Move modules (iteration 2)
3. Execute the PTB and submit your solution (iteration 3)

Be efficient - call multiple tools per iteration. Don't spend all iterations just exploring.

{TOOL_DESCRIPTIONS}

## Transaction to Simulate
{tx_summary}

Your goal: Successfully simulate this PTB by understanding the interfaces, compiling any needed modules, and executing the transaction."""

    messages = [{"role": "system", "content": system_prompt}]
    messages.append(
        {
            "role": "user",
            "content": f"You have {MAX_ITERATIONS} iterations. Start by calling list_modules and search_functions to understand what's available. Be efficient!",
        }
    )

    results = []
    submitted = False

    for iteration in range(MAX_ITERATIONS):
        remaining = MAX_ITERATIONS - iteration
        print(f"\n--- Iteration {iteration + 1}/{MAX_ITERATIONS} ({remaining} remaining) ---")

        try:
            response = call_llm(api_key, messages)
            print(f"Response: {response[:200]}...")

            tool_calls = parse_tool_calls(response)
            print(f"Parsed {len(tool_calls)} tool call(s)")

            if not tool_calls:
                print("No valid tool calls found in response")
                messages.append({"role": "assistant", "content": response})
                messages.append(
                    {
                        "role": "user",
                        "content": f"Please respond with valid JSON tool calls. You have {remaining - 1} iterations left!",
                    }
                )
                continue

            # Execute all tool calls (batched)
            tool_results = []
            for tc in tool_calls:
                tool = tc.get("tool", "")
                args = tc.get("args", {})
                print(f"  Tool: {tool}")

                result = execute_tool(sandbox, tool, args)
                tool_results.append({"tool": tool, "result": result})

                # Check for submission
                if tool.lower().replace("_", "") == "submitsolution":
                    submitted = True
                    results.append(
                        {
                            "iteration": iteration + 1,
                            "submitted": True,
                            "success": result.get("success", False),
                            "summary": result.get("summary", ""),
                        }
                    )

            if submitted:
                break

            # Build response message with all results
            results_str = json.dumps(tool_results, indent=2, default=str)
            if len(results_str) > 8000:
                results_str = results_str[:8000] + "\n... (truncated)"

            messages.append({"role": "assistant", "content": response})

            # Add urgency based on remaining iterations
            if remaining == 2:
                urgency = "You have 2 iterations left. If you need to compile modules, do it now. Next iteration should be execution!"
            elif remaining == 1:
                urgency = "FINAL ITERATION! You must execute the PTB and call submit_solution now!"
            else:
                urgency = f"You have {remaining - 1} iterations remaining after this."

            messages.append({"role": "user", "content": f"Tool results:\n{results_str}\n\n{urgency}"})

            results.append(
                {
                    "iteration": iteration + 1,
                    "tools_called": len(tool_calls),
                    "tool_names": [tc.get("tool") for tc in tool_calls],
                }
            )

        except Exception as e:
            print(f"Error: {e}")
            results.append({"iteration": iteration + 1, "error": str(e)})

    sandbox.close()

    return {
        "transaction": tx_config["name"],
        "digest": tx_config["digest"],
        "iterations": len(results),
        "submitted": submitted,
        "results": results,
    }


def main():
    print("=" * 60)
    print("GPT-5.2 Multi-PTB Test with Batched Tool Calls")
    print("=" * 60)
    print(f"Model: {MODEL}")
    print(f"Max iterations per PTB: {MAX_ITERATIONS}")
    print(f"PTBs to test: {len(TEST_TRANSACTIONS)}")
    print()

    api_key = get_api_key()
    print("API key loaded")

    all_results = []

    for tx_config in TEST_TRANSACTIONS:
        result = test_transaction(api_key, tx_config)
        all_results.append(result)

    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)

    for result in all_results:
        status = "SUBMITTED" if result.get("submitted") else "INCOMPLETE"
        print(f"\n{result['transaction']}: {status}")
        print(f"  Iterations: {result.get('iterations', 0)}")
        for r in result.get("results", []):
            if r.get("tools_called"):
                print(f"    Iter {r['iteration']}: {r['tools_called']} tools - {r.get('tool_names', [])}")
            elif r.get("submitted"):
                print(f"    Iter {r['iteration']}: SUBMITTED (success={r.get('success')})")

    print("\n" + "=" * 60)


if __name__ == "__main__":
    main()
