#!/usr/bin/env bash
# Replay a Cetus LEIA/SUI swap via Walrus — no API keys needed.
#
# Replaces: examples/cetus_swap.rs (complex Rust example with MM2 analysis,
#           child fetchers, gRPC/GraphQL setup, ~250 lines)
#
# NOTE: This transaction accesses Cetus skip_list dynamic fields that may
# have been deleted/restructured since the checkpoint. If the swap fails with
# E_FIELD_DOES_NOT_EXIST in dynamic_field, the child objects no longer exist
# at their current version. Use the full gRPC path for reliable replay of
# older DEX transactions with heavy dynamic field access.
#
# Usage:
#   bash examples_walrus/cetus_swap.sh

set -euo pipefail

DIGEST="7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp"
CHECKPOINT=234219761

echo "=== Cetus LEIA/SUI Swap Replay ==="
echo "Digest:     $DIGEST"
echo "Checkpoint: $CHECKPOINT"
echo ""

cargo run --bin sui-sandbox --features walrus -- \
  replay "$DIGEST" \
  --checkpoint "$CHECKPOINT" \
  --compare \
  --verbose
