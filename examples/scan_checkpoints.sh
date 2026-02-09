#!/bin/bash
# scan_checkpoints.sh — Scan and replay the latest Sui checkpoints (zero setup)
#
# Fetches the N most recent checkpoints from Walrus (decentralized storage),
# replays every PTB transaction locally, and prints a summary with success rates
# and per-tag breakdown.
#
# Usage:
#   ./examples/scan_checkpoints.sh         # Latest 5 checkpoints (default)
#   ./examples/scan_checkpoints.sh 10      # Latest 10 checkpoints
#   ./examples/scan_checkpoints.sh 1       # Just the tip checkpoint
#
# No API keys, no configuration, no gRPC endpoint needed.

set -euo pipefail

COUNT="${1:-5}"
BINARY="${SUI_SANDBOX_BIN:-cargo run --bin sui-sandbox --features walrus --}"

echo "=== Scanning Latest $COUNT Checkpoint(s) ==="
echo "Source: Walrus (decentralized, zero setup)"
echo ""

export SUI_REPLAY_PROGRESS=1

$BINARY replay "*" --source walrus --latest "$COUNT" --compare
