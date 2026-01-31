#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/_common.sh"

init_context
print_header "PTB Basics via MCP tool"

# Create a SUI coin to act as input
COIN_OUT="${OUT_DIR}/ptb_coin.json"
run_tool create_asset '{"type":"sui_coin","amount":1000}' "$COIN_OUT"
COIN_ID=$(python3 -c "import json; data=json.load(open('$COIN_OUT')); print(data['result']['object_id'])")

# Execute a PTB: split coin and transfer the new coin
PTB_OUT="${OUT_DIR}/ptb_exec.json"
PTB_INPUT=$(python3 - <<PY
import json
payload={
  "inputs":[
    {"object_id":"${COIN_ID}", "kind":"mutable"},
    {"type":"u64", "value": 100},
    {"type":"address", "value": "0x1"}
  ],
  "commands":[
    {"kind":"SplitCoins", "coin":{"input":0}, "amounts":[{"input":1}]},
    {"kind":"TransferObjects", "objects":[{"nested_result":[0,0]}], "address":{"input":2}}
  ]
}
print(json.dumps(payload))
PY
)
run_tool execute_ptb "$PTB_INPUT" "$PTB_OUT"

print_header "PTB basics complete"
