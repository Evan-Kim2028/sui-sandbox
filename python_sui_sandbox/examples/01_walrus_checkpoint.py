#!/usr/bin/env python3
import sui_sandbox

cp = sui_sandbox.get_latest_checkpoint()
summary = sui_sandbox.get_checkpoint(cp)
print({
    "checkpoint": summary.get("checkpoint"),
    "epoch": summary.get("epoch"),
    "transaction_count": summary.get("transaction_count"),
})
