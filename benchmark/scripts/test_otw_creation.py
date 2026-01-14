#!/usr/bin/env python3
"""
Test the LLM's ability to create a One-Time Witness (OTW) module from scratch
to construct types that require witness patterns like TreasuryCap.

This is the hardest test case - the LLM must:
1. Understand that TreasuryCap<T> requires T to be an OTW
2. Write a Move module with a proper OTW type
3. Compile and deploy the module
4. Call coin::create_currency with the witness
5. Execute the PTB and verify success

Usage:
    python benchmark/scripts/test_otw_creation.py
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import httpx

# Configuration
MAX_ITERATIONS = 25  # More iterations for this complex task
PACKAGE_DIR = Path("benchmark/corpus/0x05/0x83556891f4a0f233ce7b05cfe7f957d4020492a34f5405b2cb9377d060bef4bf")
BYTECODE_DIR = PACKAGE_DIR / "bytecode_modules"
RUST_BIN = Path("target/release/sui_move_interface_extractor")

# Target: create a TreasuryCap - this requires creating an OTW module!
TARGET_TYPES = [
    "TreasuryCap",  # Requires OTW pattern
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

## Object Tools
8. create_object - Create an object with specific field values
   Args: {"object_type": "0x2::coin::Coin<0x2::sui::SUI>", "fields": {"balance": 1000000}}

9. list_objects - List all objects in the sandbox
   Args: none

10. inspect_object - Get object details by ID
    Args: {"object_id": "0x..."}

11. register_coin - Register a custom coin type
    Args: {"coin_type": "0xabc::my_coin::MY_COIN", "decimals": 9, "symbol": "MYCOIN", "name": "My Coin"}

## Compilation Tools
12. compile_move - Compile Move source code and deploy to sandbox
    Args: {"package_name": "my_pkg", "module_name": "my_module", "source": "module my_pkg::my_module { ... }"}
    Returns: {"compiled": true, "deployed": true, "package_id": "0x..."}

    IMPORTANT: A One-Time Witness (OTW) type must:
    - Have the same name as the module (uppercase)
    - Have only `drop` ability
    - Have no fields (be a unit struct)
    Example: module foo::bar { public struct BAR has drop {} }

## PTB Execution
13. execute_ptb - Execute a PTB (Programmable Transaction Block) in the sandbox
    Args: {
        "inputs": [
            {"type": "pure", "value": 9, "value_type": "u8"},
            {"type": "witness", "witness_type": "0xabc::my_coin::MY_COIN"},
            {"type": "object", "object_id": "0x6", "mode": "shared"},
            {"type": "gas", "budget": 50000000}
        ],
        "commands": [
            {
                "type": "move_call",
                "package": "0x2",
                "module": "coin",
                "function": "create_currency",
                "type_args": ["0xabc::my_coin::MY_COIN"],
                "args": [0, 1, 2, 3, 4, 5, 6]  // Input indices
            }
        ]
    }

    Input types:
    - "pure": Pure BCS value with type hint (u8, u64, bool, address, vector<u8>)
    - "witness": One-Time Witness - synthesized for OTW patterns
    - "object": Object reference by ID
    - "gas": Gas coin with budget

    Arg references use input indices (0, 1, 2...) or nested results {"cmd": 0, "idx": 0}

    Returns: {"success": true, "effects": {...}, "events": [...], "gas_used": ...}

## Time Control
14. set_time - Set sandbox clock timestamp
    Args: {"timestamp_ms": 1704067200000}

15. get_time - Get current sandbox clock timestamp
    Args: none

CRITICAL: To create a TreasuryCap<T>, you need:
1. First compile a module with an OTW type (e.g., "MY_COIN" with only `drop` ability)
2. Use execute_ptb with a "witness" input type for the OTW
3. Call coin::create_currency<MY_COIN>(witness, decimals, symbol, name, description, icon_url, ctx)
4. The witness input synthesizes the OTW value automatically

Example PTB for TreasuryCap creation:
{
    "inputs": [
        {"type": "witness", "witness_type": "0xabc::my_coin::MY_COIN"},
        {"type": "pure", "value": 9, "value_type": "u8"},           // decimals
        {"type": "pure", "value": "4d59434f494e", "value_type": "vector<u8>"},  // symbol (hex for MYCOIN)
        {"type": "pure", "value": "4d7920436f696e", "value_type": "vector<u8>"},  // name (hex for My Coin)
        {"type": "pure", "value": "54657374", "value_type": "vector<u8>"},  // description
        {"type": "pure", "value": "", "value_type": "vector<u8>"},  // icon_url (empty = None)
        {"type": "object", "object_id": "0x6", "mode": "shared"}    // TxContext (Clock proxy)
    ],
    "commands": [
        {
            "type": "move_call",
            "package": "0x2",
            "module": "coin",
            "function": "create_currency",
            "type_args": ["0xabc::my_coin::MY_COIN"],
            "args": [0, 1, 2, 3, 4, 5, 6]
        }
    ]
}
"""


class SandboxProcess:
    """Wrapper for the sandbox subprocess."""

    def __init__(self, bytecode_dir: Path):
        self.bytecode_dir = bytecode_dir
        self.process = None
        self.start()

    def start(self):
        self.process = subprocess.Popen(
            [
                str(RUST_BIN),
                "sandbox-exec",
                "--interactive",
                "--bytecode-dir", str(self.bytecode_dir),
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )

    def execute(self, action: str, **kwargs) -> dict:
        request = {"action": action, **kwargs}
        request_json = json.dumps(request)

        self.process.stdin.write(request_json + "\n")
        self.process.stdin.flush()

        response_line = self.process.stdout.readline()
        if not response_line:
            return {"success": False, "error": "No response from sandbox"}

        try:
            return json.loads(response_line)
        except json.JSONDecodeError as e:
            return {"success": False, "error": f"Invalid JSON response: {e}"}

    def close(self):
        if self.process:
            self.process.terminate()
            self.process.wait(timeout=5)


def parse_tool_call(content: str) -> tuple[str, dict] | None:
    """Parse a tool call from LLM response."""
    content = content.strip()

    # Try direct JSON parse first
    try:
        data = json.loads(content)
        if "tool" in data:
            return data["tool"], data.get("args", {})
    except json.JSONDecodeError:
        pass

    # If LLM concatenated multiple JSON objects, take the first one
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


def get_config():
    """Get API configuration from environment."""
    api_key = os.environ.get("OPENROUTER_API_KEY")
    if not api_key:
        raise ValueError("OPENROUTER_API_KEY environment variable not set")

    return {
        "api_key": api_key,
        "model": os.environ.get("SMI_MODEL", "openai/gpt-4.1"),
        "base_url": "https://openrouter.ai/api/v1",
    }


def call_llm(messages: list[dict], config: dict) -> str:
    """Call the LLM API."""
    headers = {
        "Authorization": f"Bearer {config['api_key']}",
        "Content-Type": "application/json",
    }

    data = {
        "model": config["model"],
        "messages": messages,
        "temperature": 0.2,
        "response_format": {"type": "json_object"},
    }

    with httpx.Client(timeout=120.0) as client:
        response = client.post(
            f"{config['base_url']}/chat/completions",
            headers=headers,
            json=data,
        )
        response.raise_for_status()
        result = response.json()
        return result["choices"][0]["message"]["content"]


def main():
    print("=" * 60)
    print("OTW Creation Test - TreasuryCap Construction")
    print("=" * 60)

    config = get_config()
    print(f"Model: {config['model']}")
    print(f"Target types: {TARGET_TYPES}")
    print()

    # Start sandbox
    print("Starting sandbox...")
    sandbox = SandboxProcess(BYTECODE_DIR)

    # Get initial module list
    modules_result = sandbox.execute("list_modules")
    if not modules_result.get("success"):
        print(f"Failed to list modules: {modules_result}")
        return

    # Filter to package modules
    all_modules = modules_result.get("data", {}).get("modules", [])
    pkg_modules = [m for m in all_modules if "0xc35ee7fee" in m]
    print(f"Package modules: {pkg_modules}")
    print()

    # Build system prompt
    system_prompt = f"""You are an expert Sui Move developer. Your task is to create a PTB
(Programmable Transaction Block) that constructs instances of specific target types.

Target types to create: {TARGET_TYPES}

IMPORTANT: TreasuryCap<T> requires a One-Time Witness (OTW) type. To create one:
1. Use compile_move to create a new module with an OTW type
2. The OTW must: have the same name as the module (UPPERCASE), have only `drop` ability, have no fields
3. Example module source:
   module helper::my_coin {{
       public struct MY_COIN has drop {{}}
   }}
4. After compiling, use execute_ptb with a "witness" input type to create the TreasuryCap

Package modules available:
{json.dumps(pkg_modules, indent=2)}

{TOOL_DESCRIPTIONS}

## IMPORTANT INSTRUCTIONS
1. Use tools ONE AT A TIME to explore the package and find constructors
2. ALWAYS respond with EXACTLY ONE JSON object containing "tool" and "args" keys
3. Only call ONE tool per response - wait for the result before calling the next
4. If TreasuryCap requires a witness type, use compile_move to create a helper module with an OTW
5. Your response MUST be ONLY valid JSON like: {{"tool": "tool_name", "args": {{...}}}}
6. After compiling the OTW module, use execute_ptb to run the PTB and create the TreasuryCap
7. The goal is to execute a PTB that creates a TreasuryCap successfully

Start by finding constructors for TreasuryCap to understand what's needed."""

    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": f"Find constructors for: {TARGET_TYPES}. Respond with JSON."},
    ]

    tool_calls_log = []
    ptb_executed = False
    ptb_success = False

    for iteration in range(MAX_ITERATIONS):
        print(f"\n--- Iteration {iteration + 1}/{MAX_ITERATIONS} ---")
        print("Calling LLM...")

        try:
            response = call_llm(messages, config)
        except Exception as e:
            print(f"LLM call failed: {e}")
            break

        print(f"Response: {response[:200]}...")

        # Parse tool call
        tool_call = parse_tool_call(response)
        if not tool_call:
            print(f"Failed to parse tool call from: {response[:500]}")
            messages.append({"role": "assistant", "content": response})
            messages.append({
                "role": "user",
                "content": "Invalid response. Please respond with ONLY a JSON object containing \"tool\" and \"args\" keys."
            })
            continue

        tool_name, tool_args = tool_call
        print(f"Tool: {tool_name}")
        print(f"Args: {json.dumps(tool_args)[:500]}")
        tool_calls_log.append((tool_name, tool_args))

        # Execute tool
        result = sandbox.execute(tool_name, **tool_args)

        # Check if this was execute_ptb
        if tool_name == "execute_ptb":
            ptb_executed = True
            ptb_success = result.get("success", False)

            print("\n" + "=" * 60)
            print("PTB EXECUTION RESULT")
            print("=" * 60)
            print(f"Success: {ptb_success}")

            if ptb_success:
                effects = result.get("effects", {})
                created = effects.get("created", [])
                return_values = effects.get("return_values", [])

                # Check for created objects
                print(f"Created objects: {len(created)}")
                for obj in created:
                    print(f"  - {obj.get('object_type', 'unknown')} ({obj.get('id', 'no-id')})")

                # Check for return values (TreasuryCap is returned, not stored)
                print(f"Return values: {len(return_values)} command(s) with returns")
                for rv in return_values:
                    print(f"  - Command {rv.get('command_index')}: {rv.get('count')} value(s)")

                # Success criteria: PTB executed successfully with return values from create_currency
                # (TreasuryCap is a return value, not a stored object)
                has_returns = any(rv.get("count", 0) >= 2 for rv in return_values)
                treasury_created = any("TreasuryCap" in str(obj.get("object_type", "")) for obj in created)

                if treasury_created:
                    print("\n✓ SUCCESS: TreasuryCap was created as a stored object!")
                elif has_returns:
                    print("\n✓ SUCCESS: create_currency returned TreasuryCap + CoinMetadata!")
                else:
                    print("\n? PTB succeeded but no TreasuryCap found")
            else:
                print(f"Error: {result.get('error', 'Unknown error')}")
                if result.get("abort_code"):
                    print(f"Abort code: {result.get('abort_code')} in {result.get('abort_module')}")

            print("=" * 60)

        result_json = json.dumps(result)
        if len(result_json) > 2000:
            result_json = result_json[:2000] + "..."
        print(f"Result: {result_json}")

        # Add to conversation
        messages.append({"role": "assistant", "content": response})

        # If PTB execution succeeded with TreasuryCap, we're done
        if ptb_executed and ptb_success:
            effects = result.get("effects", {})
            created = effects.get("created", [])
            treasury_created = any("TreasuryCap" in str(obj.get("object_type", "")) for obj in created)
            if treasury_created:
                print("\n" + "=" * 60)
                print("TEST PASSED - TreasuryCap successfully created!")
                print("=" * 60)
                sandbox.close()
                print(f"\nTotal iterations: {iteration + 1}")
                print(f"Total tool calls: {len(tool_calls_log)}")
                return

        messages.append({
            "role": "user",
            "content": f"Tool result:\n{json.dumps(result)}\n\nRespond with ONLY a JSON object containing \"tool\" and \"args\" keys."
        })

    print("\n" + "=" * 60)
    if ptb_executed:
        if ptb_success:
            print("TEST RESULT: PTB executed successfully, but target type not verified")
        else:
            print("TEST RESULT: PTB execution failed")
    else:
        print("TEST RESULT: Max iterations reached without PTB execution")
    print("=" * 60)

    sandbox.close()
    print(f"\nTotal iterations: {MAX_ITERATIONS}")
    print(f"Total tool calls: {len(tool_calls_log)}")


if __name__ == "__main__":
    main()
