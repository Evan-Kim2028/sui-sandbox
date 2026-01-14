#!/usr/bin/env python3
"""
Test GPT-5.2 on the artipedia transaction using the LLM toolkit.

This test includes bytecode disassembly to help the LLM understand
what happens at the abort instruction offset.

Usage:
    OPENROUTER_API_KEY=... python benchmark/scripts/test_gpt52_artipedia.py
"""

import json
import os
import subprocess
import sys
from pathlib import Path

import httpx

# Configuration
MODEL = "openai/gpt-5.2"
BASE_URL = "https://openrouter.ai/api/v1"
MAX_ITERATIONS = 2

# Bytecode disassembly for update_points function
# This was extracted using the DisassembleFunction tool
UPDATE_POINTS_DISASSEMBLY = """
public update_points(Arg0: &mut UserNumber, Arg1: u64, Arg2: &mut TxContext) {
B0:
	0: CopyLoc[2](Arg2: &mut TxContext)
	1: Call tx_context::sender(&TxContext): address
	2: StLoc[4](loc1: address)
	3: CopyLoc[2](Arg2: &mut TxContext)
	4: Call tx_context::sender(&TxContext): address
	5: CopyLoc[0](Arg0: &mut UserNumber)
	6: ImmBorrowField[6](UserNumber.owner: address)
	7: ReadRef
	8: Eq
	9: BrFalse(11)
B1:
	10: Branch(15)
B2:
	11: MoveLoc[0](Arg0: &mut UserNumber)
	12: Pop
	13: LdConst[1](u64: 2)
	14: Abort
B3:
	15: CopyLoc[0](Arg0: &mut UserNumber)
	16: ImmBorrowField[7](UserNumber.value: u64)
	17: ReadRef
	18: StLoc[3](loc0: u64)
	19: CopyLoc[1](Arg1: u64)
	20: CopyLoc[0](Arg0: &mut UserNumber)
	21: MutBorrowField[7](UserNumber.value: u64)
	22: WriteRef
	23: MoveLoc[4](loc1: address)
	24: MoveLoc[3](loc0: u64)
	25: MoveLoc[1](Arg1: u64)
	26: MoveLoc[0](Arg0: &mut UserNumber)
	27: ImmBorrowField[5](UserNumber.id: UID)
	28: Call object::uid_to_inner(&UID): ID
	29: Pack[5](NumberUpdated)
	30: Call event::emit<NumberUpdated>(NumberUpdated)
	31: Ret
}
"""

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
        "HTTP-Referer": "https://github.com/MystenLabs/sui-move-interface-extractor",
        "X-Title": "Sui Move Artipedia Test",
    }
    payload = {
        "model": MODEL,
        "temperature": temperature,
        "messages": messages,
        "max_tokens": 4096,
    }

    with httpx.Client(timeout=120) as client:
        r = client.post(url, headers=headers, json=payload)
        r.raise_for_status()
        data = r.json()
        return data["choices"][0]["message"]["content"]

def load_artipedia_context() -> dict:
    """Load artipedia transaction data from cache."""
    # Run a quick rust command to extract the context
    result = subprocess.run(
        ["cargo", "test", "test_llm_toolkit_artipedia", "--", "--nocapture"],
        cwd="/home/evan/Documents/sui-move-interface-extractor",
        capture_output=True,
        text=True,
    )

    # Parse module info from test output
    output = result.stdout + result.stderr

    # Extract key info
    context = {
        "package": "0xb7c36a747d6fdd6b59ab0354cea52a31df078c242242465a867481b6f4509498",
        "module": "artipedia",
        "function": "update_points",
        "error": "abort code 2 at instruction offset 14",
        "user_number_struct": {
            "fields": [
                {"name": "id", "type": "UID"},
                {"name": "value", "type": "u64"},
                {"name": "owner", "type": "address"},
            ]
        },
        "update_points_signature": {
            "params": ["&mut UserNumber", "u64", "&mut TxContext"],
            "returns": [],
        },
    }
    return context

def build_prompt(context: dict, iteration: int, previous_result: str | None = None) -> str:
    """Build the prompt for the LLM."""

    base_prompt = f"""You are testing a Move smart contract on Sui blockchain.

## Transaction Context
- Package: {context['package']}
- Module: {context['module']}
- Function: {context['function']}

## Function Signature
`{context['function']}({', '.join(context['update_points_signature']['params'])})`

## UserNumber Struct
```
struct UserNumber has key, store {{
    id: UID,
    value: u64,
    owner: address,
}}
```

## Bytecode Disassembly of update_points
The following is the actual bytecode disassembly showing each instruction with its offset:
```
{UPDATE_POINTS_DISASSEMBLY}
```

## Problem
When we execute `update_points` on a synthesized UserNumber object, it fails with:
- Abort code: 2
- Instruction offset: 14

Look at instruction 14 in the disassembly above - this is where the abort happens.

## Available Tools
You have access to these tools via the LLM toolkit:
1. `ListModules` - List all loaded modules
2. `ListStructs {{ module_path }}` - List structs in a module
3. `GetStructInfo {{ module_path, struct_name }}` - Get struct details
4. `ListFunctions {{ module_path }}` - List functions in a module
5. `GetFunctionInfo {{ module_path, function_name }}` - Get function signature
6. `DisassembleFunction {{ module_path, function_name }}` - Get bytecode disassembly
7. `CreateObject {{ type_path, fields, is_shared }}` - Synthesize an object
8. `ParseError {{ error }}` - Parse an error string

## Task
Analyze the bytecode disassembly to understand exactly why the transaction is failing.

Think step by step:
1. Look at instruction 14 - what operation causes the abort?
2. Trace back through the basic blocks to understand the condition that leads to the abort
3. What check is being performed before the abort?
4. What must be true for the transaction to succeed?

Respond with a JSON object:
{{
    "analysis": "Your detailed analysis of the bytecode",
    "instruction_14_meaning": "What instruction 14 does",
    "abort_condition": "The condition that causes the abort",
    "root_cause": "The actual root cause of the failure",
    "solution": "How to fix it - what the UserNumber.owner field should be set to",
    "required_objects": [
        {{
            "type_path": "full type path",
            "fields": {{}},
            "is_shared": true/false
        }}
    ]
}}
"""

    if previous_result and iteration > 0:
        base_prompt += f"\n\n## Previous Attempt Result\n{previous_result}\n\nBased on this result, refine your approach."

    return base_prompt

def main():
    print("=" * 60)
    print("GPT-5.2 Artipedia Transaction Test")
    print("=" * 60)
    print(f"Model: {MODEL}")
    print(f"Max iterations: {MAX_ITERATIONS}")
    print()

    api_key = get_api_key()
    print("API key loaded")

    # Load context
    print("\nLoading artipedia context...")
    context = load_artipedia_context()
    print(f"  Package: {context['package'][:20]}...")
    print(f"  Function: {context['module']}::{context['function']}")

    # Run iterations
    previous_result = None
    for iteration in range(MAX_ITERATIONS):
        print(f"\n{'='*60}")
        print(f"ITERATION {iteration + 1}/{MAX_ITERATIONS}")
        print("=" * 60)

        # Build prompt
        prompt = build_prompt(context, iteration, previous_result)
        print(f"\nPrompt length: {len(prompt)} chars")

        # Call LLM
        print("\nCalling GPT-5.2...")
        messages = [
            {"role": "system", "content": "You are an expert Move developer analyzing smart contract failures. Respond with valid JSON."},
            {"role": "user", "content": prompt},
        ]

        try:
            response = call_llm(api_key, messages)
            print(f"\nResponse length: {len(response)} chars")
            print("\n--- GPT-5.2 Response ---")
            print(response)
            print("--- End Response ---")

            # Try to parse as JSON
            try:
                parsed = json.loads(response)
                print("\n--- Parsed JSON ---")
                print(json.dumps(parsed, indent=2))

                # Store for next iteration
                previous_result = json.dumps(parsed, indent=2)

                # Check if we have a solution
                if parsed.get("required_objects"):
                    print(f"\n[Iteration {iteration+1}] Found {len(parsed['required_objects'])} required objects")
                    for obj in parsed["required_objects"]:
                        print(f"  - {obj.get('type_path', 'unknown')}")

            except json.JSONDecodeError as e:
                print(f"\nFailed to parse JSON: {e}")
                previous_result = response

        except Exception as e:
            print(f"\nError calling LLM: {e}")
            previous_result = str(e)

    print("\n" + "=" * 60)
    print("Test complete")
    print("=" * 60)

if __name__ == "__main__":
    main()
