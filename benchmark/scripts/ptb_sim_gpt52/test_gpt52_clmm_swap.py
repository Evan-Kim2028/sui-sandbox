#!/usr/bin/env python3
"""
Test GPT-5.2 on CLMM Multi-Swap transaction with batched tool calls.

Usage:
    OPENROUTER_API_KEY=... python benchmark/scripts/ptb_sim_gpt52/test_gpt52_clmm_swap.py
"""

import json
import os
import re
import subprocess
import sys
import base64
import tempfile
import time
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
MAX_ITERATIONS = 5  # Limit iterations to control API costs
MAX_TIME_SECONDS = 120  # 2 minute time limit
MAX_TOKENS_RESPONSE = 4096  # Allow full responses
MAX_LIST_ITEMS = 10  # Truncate list results to this many items (conservative for token efficiency)
MAX_RESULT_CHARS = 4000  # Truncate total result JSON to this many characters
COMPACT_JSON = True  # Use compact JSON (no indentation) to reduce tokens
RUST_BIN = Path("target/release/sui_move_interface_extractor")

# Simple artipedia::update_points transaction (1 command, 2 inputs, 2 packages)
TX_DIGEST = "AHKS3JQtTJC6Bwt7uE6v9z8kho2oQVHxCKvdsezJ9rHi"

TOOL_DESCRIPTIONS = """
Available tools. You may call MULTIPLE tools in a single response by returning a JSON array.

Return either:
- A single tool call: {"tool": "...", "args": {...}}
- Multiple tool calls: [{"tool": "...", "args": {...}}, {"tool": "...", "args": {...}}, ...]

Note: List results are truncated. Use search_functions with patterns to find specific functions.

## Introspection Tools
1. list_modules - List all loaded Move modules
   Args: none

2. list_cached_objects - List pre-loaded objects for PTB execution
   Args: none
   Returns: Object IDs, types, shared status

3. list_functions - List functions in a module
   Args: {"module_path": "0x...::module_name"}

4. list_structs - List struct types in a module
   Args: {"module_path": "0x...::module_name"}

5. get_function_info - Get function signature details
   Args: {"module_path": "...", "function_name": "..."}

6. get_functions_batch - Get multiple function signatures in one call (efficient)
   Args: {"functions": [{"module_path": "...", "function_name": "..."}, ...]}

7. get_struct_info - Get struct type definition
   Args: {"type_path": "0x...::module::TypeName"}

8. find_constructors - Find functions that return a given type
   Args: {"type_path": "TypeName"}

9. search_functions - Search for functions matching a pattern (use this to find specific functions)
   Args: {"pattern": "*swap*", "entry_only": false}

10. disassemble_function - Get bytecode disassembly
    Args: {"module_path": "...", "function_name": "..."}

## Compilation Tools
11. compile_move - Compile Move source code and deploy to sandbox
    Args: {"package_name": "my_pkg", "module_name": "my_module", "source": "module my_pkg::my_module { ... }"}

## Execution Tools
12. execute_ptb - Execute a programmable transaction block
    Args: {
      "inputs": [{"type": "pure", "value": ..., "value_type": "u64"}, {"type": "object", "object_id": "0x..."}, ...],
      "commands": [{"type": "move_call", "package": "0x...", "module": "...", "function": "...", "type_args": [], "args": [0, 1]}]
    }
    Note: args are integer indices into the inputs array, or {"cmd": N, "idx": M} for command results

13. submit_solution - Submit when you've successfully simulated the PTB
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
        "HTTP-Referer": "https://github.com/MystenLabs/sui-move-interface-extractor",
        "X-Title": "Sui Move CLMM Swap Test",
    }
    payload = {
        "model": MODEL,
        "temperature": temperature,
        "messages": messages,
        "max_tokens": MAX_TOKENS_RESPONSE,
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


def extract_relevant_modules(data: dict) -> set[str]:
    """Extract module paths that are actually used in the transaction."""
    tx = data.get("transaction", {})
    commands = tx.get("commands", [])
    modules = set()

    for cmd in commands:
        if cmd.get("type") == "MoveCall":
            package = cmd.get("package", "")
            module = cmd.get("module", "")
            if package and module:
                # Extract just the module name part after ::
                if "::" in module:
                    modules.add(module)  # Already in package::module format
                else:
                    modules.add(f"{package}::{module}")

    return modules


def extract_transaction_summary(data: dict) -> str:
    """Extract a rich summary of the transaction for the prompt.

    Includes input objects with types, command details, and data flow hints.
    """
    tx = data.get("transaction", {})
    commands = tx.get("commands", [])
    inputs = tx.get("inputs", [])
    object_types = data.get("object_types", {})

    summary_lines = [
        f"Transaction: {tx.get('digest', 'unknown')}",
        f"Commands: {len(commands)}, Inputs: {len(inputs)}",
        "",
        "## Inputs (use these in PTB):",
    ]

    def shorten_type(type_str: str) -> str:
        """Shorten a type string for display while preserving generics."""
        if "::" not in type_str:
            return type_str
        # Handle generic types like Pool<USDC, MAGMA>
        if "<" in type_str:
            base, generics = type_str.split("<", 1)
            short_base = base.split("::")[-1]
            # Shorten each generic arg
            generic_parts = generics.rstrip(">").split(", ")
            short_generics = [p.split("::")[-1] for p in generic_parts]
            return f"{short_base}<{', '.join(short_generics)}>"
        return type_str.split("::")[-1]

    # Document each input with its type
    for i, inp in enumerate(inputs):
        inp_type = inp.get("type", "Unknown")
        if inp_type == "SharedObject":
            obj_id = inp.get("object_id", "")
            type_str = object_types.get(obj_id, "unknown type")
            short_type = shorten_type(type_str)
            mutable = "mutable" if inp.get("mutable") else "immutable"
            summary_lines.append(f"  [{i}] SharedObject: {short_type} ({mutable})")
            summary_lines.append(f"      id: {obj_id}")
        elif inp_type == "Object":
            obj_id = inp.get("object_id", "")
            type_str = object_types.get(obj_id, "owned object")
            short_type = shorten_type(type_str)
            summary_lines.append(f"  [{i}] OwnedObject: {short_type}")
            summary_lines.append(f"      id: {obj_id}")
        elif inp_type == "Pure":
            summary_lines.append(f"  [{i}] Pure: (value bytes)")
        else:
            summary_lines.append(f"  [{i}] {inp_type}")

    # Summarize unique functions called with FULL module paths
    summary_lines.append("")
    summary_lines.append("## Functions called (use these exact module paths):")
    seen_funcs = set()
    for cmd in commands:
        if cmd.get("type") == "MoveCall":
            pkg = cmd.get("package", "")
            mod = cmd.get("module", "")
            func = cmd.get("function", "")
            type_args = cmd.get("type_arguments", [])

            full_module_path = f"{pkg}::{mod}"
            func_key = f"{full_module_path}::{func}"
            if func_key not in seen_funcs:
                seen_funcs.add(func_key)
                if type_args:
                    # Shorten type args
                    short_args = [t.split("::")[-1] for t in type_args]
                    summary_lines.append(f"  - {full_module_path}::{func}<{', '.join(short_args)}>")
                else:
                    summary_lines.append(f"  - {full_module_path}::{func}")

    # Command sequence (compact)
    summary_lines.append("")
    summary_lines.append("## Command sequence:")
    for i, cmd in enumerate(commands):
        cmd_type = cmd.get("type", "Unknown")
        if cmd_type == "MoveCall":
            func = cmd.get("function", "")
            summary_lines.append(f"  {i}. MoveCall({func})")
        elif cmd_type == "SplitCoins":
            summary_lines.append(f"  {i}. SplitCoins")
        elif cmd_type == "MergeCoins":
            summary_lines.append(f"  {i}. MergeCoins")
        elif cmd_type == "TransferObjects":
            summary_lines.append(f"  {i}. TransferObjects")
        else:
            summary_lines.append(f"  {i}. {cmd_type}")

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

    # Try to parse as a single JSON array or object first
    try:
        parsed = json.loads(response)
        if isinstance(parsed, dict):
            return [parsed]
        elif isinstance(parsed, list):
            return parsed
    except json.JSONDecodeError:
        pass

    # Handle concatenated JSON objects like {"tool":...}{"tool":...}
    # Use a proper bracket-matching approach
    results = []
    i = 0
    while i < len(response):
        if response[i] == "{":
            # Find the matching closing brace
            depth = 0
            start = i
            in_string = False
            escape_next = False
            while i < len(response):
                char = response[i]
                if escape_next:
                    escape_next = False
                elif char == "\\":
                    escape_next = True
                elif char == '"' and not escape_next:
                    in_string = not in_string
                elif not in_string:
                    if char == "{":
                        depth += 1
                    elif char == "}":
                        depth -= 1
                        if depth == 0:
                            # Found complete object
                            try:
                                obj = json.loads(response[start : i + 1])
                                if isinstance(obj, dict) and "tool" in obj:
                                    results.append(obj)
                            except json.JSONDecodeError:
                                pass
                            break
                i += 1
        elif response[i] == "[":
            # Try to parse as array
            depth = 0
            start = i
            in_string = False
            escape_next = False
            while i < len(response):
                char = response[i]
                if escape_next:
                    escape_next = False
                elif char == "\\":
                    escape_next = True
                elif char == '"' and not escape_next:
                    in_string = not in_string
                elif not in_string:
                    if char == "[":
                        depth += 1
                    elif char == "]":
                        depth -= 1
                        if depth == 0:
                            try:
                                arr = json.loads(response[start : i + 1])
                                if isinstance(arr, list):
                                    for item in arr:
                                        if isinstance(item, dict) and "tool" in item:
                                            results.append(item)
                            except json.JSONDecodeError:
                                pass
                            break
                i += 1
        i += 1

    return results


class SessionCache:
    """Track previously retrieved results to avoid redundant data in responses."""

    def __init__(self):
        self.seen_results: dict[str, int] = {}  # key -> iteration when first seen
        self.last_errors: dict[str, str] = {}  # tool -> last error message
        self.current_iteration = 0

    def make_cache_key(self, tool: str, args: dict) -> str:
        """Create a cache key from tool name and args."""
        args_str = json.dumps(args, sort_keys=True, separators=(",", ":"))
        return f"{tool}:{args_str}"

    def check_duplicate(self, tool: str, args: dict, result: dict) -> dict | None:
        """Check if this is a duplicate request. Returns shortened response if so."""
        key = self.make_cache_key(tool, args)

        if key in self.seen_results:
            prev_iter = self.seen_results[key]
            return {
                "success": True,
                "cached": True,
                "message": f"Same as iteration {prev_iter}. Use different args or another tool.",
            }

        # Not a duplicate, record it
        self.seen_results[key] = self.current_iteration
        return None

    def check_duplicate_error(self, tool: str, result: dict) -> dict | None:
        """Check if this error is same as last error for this tool."""
        if not result.get("success", True):
            error = result.get("error", "")
            if error and tool in self.last_errors and self.last_errors[tool] == error:
                return {
                    "success": False,
                    "error": "Same error as previous attempt. Try a different approach.",
                    "error_category": result.get("error_category", ""),
                }
            self.last_errors[tool] = error
        return None


def truncate_result_for_llm(
    tool: str,
    result: dict,
    max_items: int = MAX_LIST_ITEMS,
    relevant_modules: set[str] | None = None,
) -> dict:
    """Truncate large result arrays to reduce token usage. Returns modified copy.

    If relevant_modules is provided, prioritizes those modules in list_modules output.
    """
    if not result.get("success", True):
        return result  # Don't truncate errors

    result = result.copy()
    data = result.get("data", {})
    if not data:
        return result

    data = data.copy()
    tool_lower = tool.lower().replace("_", "")

    # Truncate list results with a note about remaining items
    list_fields = {
        "listmodules": "modules",
        "listfunctions": "functions",
        "liststructs": "structs",
        "listcachedobjects": "objects",
        "searchfunctions": "matches",
    }

    field = list_fields.get(tool_lower)
    if field and field in data:
        items = data[field]
        if isinstance(items, list) and len(items) > max_items:
            # For modules, prioritize relevant ones first
            if tool_lower == "listmodules" and relevant_modules:
                relevant = [m for m in items if any(r in m for r in relevant_modules)]
                other = [m for m in items if m not in relevant]
                items = relevant + other

            data[field] = items[:max_items]
            data["truncated"] = True
            data["shown"] = max_items
            data["total"] = len(items)
            # Add hint for how to get more
            if tool_lower == "listmodules":
                data["hint"] = "Use search_functions with pattern to find specific functions"
            elif tool_lower == "listfunctions":
                data["hint"] = "Use get_function_info for details on specific functions"

    result["data"] = data
    return result


def summarize_tool_result(tool: str, result: dict) -> str:
    """Generate a neutral, factual one-line summary of a tool result."""
    if not result.get("success", False) and "error" in result:
        error = result.get("error", "")
        category = result.get("error_category", "")
        if category:
            return f"[FAILED] {category}: {error[:100]}"
        return f"[FAILED] {error[:100]}"

    tool_lower = tool.lower().replace("_", "")

    if tool_lower == "executeptb":
        effects = result.get("effects", {})
        created = len(effects.get("created", []))
        mutated = len(effects.get("mutated", []))
        deleted = len(effects.get("deleted", []))
        returns = effects.get("return_values", [])
        return_count = sum(r.get("count", 0) for r in returns)
        gas = result.get("gas_used", 0)
        return f"[OK] created={created} mutated={mutated} deleted={deleted} returns={return_count} gas={gas}"

    elif tool_lower == "listmodules":
        modules = result.get("data", {}).get("modules", [])
        return f"[OK] {len(modules)} modules"

    elif tool_lower == "listfunctions":
        functions = result.get("data", {}).get("functions", [])
        return f"[OK] {len(functions)} functions"

    elif tool_lower == "liststructs":
        structs = result.get("data", {}).get("structs", [])
        return f"[OK] {len(structs)} structs"

    elif tool_lower == "listcachedobjects":
        objects = result.get("data", {}).get("objects", [])
        return f"[OK] {len(objects)} objects"

    elif tool_lower == "searchfunctions":
        matches = result.get("data", {}).get("matches", [])
        return f"[OK] {len(matches)} matches"

    elif tool_lower == "compilemove":
        if result.get("success"):
            module = result.get("data", {}).get("module_path", "")
            return f"[OK] compiled {module}"
        return "[OK]"

    elif tool_lower in ("getfunctioninfo", "getstructinfo"):
        path = result.get("data", {}).get("path", "")
        return f"[OK] {path[:60]}" if path else "[OK]"

    elif tool_lower == "getfunctionsbatch":
        functions = result.get("data", {}).get("functions", [])
        return f"[OK] {len(functions)} function signatures"

    return "[OK]" if result.get("success") else "[FAILED]"


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

    elif tool_lower in ("getfunctionsbatch", "get_functions_batch"):
        # Batch multiple function info requests into one call
        functions = args.get("functions", [])
        results = []
        for func in functions[:10]:  # Limit to 10 to prevent abuse
            module_path = func.get("module_path", "")
            function_name = func.get("function_name", "")
            result = sandbox.execute("get_function_info", module_path=module_path, function_name=function_name)
            if result.get("success"):
                results.append(result.get("data", {}))
            else:
                results.append({"error": result.get("error", "Unknown error"), "module_path": module_path, "function_name": function_name})
        return {"success": True, "data": {"functions": results, "count": len(results)}}

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


def analyze_failure(results: list[dict], submitted: bool, timed_out: bool) -> dict:
    """Analyze why the LLM failed to complete the task.

    Returns a dict with:
      - category: Primary failure reason
      - details: Specific details
      - suggestions: What might help
    """
    if submitted:
        # Check if submission was successful
        for r in results:
            if r.get("submitted"):
                if r.get("success"):
                    return {"category": "SUCCESS", "details": "Task completed successfully", "suggestions": []}
                else:
                    return {
                        "category": "WRONG_ANSWER",
                        "details": "Submitted but marked as unsuccessful",
                        "suggestions": ["Review PTB structure", "Check error messages"],
                    }

    if timed_out:
        return {
            "category": "TIMEOUT",
            "details": f"Exceeded time limit",
            "suggestions": ["Increase MAX_TIME_SECONDS", "Reduce exploration overhead"],
        }

    # Analyze tool call patterns
    total_tools = 0
    ptb_attempts = 0
    ptb_failures = 0
    compile_attempts = 0
    compile_failures = 0
    exploration_calls = 0
    cached_hits = 0
    last_error = None

    for r in results:
        if r.get("error"):
            last_error = r.get("error")
            continue

        tools_called = r.get("tools_called", 0)
        total_tools += tools_called

        for summary in r.get("summaries", []):
            if "[CACHED]" in summary:
                cached_hits += 1
            elif "execute_ptb" in summary:
                ptb_attempts += 1
                if "[FAILED]" in summary:
                    ptb_failures += 1
            elif "compile_move" in summary:
                compile_attempts += 1
                if "[FAILED]" in summary:
                    compile_failures += 1
            elif any(x in summary for x in ["list_modules", "list_functions", "search_functions", "get_function"]):
                exploration_calls += 1

    # Determine primary failure category
    if ptb_attempts == 0:
        if exploration_calls > total_tools * 0.7:
            return {
                "category": "STUCK_EXPLORING",
                "details": f"Spent {exploration_calls}/{total_tools} calls exploring, never attempted PTB",
                "suggestions": ["Give more upfront context", "Reduce MAX_ITERATIONS to force action"],
            }
        elif compile_failures > 0:
            return {
                "category": "COMPILATION_BLOCKED",
                "details": f"Failed to compile needed code ({compile_failures} failures)",
                "suggestions": ["Check if all dependencies are loaded", "Review compile errors"],
            }
        else:
            return {
                "category": "NO_PTB_ATTEMPT",
                "details": "Never attempted to execute a PTB",
                "suggestions": ["Improve transaction summary", "Add example PTB format"],
            }

    elif ptb_failures == ptb_attempts:
        return {
            "category": "PTB_FORMAT_ERROR",
            "details": f"All {ptb_attempts} PTB attempts failed",
            "suggestions": ["Review PTB format in docs", "Check input/argument indices"],
        }

    elif cached_hits > total_tools * 0.3:
        return {
            "category": "STUCK_IN_LOOP",
            "details": f"{cached_hits} duplicate requests detected",
            "suggestions": ["LLM is repeating itself", "May need clearer error messages"],
        }

    else:
        return {
            "category": "ITERATION_LIMIT",
            "details": f"Ran out of iterations ({ptb_attempts} PTB attempts, {ptb_failures} failed)",
            "suggestions": ["Increase MAX_ITERATIONS", "Improve error feedback"],
        }


def main():
    print("=" * 60)
    print("GPT-5.2 CLMM Swap Test with Batched Tool Calls")
    print("=" * 60)
    print(f"Model: {MODEL}")
    print(f"Max iterations: {MAX_ITERATIONS}")
    print(f"Time limit: {MAX_TIME_SECONDS}s")
    print()

    api_key = get_api_key()
    print("API key loaded")

    # Load transaction data
    print(f"\nLoading transaction: {TX_DIGEST}")
    data = load_transaction_context(TX_DIGEST)
    if not data:
        print("ERROR: Transaction not found in cache")
        sys.exit(1)

    # Start sandbox and load packages
    sandbox = SandboxProcess()
    sandbox.start()

    packages = data.get("packages", {})
    module_count = sandbox.load_packages(packages)
    print(f"Loaded {module_count} modules into sandbox")

    # Load cached objects with proper shared status and type info
    object_count = sandbox.load_cached_objects(data)
    print(f"Loaded {object_count} objects into sandbox")

    # Extract modules used in transaction for prioritization
    relevant_modules = extract_relevant_modules(data)
    print(f"Relevant modules: {len(relevant_modules)}")

    tx_summary = extract_transaction_summary(data)
    print(f"\n{tx_summary}")

    # Build initial system prompt
    system_prompt = f"""You are simulating a Sui PTB in a sandbox environment.

{TOOL_DESCRIPTIONS}

## Transaction to Simulate
{tx_summary}

Your goal: Successfully simulate this PTB by understanding the interfaces, compiling any needed modules, and executing the transaction. Call submit_solution when done."""

    messages = [{"role": "system", "content": system_prompt}]
    messages.append({"role": "user", "content": "Get the function signature for the function(s) listed above, then execute the PTB. You have limited iterations - focus on action over exploration."})

    results = []
    submitted = False
    timed_out = False
    start_time = time.time()
    session_cache = SessionCache()

    for iteration in range(MAX_ITERATIONS):
        session_cache.current_iteration = iteration + 1

        # Check time limit
        elapsed = time.time() - start_time
        remaining_time = MAX_TIME_SECONDS - elapsed
        if remaining_time <= 0:
            print(f"\n*** TIME LIMIT REACHED ({MAX_TIME_SECONDS}s) ***")
            timed_out = True
            break

        remaining = MAX_ITERATIONS - iteration
        print(f"\n{'='*60}")
        print(f"ITERATION {iteration + 1}/{MAX_ITERATIONS} ({remaining} remaining, {remaining_time:.0f}s left)")
        print("=" * 60)

        try:
            response = call_llm(api_key, messages)
            print(f"\nResponse: {response[:300]}...")

            tool_calls = parse_tool_calls(response)
            print(f"\nParsed {len(tool_calls)} tool call(s)")

            if not tool_calls:
                print("No valid tool calls found in response")
                messages.append({"role": "assistant", "content": response})
                messages.append({"role": "user", "content": "Please respond with valid JSON tool calls."})
                continue

            # Execute all tool calls (batched)
            tool_results = []
            tool_summaries = []
            for tc in tool_calls:
                tool = tc.get("tool", "")
                args = tc.get("args", {})

                result = execute_tool(sandbox, tool, args)

                # Check for duplicate request (returns cached short response)
                cached = session_cache.check_duplicate(tool, args, result)
                if cached:
                    result_for_llm = cached
                    summary = "[CACHED] Already retrieved"
                else:
                    # Check for duplicate error
                    dup_error = session_cache.check_duplicate_error(tool, result)
                    if dup_error:
                        result_for_llm = dup_error
                        summary = "[DUP ERROR] Same as previous"
                    else:
                        # Truncate large results to reduce token usage, prioritize relevant modules
                        result_for_llm = truncate_result_for_llm(tool, result, relevant_modules=relevant_modules)
                        summary = summarize_tool_result(tool, result)

                tool_results.append({"tool": tool, "result": result_for_llm})
                tool_summaries.append(f"  {tool}: {summary}")
                print(f"  {tool}: {summary}")

                # Check for submission
                if tool.lower().replace("_", "") == "submitsolution":
                    submitted = True
                    print(f"\n*** SUBMITTED: success={result.get('success')} ***")
                    print(f"Summary: {result.get('summary', 'N/A')}")
                    results.append({
                        "iteration": iteration + 1,
                        "submitted": True,
                        "success": result.get("success", False),
                        "summary": result.get("summary", ""),
                    })

            if submitted:
                break

            # Build response message with all results (compact JSON to save tokens)
            if COMPACT_JSON:
                results_str = json.dumps(tool_results, separators=(",", ":"), default=str)
            else:
                results_str = json.dumps(tool_results, indent=2, default=str)
            if len(results_str) > MAX_RESULT_CHARS:
                results_str = results_str[:MAX_RESULT_CHARS] + "\n... (truncated)"

            messages.append({"role": "assistant", "content": response})
            messages.append({"role": "user", "content": f"Tool results:\n{results_str}\n\nContinue."})

            results.append({
                "iteration": iteration + 1,
                "tools_called": len(tool_calls),
                "tool_names": [tc.get("tool") for tc in tool_calls],
                "summaries": tool_summaries,
            })

        except Exception as e:
            print(f"\nError: {e}")
            results.append({"iteration": iteration + 1, "error": str(e)})

    sandbox.close()
    total_time = time.time() - start_time

    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    if submitted:
        status = "SUBMITTED"
    elif timed_out:
        status = "TIMED_OUT"
    else:
        status = "ITERATION_LIMIT"
    print(f"Result: {status}")
    print(f"Iterations: {len(results)}/{MAX_ITERATIONS}")
    print(f"Time: {total_time:.1f}s/{MAX_TIME_SECONDS}s")

    # Count successes and failures
    total_tools = 0
    ptb_successes = 0
    ptb_failures = 0
    compile_successes = 0
    compile_failures = 0

    for r in results:
        if r.get("tools_called"):
            total_tools += r["tools_called"]
            for summary in r.get("summaries", []):
                if "execute_ptb" in summary:
                    if "[OK]" in summary:
                        ptb_successes += 1
                    elif "[FAILED]" in summary:
                        ptb_failures += 1
                elif "compile_move" in summary:
                    if "[OK]" in summary:
                        compile_successes += 1
                    elif "[FAILED]" in summary:
                        compile_failures += 1

    print(f"Total tool calls: {total_tools}")
    print(f"PTB executions: {ptb_successes} succeeded, {ptb_failures} failed")
    print(f"Compilations: {compile_successes} succeeded, {compile_failures} failed")
    print()

    # Per-iteration breakdown
    for r in results:
        if r.get("tools_called"):
            print(f"Iter {r['iteration']}: {r['tools_called']} tools")
            for summary in r.get("summaries", []):
                print(f"    {summary.strip()}")
        elif r.get("submitted"):
            print(f"Iter {r['iteration']}: SUBMITTED (success={r.get('success')})")
        elif r.get("error"):
            print(f"Iter {r['iteration']}: ERROR - {r['error']}")

    # Failure analysis
    print()
    analysis = analyze_failure(results, submitted, timed_out)
    print(f"## Analysis: {analysis['category']}")
    print(f"   {analysis['details']}")
    if analysis.get("suggestions"):
        print("   Suggestions:")
        for s in analysis["suggestions"]:
            print(f"     - {s}")

    print("=" * 60)


if __name__ == "__main__":
    main()
