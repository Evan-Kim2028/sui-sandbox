#!/usr/bin/env python3
import json
import sys
import argparse
from pathlib import Path

def main():
    parser = argparse.ArgumentParser(description="Comprehensive Phase II Benchmark Analysis")
    parser.add_argument("file", type=Path, help="Path to the result JSON file")
    args = parser.parse_args()

    if not args.file.exists():
        print(f"Error: File {args.file} not found")
        sys.exit(1)

    try:
        with open(args.file) as f:
            data = json.load(f)
    except Exception as e:
        print(f"Error reading JSON: {e}")
        sys.exit(1)

    agg = data.get("aggregate", {})
    pkgs = data.get("packages", [])
    total = len(pkgs)
    
    if total == 0:
        print("No package results found in file.")
        return

    # Metrics calculation
    reasoning_ok = sum(1 for p in pkgs if p.get("ptb_parse_ok"))
    dry_run_ok = sum(1 for p in pkgs if p.get("dry_run_ok"))
    hit_rate = agg.get("avg_hit_rate", 0.0)
    
    # Timing
    total_time = sum(p.get("elapsed_seconds", 0) for p in pkgs)
    
    # Category Breakdown (Simple)
    timeouts = sum(1 for p in pkgs if p.get("timed_out"))
    errors = sum(1 for p in pkgs if p.get("error") and not p.get("timed_out"))
    
    print("\n" + "="*60)
    print(f"PHASE II EVALUATION: {data.get('agent', 'unknown')}")
    print("="*60)
    print(f"{ 'Metric':<25} | { 'Value':<20}")
    print("-" * 60)
    print(f"{ 'Total Packages':<25} | {total}")
    print(f"{ 'Avg Hit Rate':<25} | {hit_rate:.2%}")
    print(f"{ 'Dry Run Success Rate':<25} | {dry_run_ok/total:.2%} ({dry_run_ok}/{total})")
    print(f"{ 'Reasoning/Parse OK':<25} | {reasoning_ok/total:.2%} ({reasoning_ok}/{total})")
    print(f"{ 'Timeouts':<25} | {timeouts}")
    print(f"{ 'Harness Errors':<25} | {errors}")
    print("-" * 60)
    print(f"{ 'Total Prompt Tokens':<25} | {agg.get('total_prompt_tokens', 0):,}")
    print(f"{ 'Total Completion Tokens':<25} | {agg.get('total_completion_tokens', 0):,}")
    print(f"{ 'Total Time':<25} | {total_time:.1f}s")
    print(f"{ 'Average Time/Pkg':<25} | {total_time/total:.1f}s")
    print("="*60 + "\n")

if __name__ == "__main__":
    main()
