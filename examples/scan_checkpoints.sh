#!/bin/bash
# scan_checkpoints.sh — Core external flow for Walrus checkpoint streaming
#
# Replays PTB transactions from Walrus checkpoints (zero setup).
# This is the fastest way to validate replay behavior against recent mainnet activity.
#
# Usage:
#   ./examples/scan_checkpoints.sh                    # Latest 5 checkpoints
#   ./examples/scan_checkpoints.sh 10                # Latest 10 checkpoints
#   ./examples/scan_checkpoints.sh --range 100..110  # Explicit checkpoint range
#   ./examples/scan_checkpoints.sh --help            # Show help

set -euo pipefail

usage() {
    cat <<'EOF'
Core example: stream replay across Walrus checkpoints.

Usage:
  ./examples/scan_checkpoints.sh [COUNT]
  ./examples/scan_checkpoints.sh --range START..END
  ./examples/scan_checkpoints.sh --help

Examples:
  ./examples/scan_checkpoints.sh
  ./examples/scan_checkpoints.sh 10
  ./examples/scan_checkpoints.sh --range 239615920..239615926

Notes:
  - No API keys and no endpoint config required.
  - Set SUI_SANDBOX_BIN to use a prebuilt binary for faster runs, e.g.:
      export SUI_SANDBOX_BIN=./target/release/sui-sandbox
EOF
}

COUNT="5"
RANGE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            usage
            exit 0
            ;;
        --range)
            RANGE="${2:-}"
            if [[ -z "$RANGE" ]]; then
                echo "ERROR: --range requires START..END" >&2
                exit 1
            fi
            shift 2
            ;;
        *)
            if [[ "$1" =~ ^[0-9]+$ ]]; then
                COUNT="$1"
                shift
            else
                echo "ERROR: unknown argument '$1'" >&2
                usage
                exit 1
            fi
            ;;
    esac
done

if [[ -x "./target/release/sui-sandbox" ]]; then
    DEFAULT_BIN="./target/release/sui-sandbox"
else
    DEFAULT_BIN="cargo run --bin sui-sandbox --features walrus --"
fi
BINARY="${SUI_SANDBOX_BIN:-$DEFAULT_BIN}"

export SUI_REPLAY_PROGRESS=1

echo "=== Walrus Checkpoint Stream Replay ==="
echo "Source: Walrus (decentralized, zero setup)"
if [[ -n "$RANGE" ]]; then
    echo "Mode:   explicit checkpoint range ($RANGE)"
else
    echo "Mode:   latest checkpoint stream window ($COUNT)"
fi
echo ""

if [[ -n "$RANGE" ]]; then
    $BINARY replay "*" --source walrus --checkpoint "$RANGE" --compare
else
    $BINARY replay "*" --source walrus --latest "$COUNT" --compare
fi

echo ""
echo "Next steps:"
echo "  1) Drill into a specific checkpoint range:"
echo "     sui-sandbox replay '*' --source walrus --checkpoint 239615920..239615926 --compare"
echo "  2) Drill into a specific transaction:"
echo "     sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare"
