#!/usr/bin/env python3
"""Example 4: generic two-step flow via native Python bindings.

Core usage is intentionally compact:
  ctx = sui_sandbox.prepare_package_context("0x...")
  out = sui_sandbox.replay_transaction("<DIGEST>", checkpoint=...)
"""

from __future__ import annotations

import argparse

import sui_sandbox


def main() -> None:
    parser = argparse.ArgumentParser(description="Generic package->replay native flow")
    parser.add_argument("--package-id", default="0x2", help="Root package id")
    parser.add_argument("--digest", default=None, help="Transaction digest")
    parser.add_argument("--checkpoint", type=int, default=None, help="Optional checkpoint")
    parser.add_argument(
        "--state-file",
        default=None,
        help="Optional replay-state JSON (enables local deterministic replay)",
    )
    parser.add_argument(
        "--output-context",
        default=None,
        help="Optional path to write context JSON",
    )
    args = parser.parse_args()
    if args.digest is None and args.state_file is None:
        parser.error("provide --digest or --state-file")

    context = sui_sandbox.prepare_package_context(
        args.package_id,
        output_path=args.output_context,
    )
    result = sui_sandbox.replay_transaction(
        args.digest,
        checkpoint=args.checkpoint,
        state_file=args.state_file,
    )

    print("=== Generic Flow (Native Python) ===")
    print(f"context_packages: {context.get('count')}")
    print(f"success:          {result.get('local_success')}")
    print(f"gas_used:         {result.get('gas_used')}")
    if result.get("local_error"):
        print(f"local_error:      {result.get('local_error')}")


if __name__ == "__main__":
    main()
