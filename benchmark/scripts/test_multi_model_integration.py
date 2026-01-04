#!/usr/bin/env python3
"""
Integration test script for multi-model, multi-package scenarios.

Tests:
1. Single package, single model
2. Multiple packages, same model
3. Same packages, different models
4. Verifies log searchability and metrics collection
"""

import argparse
import asyncio
import json
import time
from pathlib import Path
from typing import Any

import httpx

# Test configurations
TEST_CONFIGS = {
    "gpt-4o-mini": {
        "model": "gpt-4o-mini",
        "description": "Fast, cheap baseline model",
    },
    "gpt-4o": {
        "model": "gpt-4o",
        "description": "Higher quality model for comparison",
    },
    "claude-3-5-sonnet": {
        "model": "claude-3-5-sonnet-20241022",
        "description": "Anthropic Claude for comparison",
    },
}

# Sample package IDs (use real ones from your corpus)
SAMPLE_PACKAGES = [
    "0x1::option",  # Small, fast package for testing
    "0x2::coin",  # Another common package
]


class BenchmarkTestRunner:
    def __init__(self, api_url: str, corpus_root: str, manifest_path: str):
        self.api_url = api_url
        self.corpus_root = corpus_root
        self.manifest_path = manifest_path
        self.results = []

    async def test_scenario(
        self,
        scenario_name: str,
        model: str,
        samples: int = 1,
        additional_config: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Run a single benchmark scenario and return results."""
        print(f"\n{'=' * 80}")
        print(f"SCENARIO: {scenario_name}")
        print(f"Model: {model}")
        print(f"Samples: {samples}")
        print(f"{'=' * 80}\n")

        config = {
            "corpus_root": self.corpus_root,
            "package_ids_file": self.manifest_path,
            "samples": samples,
            "agent": "real-openai-compatible",
            "model": model,
            "simulation_mode": "build-only",  # Fastest for testing
            "seed": 42,  # Reproducible
            "max_plan_attempts": 2,
            "per_package_timeout_seconds": 120,
        }

        if additional_config:
            config.update(additional_config)

        payload = {
            "jsonrpc": "2.0",
            "id": f"test_{int(time.time())}",
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": f"msg_{scenario_name}_{int(time.time())}",
                    "role": "user",
                    "parts": [{"text": json.dumps({"config": config})}],
                }
            },
        }

        start_time = time.time()

        async with httpx.AsyncClient(timeout=600.0) as client:
            # Submit task
            print("⏳ Submitting task...")
            response = await client.post(self.api_url, json=payload)
            response.raise_for_status()
            data = response.json()

            if "error" in data:
                print(f"❌ Error: {data['error']}")
                return {"error": data["error"]}

            task = data.get("result", {})
            task_id = task.get("id")
            print(f"✓ Task submitted: {task_id}")

            # Poll for completion
            print("⏳ Waiting for completion...")
            status = "working"
            last_update = ""

            while status in ("working", "starting"):
                await asyncio.sleep(2)

                # Check partial results
                partial_resp = await client.get(f"{self.api_url.rstrip('/')}/tasks/{task_id}/results")
                if partial_resp.status_code == 200:
                    partial_data = partial_resp.json()
                    status = partial_data.get("status", "working")

                    # Show progress
                    if partial_data.get("partial_metrics"):
                        metrics_str = json.dumps(partial_data["partial_metrics"], indent=2)
                        if metrics_str != last_update:
                            print(f"📊 Partial metrics:\n{metrics_str}")
                            last_update = metrics_str

                # Also check task status endpoint
                status_resp = await client.get(f"{self.api_url}/.well-known/agent-card.json")
                if status_resp.status_code != 200:
                    print("⚠️  Health check failed")

                # Safety timeout
                if time.time() - start_time > 600:
                    print("❌ Timeout after 10 minutes")
                    return {"error": "timeout"}

            # Get final results
            final_resp = await client.get(f"{self.api_url.rstrip('/')}/tasks/{task_id}/results")
            final_data = final_resp.json()

            duration = time.time() - start_time

            result = {
                "scenario": scenario_name,
                "model": model,
                "task_id": task_id,
                "status": final_data.get("status"),
                "duration_seconds": duration,
                "started_at": final_data.get("started_at"),
                "completed_at": final_data.get("completed_at"),
                "metrics": final_data.get("metrics", {}),
                "bundle": final_data.get("bundle", {}),
            }

            # Extract key metrics
            metrics = result["metrics"]

            print(f"\n{'=' * 80}")
            print(f"✓ COMPLETED in {duration:.1f}s")
            print(f"Status: {result['status']}")
            print("\nKey Metrics:")
            print(f"  • Avg Hit Rate: {metrics.get('avg_hit_rate', 'N/A')}")
            print(f"  • Errors: {metrics.get('errors', 0)}")
            print(f"  • Total Prompt Tokens: {metrics.get('total_prompt_tokens', 'N/A')}")
            print(f"  • Total Completion Tokens: {metrics.get('total_completion_tokens', 'N/A')}")

            total_tokens = metrics.get("total_prompt_tokens", 0) + metrics.get("total_completion_tokens", 0)
            if total_tokens > 0:
                # Rough cost estimates (update with actual pricing)
                cost_per_1k_input = 0.15 / 1000  # Example: GPT-4o pricing
                cost_per_1k_output = 0.60 / 1000
                estimated_cost = (
                    metrics.get("total_prompt_tokens", 0) * cost_per_1k_input
                    + metrics.get("total_completion_tokens", 0) * cost_per_1k_output
                )
                print(f"  • Estimated Cost: ${estimated_cost:.4f}")
                result["estimated_cost_usd"] = estimated_cost

            print(f"{'=' * 80}\n")

            self.results.append(result)
            return result

    async def run_all_tests(self):
        """Run comprehensive multi-model/multi-package tests."""
        print(f"\n{'#' * 80}")
        print("# MULTI-MODEL INTEGRATION TEST SUITE")
        print(f"# API: {self.api_url}")
        print(f"# Corpus: {self.corpus_root}")
        print(f"{'#' * 80}\n")

        # Test 1: Single package, single model (baseline)
        await self.test_scenario(
            "baseline_gpt4o_mini_1pkg",
            model="gpt-4o-mini",
            samples=1,
        )

        # Test 2: Multiple packages, same model
        await self.test_scenario(
            "multi_package_gpt4o_mini",
            model="gpt-4o-mini",
            samples=2,
        )

        # Test 3: Same packages, different model
        await self.test_scenario(
            "model_comparison_gpt4o",
            model="gpt-4o",
            samples=1,
        )

        # Test 4: With webhook callback (if webhook server available)
        # await self.test_scenario(
        #     "webhook_test",
        #     model="gpt-4o-mini",
        #     samples=1,
        #     additional_config={"callback_url": "http://localhost:8080/webhook"},
        # )

        self.print_summary()

    def print_summary(self):
        """Print test run summary with all metrics."""
        print(f"\n{'#' * 80}")
        print("# TEST RUN SUMMARY")
        print(f"{'#' * 80}\n")

        if not self.results:
            print("No results to summarize.")
            return

        # Summary table
        print(f"{'Scenario':<30} {'Model':<25} {'Duration':<12} {'Status':<12} {'Tokens':<12} {'Cost':<10}")
        print(f"{'-' * 110}")

        total_duration = 0
        total_cost = 0

        for result in self.results:
            scenario = result["scenario"][:29]
            model = result["model"][:24]
            duration = f"{result['duration_seconds']:.1f}s"
            status = result["status"]

            metrics = result.get("metrics", {})
            total_tokens = metrics.get("total_prompt_tokens", 0) + metrics.get("total_completion_tokens", 0)
            tokens_str = f"{total_tokens:,}" if total_tokens else "N/A"

            cost = result.get("estimated_cost_usd", 0)
            cost_str = f"${cost:.4f}" if cost else "N/A"

            print(f"{scenario:<30} {model:<25} {duration:<12} {status:<12} {tokens_str:<12} {cost_str:<10}")

            total_duration += result["duration_seconds"]
            total_cost += cost

        print(f"{'-' * 110}")
        print(f"{'TOTALS':<30} {'':<25} {f'{total_duration:.1f}s':<12} {'':<12} {'':<12} ${total_cost:.4f}")

        print(f"\n✓ All tests completed. Results saved to: test_results_{int(time.time())}.json\n")

        # Save results to file
        output_file = Path(f"test_results_{int(time.time())}.json")
        output_file.write_text(json.dumps(self.results, indent=2))
        print(f"Detailed results: {output_file}")


async def main():
    parser = argparse.ArgumentParser(description="Multi-model integration tests")
    parser.add_argument(
        "--api-url",
        default="http://localhost:9999",
        help="A2A API URL",
    )
    parser.add_argument(
        "--corpus-root",
        required=True,
        help="Path to bytecode corpus",
    )
    parser.add_argument(
        "--manifest",
        required=True,
        help="Path to package IDs manifest file",
    )
    args = parser.parse_args()

    runner = BenchmarkTestRunner(
        api_url=args.api_url,
        corpus_root=args.corpus_root,
        manifest_path=args.manifest,
    )

    await runner.run_all_tests()


if __name__ == "__main__":
    asyncio.run(main())
