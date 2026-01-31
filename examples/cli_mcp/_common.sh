#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
SUI_SANDBOX="${PROJECT_ROOT}/target/release/sui-sandbox"

load_env() {
  if [ -f "${PROJECT_ROOT}/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "${PROJECT_ROOT}/.env"
    set +a
  fi
}

ensure_binary() {
  if [ ! -f "$SUI_SANDBOX" ]; then
    echo "Building sui-sandbox CLI..."
    cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
  fi
}

init_context() {
  load_env
  ensure_binary

  RPC_GRPC_URL="${MCP_GRPC_URL:-${SUI_GRPC_ENDPOINT:-https://fullnode.mainnet.sui.io:443}}"
  RPC_GRAPHQL_URL="${MCP_GRAPHQL_URL:-${SUI_GRAPHQL_ENDPOINT:-https://graphql.mainnet.sui.io/graphql}}"

  if [ "${USE_PUBLIC_GRPC:-}" = "1" ]; then
    RPC_GRPC_URL="https://fullnode.mainnet.sui.io:443"
  fi

  STATE_DIR="/tmp/sui-sandbox-cli-mcp-examples"
  mkdir -p "$STATE_DIR"

  SCRIPT_NAME="$(basename "$0" .sh)"
  STATE_FILE="${STATE_DIR}/${SCRIPT_NAME}.json"
  LEGACY_STATE_FILE="${STATE_DIR}/${SCRIPT_NAME}.bin"

  OUT_DIR="${STATE_DIR}/outputs"
  mkdir -p "$OUT_DIR"
}

print_header() {
  echo ""
  echo "============================================================"
  echo "$1"
  echo "============================================================"
  echo ""
}

run_tool() {
  local name=$1
  local input=$2
  local out_file=$3

  "$SUI_SANDBOX" --state-file "$STATE_FILE" --rpc-url "$RPC_GRPC_URL" tool "$name" --input "$input" > "$out_file"
  python3 - "$out_file" <<'PY' >/dev/null
import json
import sys

path = sys.argv[1]
data = json.load(open(path))
if not data.get("success", False):
    raise SystemExit(f"tool failed: {data.get('error')}")
inner = data.get("result")
if isinstance(inner, dict) and inner.get("success") is False:
    raise SystemExit("tool result failed")
PY
}

replay_pair() {
  local digest=$1
  local label=$2

  local legacy_out="${OUT_DIR}/${label}_legacy.json"
  local tool_out="${OUT_DIR}/${label}_tool.json"

  print_header "Replay (legacy CLI) - ${label}"
  if "$SUI_SANDBOX" --state-file "$LEGACY_STATE_FILE" --rpc-url "$RPC_GRAPHQL_URL" --json replay "$digest" --compare > "$legacy_out"; then
    python3 - "$legacy_out" <<'PY' >/dev/null
import json
import sys

data = json.load(open(sys.argv[1]))
assert data.get("local_success") is True, data
PY
    echo "legacy ok"
  else
    echo "legacy replay failed (see ${legacy_out})"
  fi

  print_header "Replay (MCP tool) - ${label}"
  local payload
  payload=$(printf '{"digest":"%s","options":{"compare_effects":true}}' "$digest")
  "$SUI_SANDBOX" --state-file "$STATE_FILE" --rpc-url "$RPC_GRPC_URL" tool replay_transaction --input "$payload" > "$tool_out"
  python3 - "$tool_out" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if not data.get("success", False):
    err = str(data.get("error", ""))
    if "UNAUTHENTICATED" in err or "not found" in err:
        print("mcp replay skipped (gRPC access required)")
        sys.exit(0)
    raise SystemExit(err)
result = data.get("result") or {}
if result.get("success") is True:
    print("mcp ok")
    sys.exit(0)
err = str(result.get("error") or "")
if "UNAUTHENTICATED" in err or "not found" in err:
    print("mcp replay skipped (gRPC access required)")
    sys.exit(0)
raise SystemExit("mcp replay failed")
PY
}
