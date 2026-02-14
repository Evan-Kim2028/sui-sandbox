#!/usr/bin/env python3
"""Example 1: inspect Walrus checkpoint summary via Python bindings."""

from __future__ import annotations

import argparse

import sui_sandbox


def main() -> None:
    parser = argparse.ArgumentParser(description="Inspect a Walrus checkpoint summary")
    parser.add_argument(
        "--checkpoint",
        type=int,
        default=None,
        help="Checkpoint number (default: latest archived checkpoint)",
    )
    parser.add_argument(
        "--tx-limit",
        type=int,
        default=5,
        help="How many transactions to print (default: 5)",
    )
    args = parser.parse_args()

    checkpoint = args.checkpoint
    if checkpoint is None:
        checkpoint = sui_sandbox.get_latest_checkpoint()

    data = sui_sandbox.get_checkpoint(checkpoint)

    print("=== Walrus Checkpoint Summary ===")
    print(f"Checkpoint:          {data.get('checkpoint')}")
    print(f"Epoch:               {data.get('epoch')}")
    print(f"Timestamp (ms):      {data.get('timestamp_ms')}")
    print(f"Transaction count:   {data.get('transaction_count')}")
    print(f"Object versions:     {data.get('object_versions_count')}")

    txs = data.get("transactions", [])
    if not txs:
        print("\nNo transactions in this checkpoint.")
        return

    print(f"\nTop {min(args.tx_limit, len(txs))} transactions:")
    for idx, tx in enumerate(txs[: args.tx_limit], start=1):
        digest = tx.get("digest", "")
        sender = tx.get("sender", "")
        print(
            f"{idx:2d}. digest={digest[:20]}... "
            f"sender={sender[:20]}... "
            f"cmds={tx.get('commands')} "
            f"in={tx.get('input_objects')} "
            f"out={tx.get('output_objects')}"
        )


if __name__ == "__main__":
    main()
