#!/usr/bin/env python3
"""Example 3: context-first replay via native Python bindings."""

from __future__ import annotations

import argparse

import sui_sandbox


def main() -> None:
    parser = argparse.ArgumentParser(description="Context prepare + replay in one call")
    parser.add_argument("--package-id", default="0x2", help="Root package id")
    parser.add_argument("--digest", default=None, help="Transaction digest")
    parser.add_argument("--checkpoint", type=int, default=None, help="Optional checkpoint")
    parser.add_argument(
        "--discover-latest",
        type=int,
        default=0,
        help="Auto-discover digest/checkpoint from latest N checkpoints",
    )
    parser.add_argument(
        "--state-file",
        default=None,
        help="Optional replay-state JSON for deterministic local replay",
    )
    parser.add_argument(
        "--analyze-only",
        action="store_true",
        help="Hydration-only mode (skip VM execution)",
    )
    args = parser.parse_args()
    if args.digest is None and args.state_file is None and args.discover_latest <= 0:
        parser.error("provide --digest or --state-file (or use --discover-latest)")

    result = sui_sandbox.context_run(
        args.package_id,
        args.digest,
        checkpoint=args.checkpoint,
        discover_latest=args.discover_latest if args.digest is None else None,
        state_file=args.state_file,
        analyze_only=args.analyze_only,
    )

    print("=== Context Replay (Native Python) ===")
    print(f"digest:      {result.get('digest') or args.digest}")
    print(f"checkpoint:  {result.get('checkpoint') or args.checkpoint}")
    print(f"success:     {result.get('local_success')}")
    print(f"gas_used:    {result.get('gas_used')}")
    if result.get("local_error"):
        print(f"local_error: {result.get('local_error')}")


if __name__ == "__main__":
    main()
