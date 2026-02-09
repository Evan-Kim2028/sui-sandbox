#!/usr/bin/env bash
# Analyze a transaction's replay state via Walrus — no API keys needed.
#
# The `analyze replay` command introspects the replay state without executing:
# - Transaction structure (commands, inputs, sender)
# - Object and package counts
# - Missing inputs/packages
# - Readiness suggestions
#
# Usage:
#   bash examples_walrus/analyze_replay.sh

set -euo pipefail

DIGEST="D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva"
CHECKPOINT=235248874

echo "=== Analyze Replay State (DeepBook Flash Loan) ==="
echo "Digest:     $DIGEST"
echo "Checkpoint: $CHECKPOINT"
echo ""

cargo run --bin sui-sandbox --features walrus,analysis -- \
  analyze replay "$DIGEST" \
  --checkpoint "$CHECKPOINT" \
  --verbose \
  --json
