#!/usr/bin/env bash
# Replay a DeepBook cancel_order transaction via Walrus — no API keys needed.
#
# This involves BigVector dynamic field operations. Compare with:
#   examples/deepbook_orders.rs  (~350 lines of Rust with complex DF prefetching)
#
# NOTE: DeepBook BigVector operations access dynamic field child objects that
# may have been deleted since the checkpoint. The Walrus-first path fetches
# child objects via GraphQL/JSON-RPC (latest), which fails for deleted objects.
# Use the full gRPC archive path for reliable replay of these transactions.
#
# Usage:
#   bash examples_walrus/deepbook_orders.sh

set -euo pipefail

DIGEST="FbrMKMyzWm1K89qBZ45sYfCDsEtNmcnBdU9xiT7NKvmR"
CHECKPOINT=235835535

echo "=== DeepBook Cancel Order Replay ==="
echo "Digest:     $DIGEST"
echo "Checkpoint: $CHECKPOINT"
echo ""

cargo run --bin sui-sandbox --features walrus -- \
  replay "$DIGEST" \
  --checkpoint "$CHECKPOINT" \
  --compare \
  --verbose
