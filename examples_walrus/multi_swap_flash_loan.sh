#!/usr/bin/env bash
# Replay a multi-DEX flash loan arbitrage via Walrus — no API keys needed.
#
# This transaction routes through Kriya -> USDC -> SCA -> SUI across
# multiple DEX protocols. Compare with:
#   examples/multi_swap_flash_loan.rs  (~300 lines of Rust, requires gRPC key)
#
# Usage:
#   bash examples_walrus/multi_swap_flash_loan.sh

set -euo pipefail

DIGEST="63fPrufC6iYHdNzG7mXscaKkqTaYH8h4RQHuiUfUCXqz"
CHECKPOINT=237204492

echo "=== Multi-DEX Flash Loan Arbitrage Replay ==="
echo "Digest:     $DIGEST"
echo "Checkpoint: $CHECKPOINT"
echo ""

cargo run --bin sui-sandbox --features walrus -- \
  replay "$DIGEST" \
  --checkpoint "$CHECKPOINT" \
  --compare \
  --verbose
