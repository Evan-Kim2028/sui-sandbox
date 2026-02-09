#!/bin/bash
# replay.sh — Transaction replay via any data source
#
# Replays a real mainnet transaction locally. Supports multiple data sources:
#
#   walrus  — Zero setup, no API keys. Uses Walrus decentralized checkpoint storage.
#   grpc    — Standard gRPC endpoint. Requires SUI_GRPC_ENDPOINT (+ optional SUI_GRPC_API_KEY).
#   hybrid  — Snowflake/igloo-mcp + gRPC (one example of a custom pipeline).
#   json    — Load replay state from a JSON file. Bring your own data from any source.
#
# Usage:
#   ./examples/replay.sh                                            # Walrus default (zero setup)
#   ./examples/replay.sh --source walrus <DIGEST> <CHECKPOINT>      # Walrus with custom tx
#   ./examples/replay.sh --source walrus '*' 100..200               # Walrus range (all txs)
#   ./examples/replay.sh --latest 5                                 # Latest 5 checkpoints
#   ./examples/replay.sh --source grpc <DIGEST>                     # gRPC
#   ./examples/replay.sh --source json <STATE_FILE>                 # JSON state file
#
# Export state from any source, then replay offline:
#   sui-sandbox replay <DIGEST> --source grpc --export-state state.json
#   ./examples/replay.sh --source json state.json
#
# Environment variables (source-specific):
#   SUI_GRPC_ENDPOINT    — gRPC endpoint URL          (grpc/hybrid)
#   SUI_GRPC_API_KEY     — gRPC API key               (grpc/hybrid, optional)

set -euo pipefail

BINARY="${SUI_SANDBOX_BIN:-cargo run --bin sui-sandbox --}"

# --- Parse args ---

SOURCE="walrus"
DIGEST=""
CHECKPOINT=""
JSON_FILE=""
LATEST=""
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --source)
            SOURCE="$2"
            shift 2
            ;;
        --latest)
            LATEST="$2"
            shift 2
            ;;
        --verbose|-v)
            EXTRA_ARGS+=("--verbose")
            shift
            ;;
        --*)
            EXTRA_ARGS+=("$1")
            shift
            ;;
        *)
            if [ "$SOURCE" = "json" ] && [ -z "$JSON_FILE" ]; then
                JSON_FILE="$1"
            elif [ -z "$DIGEST" ]; then
                DIGEST="$1"
            elif [ -z "$CHECKPOINT" ]; then
                CHECKPOINT="$1"
            fi
            shift
            ;;
    esac
done

# --- Handle --latest shortcut ---

if [ -n "$LATEST" ]; then
    echo ""
    echo "=== Scan Latest $LATEST Checkpoints ==="
    echo "Source:     walrus (auto)"
    echo ""
    $BINARY replay "*" --source walrus --latest "$LATEST" --compare "${EXTRA_ARGS[@]}"
    exit $?
fi

# --- Defaults per source ---

case "$SOURCE" in
    walrus)
        if [ -z "$DIGEST" ]; then
            # Known-good Cetus swap: pool_script_v2::swap_b2a (3 commands, direct swap)
            DIGEST="At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2"
            CHECKPOINT="239615926"
            echo "Using known-good Cetus swap (pass <DIGEST> <CHECKPOINT> to override)"
        fi
        if [ -z "$CHECKPOINT" ]; then
            echo "ERROR: Walrus source requires a checkpoint number (or range like 100..200)."
            echo "Usage: $0 --source walrus <DIGEST> <CHECKPOINT>"
            exit 1
        fi
        ;;
    grpc)
        if [ -z "$DIGEST" ]; then
            echo "ERROR: gRPC source requires a transaction digest."
            echo "Usage: $0 --source grpc <DIGEST>"
            exit 1
        fi
        if [ -z "${SUI_GRPC_ENDPOINT:-}" ]; then
            echo "WARNING: SUI_GRPC_ENDPOINT not set, using default archive endpoint."
        fi
        ;;
    hybrid)
        if [ -z "$DIGEST" ]; then
            echo "ERROR: Hybrid source requires a transaction digest."
            echo "Usage: $0 --source hybrid <DIGEST>"
            exit 1
        fi
        ;;
    json)
        if [ -z "$JSON_FILE" ]; then
            echo "ERROR: JSON source requires a state file path."
            echo "Usage: $0 --source json <STATE_FILE>"
            echo ""
            echo "Generate a state file with --export-state:"
            echo "  sui-sandbox replay <DIGEST> --source grpc --export-state state.json"
            exit 1
        fi
        if [ ! -f "$JSON_FILE" ]; then
            echo "ERROR: State file not found: $JSON_FILE"
            exit 1
        fi
        # Extract digest from JSON file
        DIGEST=$(python3 -c "import json; print(json.load(open('$JSON_FILE'))['transaction']['digest'])" 2>/dev/null || echo "unknown")
        ;;
    *)
        echo "ERROR: Unknown source '$SOURCE'. Use: walrus, grpc, hybrid, or json."
        exit 1
        ;;
esac

# --- Display ---

echo ""
echo "=== Transaction Replay ==="
echo "Digest:     $DIGEST"
echo "Source:     $SOURCE"
[ -n "$CHECKPOINT" ] && echo "Checkpoint: $CHECKPOINT"
[ -n "$JSON_FILE" ] && echo "State file: $JSON_FILE"
echo ""

# --- Build CLI args ---

if [ "$SOURCE" = "json" ]; then
    CLI_ARGS=(replay "$DIGEST" --state-json "$JSON_FILE")
else
    CLI_ARGS=(replay "$DIGEST" --source "$SOURCE" --compare)
    if [ "$SOURCE" = "walrus" ]; then
        CLI_ARGS+=(--checkpoint "$CHECKPOINT")
    fi
fi

CLI_ARGS+=("${EXTRA_ARGS[@]}")

# --- Run ---

$BINARY "${CLI_ARGS[@]}"
