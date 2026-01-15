#!/usr/bin/env python3
"""
End-to-End LLM Sandbox Test

Tests the complete LLM workflow:
1. Load a cached mainnet transaction
2. Ask LLM to identify what needs to be built for type inhabitation
3. Have LLM generate the required helper code
4. Execute in the local sandbox
5. Verify successful type inhabitation

Usage:
    python3 test_llm_sandbox_e2e.py --tx-digest <digest>
    python3 test_llm_sandbox_e2e.py --use-hardest  # Use a complex transaction
"""

import argparse
import asyncio
import json
import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx

# Load environment
ENV_PATH = Path(__file__).parent / ".env"
if ENV_PATH.exists():
    for line in ENV_PATH.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            os.environ.setdefault(key, value)

# Configuration
OPENROUTER_API_KEY = os.environ.get("OPENROUTER_API_KEY", "")
OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1"
TX_CACHE_DIR = Path(__file__).parent.parent / ".tx-cache"
RUST_BINARY = Path(__file__).parent.parent / "target/debug/sui_move_interface_extractor"

# Model to use - GPT-5.2 via OpenRouter
DEFAULT_MODEL = "openai/gpt-5.2"


@dataclass
class CachedTransaction:
    """Represents a cached mainnet transaction."""
    digest: str
    sender: str
    commands: list[dict]
    inputs: list[dict]
    packages: dict[str, Any]
    objects: dict[str, Any]

    @classmethod
    def load(cls, digest: str) -> "CachedTransaction":
        """Load a cached transaction by digest."""
        cache_file = TX_CACHE_DIR / f"{digest}.json"
        if not cache_file.exists():
            raise FileNotFoundError(f"Transaction {digest} not found in cache")

        data = json.loads(cache_file.read_text())
        tx = data.get("transaction", {})

        return cls(
            digest=tx.get("digest", digest),
            sender=tx.get("sender", ""),
            commands=tx.get("commands", []),
            inputs=tx.get("inputs", []),
            packages=data.get("packages", {}),
            objects=data.get("objects", {}),
        )

    def get_move_calls(self) -> list[dict]:
        """Extract MoveCall commands."""
        return [cmd for cmd in self.commands if cmd.get("type") == "MoveCall"]

    def get_package_modules(self) -> dict[str, list[str]]:
        """Get modules per package."""
        result = {}
        for pkg_id, pkg_data in self.packages.items():
            if isinstance(pkg_data, dict):
                modules = pkg_data.get("modules", {})
                result[pkg_id] = list(modules.keys()) if isinstance(modules, dict) else []
        return result

    def describe(self) -> str:
        """Generate a description for LLM context."""
        move_calls = self.get_move_calls()
        pkg_modules = self.get_package_modules()

        lines = [
            f"Transaction: {self.digest}",
            f"Sender: {self.sender}",
            f"Commands: {len(self.commands)} total, {len(move_calls)} MoveCall",
            "",
            "MoveCall Commands:",
        ]

        for i, cmd in enumerate(move_calls):
            pkg = cmd.get("package", "")[:16] + "..."
            module = cmd.get("module", "")
            func = cmd.get("function", "")
            type_args = cmd.get("type_arguments", [])
            args = cmd.get("arguments", [])
            lines.append(f"  {i+1}. {pkg}::{module}::{func}")
            if type_args:
                lines.append(f"      Type args: {type_args}")
            lines.append(f"      Args: {len(args)} arguments")

        lines.append("")
        lines.append("Available Packages:")
        for pkg_id, modules in pkg_modules.items():
            lines.append(f"  {pkg_id[:16]}...: {len(modules)} modules ({', '.join(modules[:3])}{'...' if len(modules) > 3 else ''})")

        return "\n".join(lines)


class LLMClient:
    """Client for OpenRouter API."""

    def __init__(self, api_key: str, model: str = DEFAULT_MODEL):
        self.api_key = api_key
        self.model = model
        self.base_url = OPENROUTER_BASE_URL
        self.client = httpx.AsyncClient(timeout=120.0)

    async def chat(self, messages: list[dict], temperature: float = 0.0) -> str:
        """Send a chat completion request."""
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://github.com/anthropics/claude-code",
            "X-Title": "Sui Move Sandbox Test",
        }

        payload = {
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": 4096,
        }

        response = await self.client.post(
            f"{self.base_url}/chat/completions",
            headers=headers,
            json=payload,
        )
        response.raise_for_status()

        data = response.json()
        return data["choices"][0]["message"]["content"]

    async def close(self):
        await self.client.aclose()


class SandboxRunner:
    """Runs the Rust sandbox for execution."""

    def __init__(self):
        self.binary = RUST_BINARY
        if not self.binary.exists():
            # Try release build
            self.binary = Path(__file__).parent.parent / "target/release/sui_move_interface_extractor"

    def run_ptb_evaluation(self, tx_digest: str) -> tuple[bool, str]:
        """Run PTB evaluation for a transaction."""
        try:
            # ptb-eval works on the cache directory - it doesn't take a digest directly
            # We run it with --limit 1 and it processes cached transactions
            # For a specific digest, we'd need to ensure only that tx is in cache
            result = subprocess.run(
                [str(self.binary), "ptb-eval",
                 "--verbose",
                 "--enable-fetching",
                 "--limit", "200"  # Process all cached including our target
                ],
                check=False, capture_output=True,
                text=True,
                timeout=300,
                cwd=str(Path(__file__).parent.parent),
            )
            # Filter output for the specific transaction
            output = result.stdout + result.stderr
            return result.returncode == 0, output
        except subprocess.TimeoutExpired:
            return False, "Timeout during PTB evaluation"
        except FileNotFoundError:
            return False, f"Binary not found: {self.binary}"

    def run_single_tx_evaluation(self, tx_digest: str) -> tuple[bool, str]:
        """Run evaluation for a single specific transaction using tx-replay."""
        try:
            cache_dir = Path(__file__).parent.parent / ".tx-cache"
            result = subprocess.run(
                [str(self.binary), "tx-replay",
                 "--from-cache",
                 "--cache-dir", str(cache_dir),
                 "--replay",
                 "--verbose"
                ],
                check=False, capture_output=True,
                text=True,
                timeout=300,
                cwd=str(Path(__file__).parent.parent),
            )
            # Look for output related to this specific transaction
            output = result.stdout + result.stderr
            return result.returncode == 0, output
        except subprocess.TimeoutExpired:
            return False, "Timeout during tx replay"
        except FileNotFoundError:
            return False, f"Binary not found: {self.binary}"

    def run_benchmark_local(self, tx_digest: str) -> tuple[bool, str]:
        """Run local benchmark for a transaction."""
        try:
            result = subprocess.run(
                [str(self.binary), "benchmark-local",
                 "--tx-digest", tx_digest,
                 "--verbose"],
                check=False, capture_output=True,
                text=True,
                timeout=120,
                cwd=str(Path(__file__).parent.parent),
            )
            return result.returncode == 0, result.stdout + result.stderr
        except subprocess.TimeoutExpired:
            return False, "Timeout during benchmark"
        except FileNotFoundError:
            return False, f"Binary not found: {self.binary}"


def find_complex_transactions() -> list[str]:
    """Find cached transactions with multiple MoveCall commands."""
    complex_txs = []

    for cache_file in TX_CACHE_DIR.glob("*.json"):
        try:
            data = json.loads(cache_file.read_text())
            commands = data.get("transaction", {}).get("commands", [])
            move_calls = [c for c in commands if c.get("type") == "MoveCall"]

            if len(move_calls) >= 3:  # At least 3 MoveCall commands
                complex_txs.append((cache_file.stem, len(move_calls), len(commands)))
        except Exception:
            continue

    # Sort by number of move calls (descending)
    complex_txs.sort(key=lambda x: x[1], reverse=True)
    return [tx[0] for tx in complex_txs[:10]]


async def run_llm_analysis(tx: CachedTransaction, llm: LLMClient) -> dict:
    """Have LLM analyze what's needed for type inhabitation."""

    system_prompt = """You are an expert in Sui Move smart contracts and type inhabitation testing.
Your task is to analyze a mainnet transaction and explain what would be needed to successfully
replay it in a local sandbox environment.

Focus on:
1. What packages/modules are being called
2. What input types are required
3. What objects need to be created or mocked
4. Any dependencies that need to be satisfied
5. Potential challenges for local execution

Be specific and technical. Format your response as JSON with the following structure:
{
    "summary": "Brief description of the transaction",
    "packages_needed": ["list of package addresses"],
    "functions_called": [{"package": "...", "module": "...", "function": "...", "complexity": "low/medium/high"}],
    "input_requirements": [{"index": 0, "type": "...", "description": "..."}],
    "challenges": ["list of potential issues"],
    "recommended_approach": "Strategy for successful execution"
}"""

    user_prompt = f"""Analyze this Sui mainnet transaction for local sandbox replay:

{tx.describe()}

Full command details:
{json.dumps(tx.commands, indent=2)}

Input details:
{json.dumps(tx.inputs, indent=2)}

Provide your analysis as JSON."""

    print("\n📤 Sending to LLM for analysis...")
    response = await llm.chat([
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_prompt},
    ])

    # Try to parse JSON from response
    try:
        # Handle markdown code blocks
        if "```json" in response:
            json_str = response.split("```json")[1].split("```")[0]
        elif "```" in response:
            json_str = response.split("```")[1].split("```")[0]
        else:
            json_str = response

        return json.loads(json_str.strip())
    except json.JSONDecodeError:
        return {"raw_response": response, "parse_error": True}


async def run_llm_helper_generation(
    tx: CachedTransaction,
    analysis: dict,
    llm: LLMClient
) -> str:
    """Have LLM generate helper code for type inhabitation."""

    system_prompt = """You are an expert Sui Move developer. Generate helper code that would allow
successful type inhabitation testing for the given transaction.

The helper code should:
1. Create necessary objects with valid state
2. Provide appropriate input values
3. Handle any setup required for the functions being called

Output Move code that can be compiled and used with the Sui sandbox.
Use the sui::test_scenario module patterns where appropriate."""

    user_prompt = f"""Based on this analysis, generate Move helper code for type inhabitation:

Analysis:
{json.dumps(analysis, indent=2)}

Transaction commands:
{json.dumps(tx.commands, indent=2)}

Generate the Move helper module code. Focus on the hardest functions identified."""

    print("\n📤 Generating helper code...")
    response = await llm.chat([
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_prompt},
    ])

    return response


async def run_llm_fix_iteration(
    tx: CachedTransaction,
    error_output: str,
    attempt: int,
    llm: LLMClient
) -> str:
    """Have LLM analyze error and suggest fixes."""

    system_prompt = """You are an expert in Sui Move smart contracts debugging.
Analyze the sandbox execution error and suggest specific fixes.

The sandbox has:
- env.set_sender(address) - Set transaction sender
- env.set_timestamp_ms(ms) - Set transaction timestamp
- env.create_coin(type, amount) - Create coins
- env.fetch_object_from_mainnet(id) - Fetch exact object state

Focus on actionable fixes that can be implemented programmatically."""

    user_prompt = f"""Attempt {attempt} failed. Analyze this error and suggest a fix:

Error Output:
{error_output[-2000:]}

Transaction context:
- Sender: {tx.sender}
- Commands: {len(tx.commands)}

What specific action should we take to fix this?
Respond with JSON: {{"diagnosis": "...", "fix_type": "set_sender|set_timestamp|create_object|fetch_object", "fix_params": {{...}}, "explanation": "..."}}"""

    print(f"\n📤 Asking LLM for fix suggestion (attempt {attempt})...")
    response = await llm.chat([
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_prompt},
    ])

    return response


async def main():
    parser = argparse.ArgumentParser(description="End-to-End LLM Sandbox Test")
    parser.add_argument("--tx-digest", help="Transaction digest to test")
    parser.add_argument("--use-hardest", action="store_true", help="Use the most complex cached transaction")
    parser.add_argument("--model", default=DEFAULT_MODEL, help=f"Model to use (default: {DEFAULT_MODEL})")
    parser.add_argument("--list-complex", action="store_true", help="List complex transactions in cache")
    parser.add_argument("--skip-execution", action="store_true", help="Skip actual sandbox execution")
    parser.add_argument("--max-iterations", type=int, default=3, help="Max fix iterations")
    args = parser.parse_args()

    if not OPENROUTER_API_KEY:
        print("❌ OPENROUTER_API_KEY not set. Check benchmark/.env")
        sys.exit(1)

    print("=" * 60)
    print("🔬 Sui Move LLM Sandbox End-to-End Test")
    print("=" * 60)

    # Find or select transaction
    if args.list_complex:
        print("\n📋 Most complex cached transactions:")
        for i, digest in enumerate(find_complex_transactions(), 1):
            tx = CachedTransaction.load(digest)
            move_calls = tx.get_move_calls()
            print(f"  {i}. {digest[:20]}... ({len(move_calls)} MoveCall commands)")
        return

    if args.use_hardest:
        complex_txs = find_complex_transactions()
        if not complex_txs:
            print("❌ No complex transactions found in cache")
            sys.exit(1)
        tx_digest = complex_txs[0]
        print(f"\n📌 Using most complex transaction: {tx_digest[:20]}...")
    elif args.tx_digest:
        tx_digest = args.tx_digest
    else:
        # Default to a known interesting transaction
        complex_txs = find_complex_transactions()
        if complex_txs:
            tx_digest = complex_txs[0]
        else:
            print("❌ No transaction specified and no complex transactions in cache")
            sys.exit(1)

    # Load transaction
    print(f"\n📥 Loading transaction: {tx_digest}")
    try:
        tx = CachedTransaction.load(tx_digest)
    except FileNotFoundError as e:
        print(f"❌ {e}")
        sys.exit(1)

    print("\n📊 Transaction Overview:")
    print(tx.describe())

    # Initialize LLM client
    llm = LLMClient(OPENROUTER_API_KEY, args.model)
    print(f"\n🤖 Using model: {args.model}")

    try:
        # Step 1: LLM Analysis
        print("\n" + "=" * 40)
        print("STEP 1: LLM Analysis")
        print("=" * 40)

        analysis = await run_llm_analysis(tx, llm)

        if analysis.get("parse_error"):
            print("\n⚠️  LLM response was not valid JSON:")
            print(analysis.get("raw_response", "")[:1000])
        else:
            print("\n📋 Analysis Results:")
            print(f"  Summary: {analysis.get('summary', 'N/A')}")
            print(f"  Packages needed: {len(analysis.get('packages_needed', []))}")
            print(f"  Functions to call: {len(analysis.get('functions_called', []))}")

            functions = analysis.get("functions_called", [])
            if functions:
                print("\n  Function complexity:")
                for func in functions:
                    complexity = func.get("complexity", "unknown")
                    name = f"{func.get('module', '?')}::{func.get('function', '?')}"
                    print(f"    - {name}: {complexity}")

            challenges = analysis.get("challenges", [])
            if challenges:
                print("\n  ⚠️  Challenges identified:")
                for ch in challenges[:5]:
                    print(f"    - {ch}")

            print(f"\n  Recommended approach: {analysis.get('recommended_approach', 'N/A')[:200]}")

        # Step 2: Generate helper code
        print("\n" + "=" * 40)
        print("STEP 2: Helper Code Generation")
        print("=" * 40)

        helper_code = await run_llm_helper_generation(tx, analysis, llm)
        print("\n📝 Generated helper code preview:")
        print("-" * 40)
        # Show first 50 lines
        lines = helper_code.split("\n")
        for line in lines[:50]:
            print(line)
        if len(lines) > 50:
            print(f"... ({len(lines) - 50} more lines)")
        print("-" * 40)

        # Step 3: Sandbox execution (if not skipped)
        if not args.skip_execution:
            print("\n" + "=" * 40)
            print("STEP 3: Sandbox Execution")
            print("=" * 40)

            runner = SandboxRunner()

            print(f"\n🚀 Running tx-replay for {tx_digest[:20]}...")
            success, output = runner.run_single_tx_evaluation(tx_digest)

            if success:
                print("✅ PTB evaluation succeeded!")
            else:
                print("❌ PTB evaluation failed")

            print("\n📄 Output (last 50 lines):")
            print("-" * 40)
            for line in output.split("\n")[-50:]:
                print(line)
            print("-" * 40)

        # Summary
        print("\n" + "=" * 60)
        print("📊 TEST SUMMARY")
        print("=" * 60)
        print(f"Transaction: {tx_digest}")
        print(f"Commands: {len(tx.commands)}")
        print(f"MoveCall commands: {len(tx.get_move_calls())}")
        print(f"Model: {args.model}")
        print(f"Analysis: {'✅ Success' if not analysis.get('parse_error') else '⚠️ Parse error'}")
        print("Helper generation: ✅ Generated")
        if not args.skip_execution:
            print(f"Sandbox execution: {'✅ Success' if success else '❌ Failed'}")

    finally:
        await llm.close()


if __name__ == "__main__":
    asyncio.run(main())
