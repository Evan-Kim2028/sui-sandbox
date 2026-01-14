#!/usr/bin/env python3
"""
Test GPT-5.2 on multiple PTBs with bytecode disassembly support.

Usage:
    OPENROUTER_API_KEY=... python benchmark/scripts/test_gpt52_multi.py
"""

import json
import os
import subprocess
import sys

import httpx

# Configuration
MODEL = "openai/gpt-5.2"
BASE_URL = "https://openrouter.ai/api/v1"
MAX_ITERATIONS = 2

# Test transactions - moderately complex DeFi operations
TEST_TRANSACTIONS = [
    {
        "name": "DeepBook Order Placement",
        "digest": "23CGb43kq21r6w5VK4zR9zMCpcYBmvrvsyuDmWXaRhRB",
        "description": "Places a limit order on DeepBook DEX. Requires balance manager proof and pool access.",
    },
    {
        "name": "Flashloan Swap",
        "digest": "6adrRRJ7WbHm1kyp2mNPMwAnt8uk8EyBxhvLi6ypv36X",
        "description": "Borrows via flashloan, swaps through DeepBook, confirms via router.",
    },
]

def get_api_key():
    key = os.environ.get("OPENROUTER_API_KEY") or os.environ.get("SMI_API_KEY")
    if not key:
        print("ERROR: Set OPENROUTER_API_KEY environment variable")
        sys.exit(1)
    return key


def call_llm(api_key: str, messages: list[dict], temperature: float = 0.0) -> str:
    """Call GPT-5.2 via OpenRouter."""
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


def get_disassembly_for_function(toolkit_output: str, module_path: str, function_name: str) -> str | None:
    """Run rust test to get disassembly for a function."""
    # For now, we'll extract from pre-run test output or describe the approach
    # In production, this would call the toolkit directly
    return None


def extract_transaction_info(data: dict) -> dict:
    """Extract key information from cached transaction."""
    tx = data.get("transaction", {})
    commands = tx.get("commands", [])

    move_calls = []
    for cmd in commands:
        if cmd.get("type") == "MoveCall":
            move_calls.append({
                "package": cmd.get("package", "")[:20] + "...",
                "module": cmd.get("module", ""),
                "function": cmd.get("function", ""),
                "type_arguments": cmd.get("type_arguments", []),
            })

    packages = list(data.get("packages", {}).keys())
    objects = list(data.get("objects", {}).keys())[:10]  # First 10 objects

    return {
        "digest": tx.get("digest", ""),
        "sender": tx.get("sender", ""),
        "commands": move_calls,
        "num_packages": len(packages),
        "package_ids": packages[:5],
        "num_objects": len(data.get("objects", {})),
        "sample_objects": objects,
    }


def build_prompt(tx_info: dict, iteration: int, previous_result: str | None = None) -> str:
    """Build the prompt for analyzing a PTB."""

    commands_str = ""
    for i, cmd in enumerate(tx_info["commands"]):
        commands_str += f"  {i}. {cmd['module']}::{cmd['function']}\n"
        if cmd["type_arguments"]:
            commands_str += f"     Type args: {cmd['type_arguments'][:2]}...\n"

    prompt = f"""You are analyzing a Sui Programmable Transaction Block (PTB) to understand why it might fail in a sandbox environment.

## Transaction Context
- Digest: {tx_info['digest']}
- Sender: {tx_info['sender'][:20]}...
- Number of packages: {tx_info['num_packages']}
- Number of objects: {tx_info['num_objects']}

## Commands in PTB
{commands_str}

## Sample Package IDs
{chr(10).join(tx_info['package_ids'][:3])}

## Task
Analyze this PTB and determine:
1. What is this transaction trying to accomplish?
2. What objects/state would need to be synthesized to replay this transaction?
3. What are the likely failure modes when replaying in a sandbox?

## Available Tools
You have access to these tools (imagine you can call them):
- `ListModules` - List all loaded modules
- `GetStructInfo {{ module_path, struct_name }}` - Get struct details
- `GetFunctionInfo {{ module_path, function_name }}` - Get function signature
- `DisassembleFunction {{ module_path, function_name }}` - Get bytecode
- `CreateObject {{ type_path, fields, is_shared }}` - Synthesize an object

Respond with a JSON object:
{{
    "analysis": "What this transaction does",
    "required_state": [
        {{
            "type": "object type",
            "description": "why it's needed",
            "is_shared": true/false
        }}
    ],
    "likely_failures": [
        {{
            "failure_mode": "description",
            "cause": "what causes it",
            "solution": "how to fix"
        }}
    ],
    "complexity_assessment": "easy/medium/hard - how hard to replay"
}}
"""

    if previous_result and iteration > 0:
        prompt += f"\n\n## Previous Analysis\n{previous_result}\n\nRefine your analysis based on this."

    return prompt


def test_transaction(api_key: str, tx_config: dict) -> dict:
    """Test GPT-5.2 on a single transaction."""
    print(f"\n{'='*60}")
    print(f"Testing: {tx_config['name']}")
    print(f"Digest: {tx_config['digest']}")
    print(f"Description: {tx_config['description']}")
    print("="*60)

    # Load transaction data
    data = load_transaction_context(tx_config["digest"])
    if not data:
        return {"error": "Transaction not found in cache"}

    tx_info = extract_transaction_info(data)
    print(f"\nCommands: {len(tx_info['commands'])}")
    for cmd in tx_info["commands"][:5]:
        print(f"  - {cmd['module']}::{cmd['function']}")

    results = []
    previous_result = None

    for iteration in range(MAX_ITERATIONS):
        print(f"\n--- Iteration {iteration + 1}/{MAX_ITERATIONS} ---")

        prompt = build_prompt(tx_info, iteration, previous_result)
        print(f"Prompt length: {len(prompt)} chars")

        messages = [
            {"role": "system", "content": "You are an expert Sui/Move developer analyzing transaction replays. Respond with valid JSON."},
            {"role": "user", "content": prompt},
        ]

        try:
            print("Calling GPT-5.2...")
            response = call_llm(api_key, messages)
            print(f"Response length: {len(response)} chars")

            # Try to parse JSON
            try:
                # Handle markdown code blocks
                if "```json" in response:
                    response = response.split("```json")[1].split("```")[0]
                elif "```" in response:
                    response = response.split("```")[1].split("```")[0]

                parsed = json.loads(response)
                results.append({
                    "iteration": iteration + 1,
                    "success": True,
                    "response": parsed,
                })
                previous_result = json.dumps(parsed, indent=2)

                print("\n--- Parsed Response ---")
                print(f"Analysis: {parsed.get('analysis', 'N/A')[:200]}...")
                print(f"Required state: {len(parsed.get('required_state', []))} items")
                print(f"Likely failures: {len(parsed.get('likely_failures', []))} items")
                print(f"Complexity: {parsed.get('complexity_assessment', 'N/A')}")

            except json.JSONDecodeError as e:
                results.append({
                    "iteration": iteration + 1,
                    "success": False,
                    "error": f"JSON parse error: {e}",
                    "raw_response": response[:500],
                })
                previous_result = response

        except Exception as e:
            results.append({
                "iteration": iteration + 1,
                "success": False,
                "error": str(e),
            })
            print(f"Error: {e}")

    return {
        "transaction": tx_config["name"],
        "digest": tx_config["digest"],
        "results": results,
    }


def main():
    print("="*60)
    print("GPT-5.2 Multi-PTB Analysis Test")
    print("="*60)
    print(f"Model: {MODEL}")
    print(f"Iterations per PTB: {MAX_ITERATIONS}")
    print(f"PTBs to test: {len(TEST_TRANSACTIONS)}")
    print()

    api_key = get_api_key()
    print("API key loaded")

    all_results = []

    for tx_config in TEST_TRANSACTIONS:
        result = test_transaction(api_key, tx_config)
        all_results.append(result)

    # Summary
    print("\n" + "="*60)
    print("SUMMARY")
    print("="*60)

    for result in all_results:
        print(f"\n{result['transaction']}:")
        for r in result.get("results", []):
            status = "OK" if r.get("success") else "FAIL"
            print(f"  Iteration {r['iteration']}: {status}")
            if r.get("success") and r.get("response"):
                complexity = r["response"].get("complexity_assessment", "?")
                failures = len(r["response"].get("likely_failures", []))
                print(f"    Complexity: {complexity}, Failures identified: {failures}")

    print("\n" + "="*60)
    print("Test complete")
    print("="*60)


if __name__ == "__main__":
    main()
