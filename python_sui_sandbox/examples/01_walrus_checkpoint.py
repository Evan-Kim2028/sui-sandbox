#!/usr/bin/env python3
"""Fetch a latest-checkpoint summary from Sui.

Example output (run on 2026-02-16):
{'checkpoint': 239615933, 'epoch': 1022, 'transaction_count': 19}
"""

import sui_sandbox

# Query the latest checkpoint number, then load its summary.
cp = sui_sandbox.get_latest_checkpoint()
summary = sui_sandbox.get_checkpoint(cp)

# Print a compact snapshot that is easy to inspect in logs.
print({
    "checkpoint": summary.get("checkpoint"),
    "epoch": summary.get("epoch"),
    "transaction_count": summary.get("transaction_count"),
})
