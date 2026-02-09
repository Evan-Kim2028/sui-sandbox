#!/usr/bin/env bash
# Zero-setup transaction replay via Walrus checkpoint data.
# No API keys, no gRPC, no GraphQL — just Walrus HTTP + local VM.
#
# This is the simplest possible replay example. Compare with:
#   examples/deepbook_replay.rs  (316 lines of Rust, requires gRPC API key)
#
# Usage:
#   bash examples_walrus/walrus_replay.sh
#   # or after `cargo install --path . --features walrus`:
#   sui-sandbox replay D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva \
#     --checkpoint 235248874 --compare

set -euo pipefail

DIGEST="D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva"
CHECKPOINT=235248874

echo "=== DeepBook Flash Loan Replay (expected: FAILED on-chain) ==="
echo "Digest:     $DIGEST"
echo "Checkpoint: $CHECKPOINT"
echo ""

cargo run --bin sui-sandbox --features walrus -- \
  replay "$DIGEST" \
  --checkpoint "$CHECKPOINT" \
  --compare \
  --verbose \
  --json
