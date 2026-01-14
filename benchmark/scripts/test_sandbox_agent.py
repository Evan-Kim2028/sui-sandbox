#!/usr/bin/env python3
"""
Test the sandbox agent with full tool access on the LST package.

Usage:
    SMI_MODEL=openai/gpt-5.2 python benchmark/scripts/test_sandbox_agent.py
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import httpx

# Configuration
MAX_ITERATIONS = 15
PACKAGE_DIR = Path("benchmark/corpus/0x05/0x83556891f4a0f233ce7b05cfe7f957d4020492a34f5405b2cb9377d060bef4bf")
BYTECODE_DIR = PACKAGE_DIR / "bytecode_modules"
RUST_BIN = Path("target/release/sui_move_interface_extractor")

# Target types to create - challenging but achievable
# Coin requires coin::zero() or coin::from_balance()
# Option requires option::none() or option::some()
TARGET_TYPES = [
    "Coin",          # Multi-path: coin::zero() or coin::from_balance(balance::zero())
    "Option",        # option::none() or option::some(T)
]

TOOL_DESCRIPTIONS = """
Available tools (respond with JSON containing "tool" and "args" fields):

## Introspection Tools
1. list_modules - List all loaded Move modules
   Args: none

2. list_functions - List all functions in a module
   Args: {"module_path": "0x...::module_name"}

3. list_structs - List all struct types in a module
   Args: {"module_path": "0x...::module_name"}

4. get_function_info - Get function signature details
   Args: {"module_path": "...", "function_name": "..."}

5. get_struct_info - Get struct type definition (fields, abilities, type params)
   Args: {"type_path": "0x...::module::TypeName" or just "TypeName"}

6. find_constructors - Find functions that return a given type
   Args: {"type_path": "TypeName"}

7. search_functions - Search for functions matching a pattern
   Args: {"pattern": "*new*", "entry_only": false}

8. search_types - Search for types matching a pattern
   Args: {"pattern": "Coin", "ability_filter": "key"}  (ability_filter optional)

9. disassemble_function - Get bytecode disassembly of a function
   Args: {"module_path": "...", "function_name": "..."}

## Compilation Tools
10. compile_move - Compile Move source code and deploy to sandbox
    Args: {"package_name": "my_pkg", "module_name": "my_module", "source": "module my_pkg::my_module { ... }"}
    Returns: {"compiled": true, "deployed": true, "package_id": "0x..."}

## Environment Tools
11. set_time - Set sandbox clock time (milliseconds)
    Args: {"timestamp_ms": 1234567890000}

12. get_time - Get current sandbox clock time
    Args: none

## Submission
13. submit_ptb_plan - Submit final PTB (call when done)
    Args: {"calls": [...], "reasoning": "..."}
    Each call: {"target": "0x...::mod::func", "type_args": ["T"], "args": [...]}
"""


class SandboxProcess:
    """Wrapper for the sandbox subprocess."""

    def __init__(self):
        self.process = None
        self._start()

    def _start(self):
        cmd = [
            str(RUST_BIN),
            "sandbox-exec",
            "--interactive",
            "--bytecode-dir", str(BYTECODE_DIR),
        ]
        self.process = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def execute(self, action: str, **kwargs) -> dict:
        request = {"action": action, **kwargs}
        req_json = json.dumps(request) + "\n"
        self.process.stdin.write(req_json)
        self.process.stdin.flush()
        resp = self.process.stdout.readline()
        if not resp:
            return {"error": "No response"}
        return json.loads(resp)

    def close(self):
        if self.process:
            self.process.terminate()
            self.process.wait(timeout=5)


def execute_ptb_plan(sandbox: SandboxProcess, plan: dict) -> dict:
    """Convert LLM's PTB plan to sandbox format and execute it."""
    calls = plan.get("calls", [])
    if not calls:
        return {"success": False, "error": "No calls in PTB plan"}

    inputs = []
    commands = []
    input_idx = 0

    for i, call in enumerate(calls):
        target = call.get("target", "")
        type_args = call.get("type_args", [])
        args = call.get("args", [])

        # Parse target: "0x...::module::function"
        parts = target.split("::")
        if len(parts) < 3:
            return {"success": False, "error": f"Invalid target format: {target}"}
        package = parts[0]
        module = parts[1]
        function = "::".join(parts[2:])  # Handle nested modules

        # Convert args to PTB format
        # PtbArg is untagged: Input(usize) or Result { cmd, idx }
        ptb_args = []
        for arg in args:
            if arg == "TxContext":
                # TxContext is handled implicitly, skip
                continue
            elif isinstance(arg, str) and arg.startswith("$"):
                # Reference to previous result: "$0", "$1", etc.
                ref_idx = int(arg[1:])
                ptb_args.append({"cmd": ref_idx, "idx": 0})  # Result { cmd, idx }
            elif isinstance(arg, str) and arg.startswith("Result("):
                # Alternative format: "Result(2)"
                ref_idx = int(arg[7:-1])  # Extract number from "Result(X)"
                ptb_args.append({"cmd": ref_idx, "idx": 0})  # Result { cmd, idx }
            elif isinstance(arg, int):
                # Pure value - add to inputs, reference by index
                inputs.append({"type": "pure", "value": arg, "value_type": "u64"})
                ptb_args.append(input_idx)  # Input(usize) is just a number
                input_idx += 1
            elif isinstance(arg, bool):
                inputs.append({"type": "pure", "value": arg, "value_type": "bool"})
                ptb_args.append(input_idx)  # Input(usize)
                input_idx += 1
            elif isinstance(arg, str):
                # Could be an address or string
                if arg.startswith("0x"):
                    inputs.append({"type": "pure", "value": arg, "value_type": "address"})
                else:
                    inputs.append({"type": "pure", "value": arg, "value_type": "string"})
                ptb_args.append(input_idx)  # Input(usize)
                input_idx += 1

        commands.append({
            "type": "move_call",
            "package": package,
            "module": module,
            "function": function,
            "type_args": type_args,
            "args": ptb_args,
        })

    # Execute the PTB
    return sandbox.execute("execute_ptb", inputs=inputs, commands=commands)


def get_config():
    """Get API configuration from environment."""
    api_key = os.environ.get("OPENROUTER_API_KEY") or os.environ.get("SMI_API_KEY")
    if not api_key:
        print("ERROR: Set OPENROUTER_API_KEY or SMI_API_KEY")
        sys.exit(1)

    model = os.environ.get("SMI_MODEL", "openai/gpt-5.2")
    base_url = os.environ.get("SMI_API_BASE_URL", "https://openrouter.ai/api/v1")

    return {
        "api_key": api_key,
        "model": model,
        "base_url": base_url,
    }


def call_llm(cfg: dict, messages: list) -> str:
    """Call the LLM and return content."""
    url = f"{cfg['base_url']}/chat/completions"
    headers = {
        "Authorization": f"Bearer {cfg['api_key']}",
        "HTTP-Referer": "https://github.com/MystenLabs/sui-sandbox",
        "X-Title": "Sui Sandbox Agent Test",
    }
    payload = {
        "model": cfg["model"],
        "temperature": 0,
        "messages": messages,
        "max_tokens": 4096,
        "response_format": {"type": "json_object"},
    }

    with httpx.Client(timeout=180) as client:
        r = client.post(url, headers=headers, json=payload)
        r.raise_for_status()
        data = r.json()
        return data["choices"][0]["message"]["content"]


def parse_tool_call(content: str) -> tuple[str, dict] | None:
    """Parse a tool call from LLM response."""
    content = content.strip()

    # Try direct JSON parse first
    try:
        data = json.loads(content)
        if "tool" in data:
            return data["tool"], data.get("args", {})
        if "calls" in data:
            # Final submission
            return "submit_ptb_plan", data
    except json.JSONDecodeError:
        pass

    # If LLM concatenated multiple JSON objects, take the first one
    # Look for pattern: {...}{...}
    if content.startswith("{"):
        brace_count = 0
        end_idx = 0
        for i, c in enumerate(content):
            if c == "{":
                brace_count += 1
            elif c == "}":
                brace_count -= 1
                if brace_count == 0:
                    end_idx = i + 1
                    break
        if end_idx > 0:
            try:
                data = json.loads(content[:end_idx])
                if "tool" in data:
                    return data["tool"], data.get("args", {})
            except json.JSONDecodeError:
                pass

    return None


def main():
    print("=" * 60)
    print("Sandbox Agent Test with Full Tool Access")
    print("=" * 60)

    cfg = get_config()
    print(f"Model: {cfg['model']}")
    print(f"Target types: {TARGET_TYPES}")
    print()

    # Start sandbox
    print("Starting sandbox...")
    sandbox = SandboxProcess()

    # Get initial module list for context
    modules_result = sandbox.execute("list_modules")
    pkg_modules = [m for m in modules_result.get("data", {}).get("modules", [])
                   if "0xc35ee7fee" in m]
    print(f"Package modules: {pkg_modules}")
    print()

    # Build system prompt
    system_prompt = f"""You are an expert Sui Move developer. Your task is to create a PTB
(Programmable Transaction Block) that constructs instances of specific target types.

Target types to create: {TARGET_TYPES}

Package modules available:
{json.dumps(pkg_modules, indent=2)}

{TOOL_DESCRIPTIONS}

## IMPORTANT INSTRUCTIONS
1. Use tools ONE AT A TIME to explore the package and find constructors
2. ALWAYS respond with EXACTLY ONE JSON object containing "tool" and "args" keys
3. Only call ONE tool per response - wait for the result before calling the next
4. When you have gathered enough information, submit your PTB plan using submit_ptb_plan
5. Your response MUST be ONLY valid JSON like: {{"tool": "tool_name", "args": {{...}}}}
6. DO NOT concatenate multiple JSON objects - only ONE tool call per response

For submit_ptb_plan, each call needs:
- "target": full function path like "0x...::module::function"
- "type_args": array of type arguments (can be empty [])
- "args": array of argument values (use "TxContext" for context, integers as numbers)

NOTE: In this sandbox simulation, you CAN call functions with "friend" visibility - they are callable.
Reference previous results with "$0", "$1", etc. (index of the call that produced the value).

Start by finding constructors for the target types."""

    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": f"Find constructors for: {TARGET_TYPES}. Respond with JSON."},
    ]

    tool_calls_log = []

    for iteration in range(MAX_ITERATIONS):
        print(f"\n--- Iteration {iteration + 1}/{MAX_ITERATIONS} ---")

        # Call LLM
        print("Calling LLM...")
        try:
            content = call_llm(cfg, messages)
        except Exception as e:
            print(f"LLM error: {e}")
            break

        print(f"Response: {content[:300]}...")
        messages.append({"role": "assistant", "content": content})

        # Parse tool call
        parsed = parse_tool_call(content)
        if not parsed:
            print("Could not parse tool call from response")
            break

        tool_name, tool_args = parsed
        print(f"Tool: {tool_name}")
        print(f"Args: {json.dumps(tool_args)[:200]}")
        tool_calls_log.append({"name": tool_name, "args": tool_args})

        # Check for final submission
        if tool_name == "submit_ptb_plan":
            print("\n" + "=" * 60)
            print("FINAL PTB PLAN SUBMITTED")
            print("=" * 60)
            print(json.dumps(tool_args, indent=2))

            # Convert PTB plan to sandbox format and execute
            print("\n" + "=" * 60)
            print("EXECUTING PTB")
            print("=" * 60)

            try:
                ptb_result = execute_ptb_plan(sandbox, tool_args)
                print(f"Execution result: {json.dumps(ptb_result, indent=2)[:2000]}")

                # Check for hits
                if ptb_result.get("success"):
                    effects = ptb_result.get("effects", {})
                    return_values = effects.get("return_values", [])

                    # Count successful calls - each return value indicates a successful function call
                    successful_calls = len(return_values)
                    print(f"\nSuccessful function calls: {successful_calls}")

                    # The plan includes reasoning about what types are created
                    # Since execution succeeded, we trust the LLM's plan
                    plan_calls = tool_args.get("calls", [])

                    # Map call index to target type (heuristic based on function names)
                    produced = []
                    for call in plan_calls:
                        target = call.get("target", "")
                        type_args = call.get("type_args", [])
                        # Extract function name and infer return type
                        if "::cell::new" in target:
                            if type_args:
                                produced.append(f"Cell<{type_args[0]}>")
                            else:
                                produced.append("Cell")
                        elif "::to_fee_config" in target:
                            produced.append("FeeConfig")
                        elif "::fees::new_builder" in target:
                            produced.append("FeeConfigBuilder")
                        elif "::storage::new" in target:
                            produced.append("Storage")
                        elif "::version::new" in target:
                            produced.append("Version")
                        elif "::balance::zero" in target:
                            produced.append("Balance")
                        elif "::coin::zero" in target or "::coin::from_balance" in target:
                            produced.append("Coin")
                        elif "::option::none" in target or "::option::some" in target:
                            produced.append("Option")

                    print(f"Types produced by plan: {produced}")
                    print(f"Target types: {TARGET_TYPES}")

                    # Check hits
                    hits = []
                    for target in TARGET_TYPES:
                        for p in produced:
                            if target in p or p in target:
                                hits.append(target)
                                break

                    print(f"\n{'='*60}")
                    print(f"HITS: {len(hits)}/{len(TARGET_TYPES)} = {len(hits)/len(TARGET_TYPES)*100:.0f}%")
                    print(f"Hit types: {hits}")
                    print(f"{'='*60}")
                else:
                    print(f"PTB execution failed: {ptb_result.get('error')}")
            except Exception as e:
                print(f"Error executing PTB: {e}")
                import traceback
                traceback.print_exc()

            sandbox.close()

            # Summary
            print(f"\nTotal iterations: {iteration + 1}")
            print(f"Total tool calls: {len(tool_calls_log)}")
            return

        # Execute tool
        result = sandbox.execute(tool_name, **tool_args)
        result_json = json.dumps(result)
        if len(result_json) > 1000:
            result_json = result_json[:1000] + "..."
        print(f"Result: {result_json}")

        # Add result to conversation
        messages.append({
            "role": "user",
            "content": f"Tool result:\n{json.dumps(result)}\n\nRespond with ONLY a JSON object containing \"tool\" and \"args\" keys. Either call another tool or submit your PTB plan with {{\"tool\": \"submit_ptb_plan\", \"args\": {{\"calls\": [...], \"reasoning\": \"...\"}}}}"
        })

    print("\n" + "=" * 60)
    print("Max iterations reached")
    print(f"Total tool calls: {len(tool_calls_log)}")
    for i, tc in enumerate(tool_calls_log):
        print(f"  {i+1}. {tc['name']}")
    print("=" * 60)

    sandbox.close()


if __name__ == "__main__":
    main()
