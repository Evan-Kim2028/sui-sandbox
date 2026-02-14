#!/usr/bin/env python3
"""Example 3: analyze replay hydration (no VM execution) via Walrus."""

from __future__ import annotations

import argparse

import sui_sandbox

DEFAULT_DIGEST = "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2"
DEFAULT_CHECKPOINT = 239615926


def main() -> None:
    parser = argparse.ArgumentParser(description="Analyze replay state for a transaction")
    parser.add_argument("--digest", default=DEFAULT_DIGEST, help="Transaction digest")
    parser.add_argument(
        "--checkpoint",
        type=int,
        default=DEFAULT_CHECKPOINT,
        help="Checkpoint containing digest (default: known-good sample)",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Enable verbose replay analysis logging",
    )
    args = parser.parse_args()

    analysis = sui_sandbox.replay(
        digest=args.digest,
        checkpoint=args.checkpoint,
        analyze_only=True,
        verbose=args.verbose,
    )

    print("=== Replay Analyze Summary ===")
    print(f"Digest:              {args.digest}")
    print(f"Checkpoint:          {args.checkpoint}")
    print(f"Sender:              {analysis.get('sender')}")
    print(f"Commands:            {analysis.get('commands')}")
    print(f"Inputs:              {analysis.get('inputs')}")
    print(f"Objects:             {analysis.get('objects')}")
    print(f"Packages:            {analysis.get('packages')}")
    print(f"Modules:             {analysis.get('modules')}")
    print(f"Epoch:               {analysis.get('epoch')}")
    print(f"Protocol version:    {analysis.get('protocol_version')}")

    command_summaries = analysis.get("command_summaries", [])
    if command_summaries:
        print("\nCommand summaries:")
        for idx, cmd in enumerate(command_summaries, start=1):
            target = cmd.get("target")
            target_part = f" -> {target}" if target else ""
            print(
                f"{idx:2d}. {cmd.get('kind')}{target_part} "
                f"(type_args={cmd.get('type_args')}, args={cmd.get('args')})"
            )

    input_summary = analysis.get("input_summary", {})
    if input_summary:
        print("\nInput summary:")
        for key, value in input_summary.items():
            print(f"- {key}: {value}")


if __name__ == "__main__":
    main()
