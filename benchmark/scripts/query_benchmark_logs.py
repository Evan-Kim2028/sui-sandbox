#!/usr/bin/env python3
"""
Query and analyze benchmark run logs.

Provides searchable access to:
- Run metadata (model, duration, tokens, cost)
- Package-level results
- Error analysis
- Performance trends over time
"""

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path
from typing import Any


class BenchmarkLogQuery:
    def __init__(self, logs_dir: Path, results_dir: Path):
        self.logs_dir = logs_dir
        self.results_dir = results_dir

    def list_runs(self, model_filter: str | None = None, limit: int = 20) -> list[dict[str, Any]]:
        """List all benchmark runs with metadata."""
        runs = []

        # Scan results directory
        for result_file in sorted(self.results_dir.glob("*.json"), reverse=True):
            try:
                data = json.loads(result_file.read_text())

                # Extract metadata
                run_id = result_file.stem
                aggregate = data.get("aggregate", {})

                # Check for run_metadata in logs
                log_metadata_path = self.logs_dir / run_id / "run_metadata.json"
                metadata = {}
                if log_metadata_path.exists():
                    metadata = json.loads(log_metadata_path.read_text())

                model = metadata.get("model") or "unknown"

                # Filter by model if specified
                if model_filter and model_filter.lower() not in model.lower():
                    continue

                run_info = {
                    "run_id": run_id,
                    "model": model,
                    "timestamp": metadata.get("started_at_unix_seconds"),
                    "agent": metadata.get("agent"),
                    "simulation_mode": metadata.get("simulation_mode"),
                    "packages_total": len(data.get("packages", [])),
                    "avg_hit_rate": aggregate.get("avg_hit_rate"),
                    "errors": aggregate.get("errors", 0),
                    "total_prompt_tokens": aggregate.get("total_prompt_tokens", 0),
                    "total_completion_tokens": aggregate.get("total_completion_tokens", 0),
                    "result_file": str(result_file),
                }

                runs.append(run_info)

                if len(runs) >= limit:
                    break

            except Exception as e:
                print(f"Warning: Failed to parse {result_file}: {e}", file=sys.stderr)
                continue

        return runs

    def get_run_details(self, run_id: str) -> dict[str, Any]:
        """Get detailed information about a specific run."""
        result_file = self.results_dir / f"{run_id}.json"
        if not result_file.exists():
            raise FileNotFoundError(f"Run {run_id} not found")

        data = json.loads(result_file.read_text())

        # Load metadata
        metadata_path = self.logs_dir / run_id / "run_metadata.json"
        metadata = {}
        if metadata_path.exists():
            metadata = json.loads(metadata_path.read_text())

        # Load events log
        events_path = self.logs_dir / run_id / "events.jsonl"
        events = []
        if events_path.exists():
            for line in events_path.read_text().splitlines():
                if line.strip():
                    events.append(json.loads(line))

        return {
            "run_id": run_id,
            "metadata": metadata,
            "results": data,
            "events": events,
        }

    def analyze_package_performance(self, run_id: str) -> dict[str, Any]:
        """Analyze per-package performance metrics."""
        details = self.get_run_details(run_id)
        packages = details["results"].get("packages", [])

        analysis = {
            "total_packages": len(packages),
            "successful": 0,
            "failed": 0,
            "timed_out": 0,
            "per_package_stats": [],
        }

        for pkg in packages:
            pkg_id = pkg.get("package_id")
            error = pkg.get("error")
            timed_out = pkg.get("timed_out", False)
            elapsed = pkg.get("elapsed_seconds", 0)
            score = pkg.get("score", {})

            if error or timed_out:
                analysis["failed"] += 1
                if timed_out:
                    analysis["timed_out"] += 1
            else:
                analysis["successful"] += 1

            analysis["per_package_stats"].append(
                {
                    "package_id": pkg_id,
                    "elapsed_seconds": elapsed,
                    "hit_rate": score.get("hit_rate"),
                    "created_hits": score.get("created_hits"),
                    "targets": score.get("targets"),
                    "error": error,
                    "timed_out": timed_out,
                }
            )

        return analysis

    def compare_models(self, run_ids: list[str]) -> dict[str, Any]:
        """Compare performance across multiple runs (different models)."""
        comparison = {
            "runs": [],
            "summary": {},
        }

        for run_id in run_ids:
            try:
                details = self.get_run_details(run_id)
                metadata = details["metadata"]
                aggregate = details["results"].get("aggregate", {})

                comparison["runs"].append(
                    {
                        "run_id": run_id,
                        "model": metadata.get("model"),
                        "avg_hit_rate": aggregate.get("avg_hit_rate"),
                        "errors": aggregate.get("errors", 0),
                        "total_prompt_tokens": aggregate.get("total_prompt_tokens", 0),
                        "total_completion_tokens": aggregate.get("total_completion_tokens", 0),
                        "packages": len(details["results"].get("packages", [])),
                    }
                )
            except Exception as e:
                print(f"Warning: Failed to load run {run_id}: {e}", file=sys.stderr)

        return comparison

    def estimate_cost(self, run_id: str, pricing: dict[str, Any] | None = None) -> dict[str, Any]:
        """Estimate cost for a run based on token usage."""
        details = self.get_run_details(run_id)
        aggregate = details["results"].get("aggregate", {})

        prompt_tokens = aggregate.get("total_prompt_tokens", 0)
        completion_tokens = aggregate.get("total_completion_tokens", 0)

        # Default pricing (update based on actual model)
        if pricing is None:
            pricing = {
                "input_per_1k": 0.15 / 1000,  # $0.15 per 1M tokens
                "output_per_1k": 0.60 / 1000,  # $0.60 per 1M tokens
            }

        input_cost = prompt_tokens * pricing["input_per_1k"]
        output_cost = completion_tokens * pricing["output_per_1k"]
        total_cost = input_cost + output_cost

        return {
            "run_id": run_id,
            "model": details["metadata"].get("model"),
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
            "input_cost_usd": input_cost,
            "output_cost_usd": output_cost,
            "total_cost_usd": total_cost,
        }


def main():
    parser = argparse.ArgumentParser(description="Query benchmark logs")
    parser.add_argument(
        "--logs-dir",
        type=Path,
        default=Path("logs"),
        help="Logs directory",
    )
    parser.add_argument(
        "--results-dir",
        type=Path,
        default=Path("results/a2a"),
        help="Results directory",
    )

    subparsers = parser.add_subparsers(dest="command", required=True)

    # List runs
    list_parser = subparsers.add_parser("list", help="List all runs")
    list_parser.add_argument("--model", help="Filter by model")
    list_parser.add_argument("--limit", type=int, default=20, help="Max results")

    # Show run details
    show_parser = subparsers.add_parser("show", help="Show run details")
    show_parser.add_argument("run_id", help="Run ID")

    # Analyze package performance
    analyze_parser = subparsers.add_parser("analyze", help="Analyze package performance")
    analyze_parser.add_argument("run_id", help="Run ID")

    # Compare models
    compare_parser = subparsers.add_parser("compare", help="Compare multiple runs")
    compare_parser.add_argument("run_ids", nargs="+", help="Run IDs to compare")

    # Estimate cost
    cost_parser = subparsers.add_parser("cost", help="Estimate run cost")
    cost_parser.add_argument("run_id", help="Run ID")

    args = parser.parse_args()

    query = BenchmarkLogQuery(args.logs_dir, args.results_dir)

    if args.command == "list":
        runs = query.list_runs(model_filter=args.model, limit=args.limit)

        if not runs:
            print("No runs found.")
            return

        # Print table
        header = (
            f"\n{'Run ID':<30} {'Model':<25} {'Packages':<10} {'Hit Rate':<10} "
            f"{'Errors':<8} {'Tokens':<12} {'Timestamp'}"
        )
        print(header)
        print("-" * 120)

        for run in runs:
            run_id = run["run_id"][:29]
            model = (run["model"] or "unknown")[:24]
            packages = run["packages_total"]
            hit_rate = f"{run['avg_hit_rate']:.2%}" if run["avg_hit_rate"] is not None else "N/A"
            errors = run["errors"]
            total_tokens = run["total_prompt_tokens"] + run["total_completion_tokens"]
            tokens_str = f"{total_tokens:,}" if total_tokens else "0"

            timestamp = "N/A"
            if run["timestamp"]:
                timestamp = datetime.fromtimestamp(run["timestamp"]).strftime("%Y-%m-%d %H:%M")

            print(f"{run_id:<30} {model:<25} {packages:<10} {hit_rate:<10} {errors:<8} {tokens_str:<12} {timestamp}")

    elif args.command == "show":
        details = query.get_run_details(args.run_id)
        print(json.dumps(details, indent=2))

    elif args.command == "analyze":
        analysis = query.analyze_package_performance(args.run_id)
        print(json.dumps(analysis, indent=2))

    elif args.command == "compare":
        comparison = query.compare_models(args.run_ids)
        print(json.dumps(comparison, indent=2))

    elif args.command == "cost":
        cost_info = query.estimate_cost(args.run_id)
        print(json.dumps(cost_info, indent=2))


if __name__ == "__main__":
    main()
