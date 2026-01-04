#!/usr/bin/env python3
"""
Real-World Production Test Script

Tests Sui Move benchmark on real mainnet packages using multiple models via API.

Usage:
    uv run python3 run_real_world_test.py \
        --samples 2 \
        --models gpt-4o,google/gemini-3-flash-preview,google/gemini-2.5-flash-preview \
        --simulation-mode dry-run
"""

import argparse
import asyncio
import json
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List

import httpx

# Default configuration
DEFAULT_API_URL = "http://localhost:9999"
DEFAULT_CORPUS_ROOT = "/app/corpus"
DEFAULT_MANIFEST = "/app/corpus/manifest.txt"
# Host-mounted paths (when running from host)
DEFAULT_CORPUS_ROOT_HOST = "/app/corpus"
DEFAULT_MANIFEST_HOST = "/app/corpus/manifest.txt"
DEFAULT_SAMPLES = 2
DEFAULT_TIMEOUT = 600  # 10 minutes per task

# Model configurations
MODEL_CONFIGS = {
    "gpt-4o": {
        "name": "GPT-4o",
        "provider": "openai",
        "description": "OpenAI GPT-4o (top tier)",
        "expected_duration": "30-120s per package",
    },
    "gpt-4o-mini": {
        "name": "GPT-4o-mini",
        "provider": "openai",
        "description": "OpenAI GPT-4o-mini (fast, cheaper)",
        "expected_duration": "10-60s per package",
    },
    "google/gemini-3-flash-preview": {
        "name": "Gemini Flash 3",
        "provider": "google",
        "description": "Google Gemini 3 Flash (latest)",
        "expected_duration": "20-90s per package",
    },
    "google/gemini-2.5-flash-preview": {
        "name": "Gemini Flash 2.5",
        "provider": "google",
        "description": "Google Gemini 2.5 Flash (fast)",
        "expected_duration": "15-60s per package",
    },
    "claude-sonnet-4-5-20250929": {
        "name": "Claude Sonnet 4.5",
        "provider": "anthropic",
        "description": "Anthropic Claude Sonnet 4.5",
        "expected_duration": "25-100s per package",
    },
}


class RealWorldTester:
    def __init__(
        self,
        api_url: str,
        corpus_root: str,
        manifest: str,
        samples: int,
        simulation_mode: str,
        timeout: int,
        verbose: bool = False,
    ):
        self.api_url = api_url
        self.corpus_root = corpus_root
        self.manifest = manifest
        self.samples = samples
        self.simulation_mode = simulation_mode
        self.timeout = timeout
        self.verbose = verbose
        self.client = httpx.AsyncClient(timeout=timeout)

        # Results tracking
        self.test_results = []

    def log(self, message: str, level: str = "INFO"):
        """Print timestamped log message"""
        timestamp = datetime.now().strftime("%H:%M:%S")
        prefix = f"[{timestamp}]"
        if level == "INFO":
            print(f"{prefix} {message}")
        elif level == "SUCCESS":
            print(f"{prefix} ✅ {message}")
        elif level == "ERROR":
            print(f"{prefix} ❌ {message}")
        elif level == "WARNING":
            print(f"{prefix} ⚠️  {message}")

    async def check_api_health(self) -> bool:
        """Check if API is healthy"""
        try:
            response = await self.client.get(f"{self.api_url}/health")
            if response.status_code == 200:
                health = response.json()
                status = health.get("status")
                if status == "ok":
                    self.log("API is healthy")
                    return True
                else:
                    self.log(f"API status: {status}", "WARNING")
            else:
                self.log(f"API health check failed: HTTP {response.status_code}", "ERROR")
            return False
        except Exception as e:
            self.log(f"API health check error: {e}", "ERROR")
            return False

    def list_packages_in_manifest(self) -> List[str]:
        """List package IDs from manifest"""
        try:
            manifest_path = Path(self.manifest)
            if not manifest_path.exists():
                # Manifest path is for container, skip check on host
                return []

            with open(manifest_path) as f:
                packages = [line.strip() for line in f if line.strip()]

            self.log(f"Found {len(packages)} packages in manifest")
            if len(packages) < self.samples:
                self.log(
                    f"Warning: Requested {self.samples} samples but only {len(packages)} packages available", "WARNING"
                )

            return packages[: self.samples]
        except Exception as e:
            self.log(f"Error reading manifest: {e}", "ERROR")
            return []

    async def submit_task(
        self, model: str, samples: int = None, seed: int = None
    ) -> tuple[bool, str, str, Dict[str, Any]]:
        """Submit benchmark task via API

        Returns:
            (success, task_id, run_id, task_data)
        """
        if seed is None:
            seed = int(time.time())

        if samples is None:
            samples = self.samples

        config = {
            "corpus_root": self.corpus_root,
            "package_ids_file": self.manifest,
            "samples": samples,
            "agent": "real-openai-compatible",
            "model": model,
            "seed": seed,
            "simulation_mode": self.simulation_mode,
            "sender": "0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0",  # Dummy address for dry-run
        }

        payload = {
            "jsonrpc": "2.0",
            "id": f"test_{model}_{int(time.time())}",
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": f"msg_{model}_{int(time.time())}",
                    "role": "user",
                    "parts": [{"text": json.dumps({"config": config})}],
                }
            },
        }

        model_config = MODEL_CONFIGS.get(model, {"name": model})
        self.log(f"Submitting task with {model_config['name']}")
        if self.verbose:
            self.log(f"  Samples: {samples}, Seed: {seed}")
            self.log(f"  Mode: {self.simulation_mode}")

        try:
            response = await self.client.post(self.api_url, json=payload)

            if response.status_code != 200:
                error = response.text
                self.log(f"Task submission failed: HTTP {response.status_code}", "ERROR")
                self.log(f"  Error: {error}", "ERROR")
                return False, None, None, {}

            data = response.json()
            if "result" not in data:
                self.log("No result in API response", "ERROR")
                self.log(f"  Response: {json.dumps(data, indent=2)}", "ERROR")
                return False, None, None, {}

            result = data["result"]
            task_id = result.get("id")
            self.log(f"Task submitted: {task_id}", "SUCCESS")
            return True, task_id, None, result

        except Exception as e:
            self.log(f"Task submission error: {e}", "ERROR")
            return False, None, None, {}

    async def poll_task_completion(self, task_id: str, check_interval: int = 5) -> tuple[bool, Dict[str, Any]]:
        """Poll task until completion

        Returns:
            (success, task_data)
        """
        max_polls = self.timeout // check_interval

        for poll in range(max_polls):
            await asyncio.sleep(check_interval)

            try:
                response = await self.client.get(f"{self.api_url}/tasks/{task_id}/results")

                if response.status_code != 200:
                    self.log(f"Task query failed: HTTP {response.status_code}", "ERROR")
                    return False, {}

                data = response.json()
                status = data.get("status")
                run_id = data.get("run_id")
                duration = data.get("duration_seconds")

                # Print progress every 30 seconds
                if poll % (30 // check_interval) == 0:
                    elapsed = poll * check_interval
                    self.log(f"  [{elapsed}s] Status: {status}, Run ID: {run_id}")

                if status == "completed":
                    self.log(f"Task completed in {duration:.2f}s", "SUCCESS")
                    return True, data
                elif status == "failed":
                    error = data.get("error")
                    self.log(f"Task failed: {error}", "ERROR")
                    return False, data

            except Exception as e:
                self.log(f"Task query error: {e}", "ERROR")
                return False, {}

        self.log(f"Task timeout after {self.timeout}s", "ERROR")
        return False, {}

    async def test_model(self, model: str, description: str = "") -> Dict[str, Any]:
        """Run test with specified model"""
        model_config = MODEL_CONFIGS.get(model, {"name": model, "description": description})

        self.log("")
        self.log("=" * 70)
        self.log(f"TESTING MODEL: {model_config['name']}")
        self.log("=" * 70)
        if model_config.get("description"):
            self.log(f"Description: {model_config['description']}")
        if model_config.get("expected_duration"):
            self.log(f"Expected: {model_config['expected_duration']}")
        self.log("")

        start_time = time.time()

        # Submit task
        success, task_id, run_id, task_data = await self.submit_task(model)

        if not success:
            return {
                "model": model,
                "model_name": model_config["name"],
                "status": "failed",
                "error": "Task submission failed",
                "duration_seconds": time.time() - start_time,
            }

        # Poll for completion
        success, result_data = await self.poll_task_completion(task_id)

        end_time = time.time()
        total_duration = end_time - start_time

        if not success:
            return {
                "model": model,
                "model_name": model_config["name"],
                "status": "failed",
                "error": result_data.get("error", "Unknown error"),
                "duration_seconds": total_duration,
            }

        # Extract metrics
        metrics = result_data.get("metrics", {})
        run_id = result_data.get("run_id")

        return {
            "model": model,
            "model_name": model_config["name"],
            "task_id": task_id,
            "run_id": run_id,
            "status": "completed",
            "duration_seconds": total_duration,
            "metrics": metrics,
            "package_count": metrics.get("packages_total", 0),
            "avg_hit_rate": metrics.get("avg_hit_rate", 0),
            "errors": metrics.get("errors", 0),
            "prompt_tokens": metrics.get("total_prompt_tokens", 0),
            "completion_tokens": metrics.get("total_completion_tokens", 0),
            "started_at": int(start_time),
            "completed_at": int(end_time),
        }

    async def run_multiple_models(self, models: List[str]) -> List[Dict[str, Any]]:
        """Run tests for multiple models sequentially"""
        results = []

        for i, model in enumerate(models, 1):
            self.log(f"\n{'#' * 70}")
            self.log(f"# TEST {i}/{len(models)}: {model}")
            self.log(f"{'#' * 70}")

            result = await self.test_model(model)
            results.append(result)

            # Check if test passed
            if result["status"] == "completed":
                self.log(f"✅ {model} test PASSED", "SUCCESS")
            else:
                self.log(f"❌ {model} test FAILED", "ERROR")

        return results

    def print_summary(self, results: List[Dict[str, Any]]):
        """Print comprehensive test summary"""
        print("\n" + "=" * 70)
        print("REAL-WORLD TEST SUMMARY")
        print("=" * 70)

        # Calculate totals
        total_tasks = len(results)
        successful = sum(1 for r in results if r["status"] == "completed")
        failed = total_tasks - successful
        total_duration = sum(r["duration_seconds"] for r in results)
        total_packages = sum(r.get("package_count", 0) for r in results)
        total_errors = sum(r.get("errors", 0) for r in results)

        print(f"\nModels Tested: {total_tasks}")
        print(f"Successful: {successful}")
        print(f"Failed: {failed}")
        print(f"Total Packages: {total_packages}")
        print(f"Total Errors: {total_errors}")
        print(f"Total Duration: {total_duration:.2f}s ({total_duration / 60:.1f} min)")

        # Detailed results table
        print("\n" + "-" * 70)
        print(f"{'Model':<30} {'Status':<10} {'Packages':<10} {'Hit Rate':<10} {'Errors':<8} {'Duration'}")
        print("-" * 70)

        for result in results:
            model_name = result["model_name"]
            status = "✅" if result["status"] == "completed" else "❌"
            packages = result.get("package_count", 0)
            hit_rate = result.get("avg_hit_rate", 0)
            errors = result.get("errors", 0)
            duration = result["duration_seconds"]

            print(f"{model_name:<30} {status:<10} {packages:<10} {hit_rate:<10.2f} {errors:<8} {duration:.2f}s")

        print("-" * 70)

        # Detailed metrics
        print("\nDetailed Metrics:")
        for result in results:
            if result["status"] == "completed":
                print(f"\n📊 {result['model_name']}:")
                print(f"   Duration: {result['duration_seconds']:.2f}s")
                print(f"   Packages: {result['package_count']}")
                print(f"   Hit Rate: {result['avg_hit_rate']:.4f}")
                print(f"   Errors: {result['errors']}")
                print(f"   Prompt Tokens: {result['prompt_tokens']}")
                print(f"   Completion Tokens: {result['completion_tokens']}")

                if result["run_id"]:
                    print(f"   Run ID: {result['run_id']}")
                    print(f"   Results: /app/benchmark/results/a2a/{result['run_id']}.json")
                    print(f"   Logs: /app/benchmark/logs/{result['run_id']}/")

                # Token cost estimation (rough)
                prompt_cost = result["prompt_tokens"] * 2.5e-7  # $0.25 per 1M
                completion_cost = result["completion_tokens"] * 1e-6  # $1 per 1M
                estimated_cost = prompt_cost + completion_cost
                if estimated_cost > 0:
                    print(f"   Est. Cost: ${estimated_cost:.4f}")

        # Success/failure message
        print("\n" + "=" * 70)
        if successful == total_tasks:
            print("✅ ALL TESTS PASSED - System is production-ready!")
        else:
            print(f"⚠️  {failed}/{total_tasks} TESTS FAILED - Review errors above")
        print("=" * 70)

        # Save results to file
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        results_file = Path(f"real_world_test_results_{timestamp}.json")

        with open(results_file, "w") as f:
            json.dump(
                {
                    "test_configuration": {
                        "api_url": self.api_url,
                        "corpus_root": self.corpus_root,
                        "manifest": self.manifest,
                        "samples": self.samples,
                        "simulation_mode": self.simulation_mode,
                        "tested_at": datetime.now().isoformat(),
                    },
                    "results": results,
                    "summary": {
                        "total_tasks": total_tasks,
                        "successful": successful,
                        "failed": failed,
                        "total_duration_seconds": total_duration,
                        "total_packages": total_packages,
                        "total_errors": total_errors,
                    },
                },
                f,
                indent=2,
            )

        print(f"\n📄 Results saved to: {results_file}")


def parse_arguments():
    """Parse command line arguments"""
    parser = argparse.ArgumentParser(
        description="Real-World Production Test for Sui Move Interface Extractor Benchmark",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Test with 2 samples, 3 models, dry-run mode
  %(prog)s --samples 2 --models gpt-4o,google/gemini-3-flash-preview,\\
    google/gemini-2.5-flash-preview --simulation-mode dry-run

  # Test with real models (simulation mode live)
  %(prog)s --samples 5 --models gpt-4o-mini,google/gemini-3-flash-preview \\
    --simulation-mode live

  # Quick test with 1 sample, 2 models
  %(prog)s --samples 1 --models gpt-4o,claude-sonnet-4-5-20250929
        """,
    )

    parser.add_argument(
        "--api-url", type=str, default=DEFAULT_API_URL, help=f"API base URL (default: {DEFAULT_API_URL})"
    )

    parser.add_argument(
        "--corpus-root",
        type=str,
        default=DEFAULT_CORPUS_ROOT_HOST,
        help=f"Path to corpus (default: {DEFAULT_CORPUS_ROOT_HOST})",
    )

    parser.add_argument(
        "--manifest",
        type=str,
        default=DEFAULT_MANIFEST_HOST,
        help=f"Path to package manifest (default: {DEFAULT_MANIFEST_HOST})",
    )

    parser.add_argument(
        "--samples", type=int, default=DEFAULT_SAMPLES, help=f"Number of packages to test (default: {DEFAULT_SAMPLES})"
    )

    parser.add_argument("--models", type=str, required=True, help="Comma-separated list of models to test (required)")

    parser.add_argument(
        "--simulation-mode",
        type=str,
        choices=["dry-run", "live"],
        default="dry-run",
        help="Simulation mode (default: dry-run)",
    )

    parser.add_argument(
        "--timeout", type=int, default=DEFAULT_TIMEOUT, help=f"Per-task timeout in seconds (default: {DEFAULT_TIMEOUT})"
    )

    parser.add_argument("--verbose", action="store_true", help="Enable verbose output")

    return parser.parse_args()


async def main():
    """Main entry point"""
    args = parse_arguments()

    # Parse models list
    models = [m.strip() for m in args.models.split(",")]

    # Validate models
    invalid_models = [m for m in models if m not in MODEL_CONFIGS]
    if invalid_models:
        print(f"Warning: Unknown models: {', '.join(invalid_models)}")
        print(f"Valid models: {', '.join(MODEL_CONFIGS.keys())}")

    print("\n" + "=" * 70)
    print("REAL-WORLD PRODUCTION TEST")
    print("=" * 70)
    print("\nConfiguration:")
    print(f"  API URL: {args.api_url}")
    print(f"  Corpus: {args.corpus_root}")
    print(f"  Manifest: {args.manifest}")
    print(f"  Samples: {args.samples}")
    print(f"  Models: {', '.join([MODEL_CONFIGS.get(m, {'name': m})['name'] for m in models])}")
    print(f"  Simulation Mode: {args.simulation_mode}")
    print(f"  Timeout: {args.timeout}s per task")
    print("=" * 70)

    # Create tester
    tester = RealWorldTester(
        api_url=args.api_url,
        corpus_root=args.corpus_root,
        manifest=args.manifest,
        samples=args.samples,
        simulation_mode=args.simulation_mode,
        timeout=args.timeout,
        verbose=args.verbose,
    )

    # Check API health
    if not await tester.check_api_health():
        print("\n❌ API is not healthy. Exiting.")
        return 1

    # Use known test package
    packages = ["0x00"]
    print(f"\nTesting packages: {', '.join(packages)}")

    # Run tests for each model
    results = await tester.run_multiple_models(models)

    # Print summary
    tester.print_summary(results)

    # Return exit code based on results
    all_passed = all(r["status"] == "completed" for r in results)
    return 0 if all_passed else 1


if __name__ == "__main__":
    exit_code = asyncio.run(main())
    exit(exit_code)
