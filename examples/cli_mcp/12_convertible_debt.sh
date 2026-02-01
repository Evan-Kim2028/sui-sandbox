#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=examples/cli_mcp/_common.sh
source "${SCRIPT_DIR}/_common.sh"

init_context

ADMIN=0x300
BORROWER=0x100
LENDER=0x200

PKG_PATH="${PROJECT_ROOT}/examples/convertible_debt"

print_header "Publish convertible_debt package"
PUBLISH_OUT="${OUT_DIR}/convertible_publish.json"
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool publish --input "{\"path\":\"${PKG_PATH}\"}" > "$PUBLISH_OUT"
PKG_ID=$(python3 - "$PUBLISH_OUT" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
print(data["result"]["package_id"])
PY
)

print_header "Create oracle"
INIT_OUT="${OUT_DIR}/convertible_init.json"
INIT_PAYLOAD=$(cat <<JSON
{
  "inputs": [],
  "calls": [
    {"target": "${PKG_ID}::oracle::create_shared", "args": [{"u64": 2000000000}]}
  ],
  "sender": "${ADMIN}"
}
JSON
)
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool ptb --input "$INIT_PAYLOAD" > "$INIT_OUT"

extract_id() {
  local file=$1
  local type_suffix=$2
  python3 - "$file" "$type_suffix" <<'PY'
import json
import sys

path, type_suffix = sys.argv[1], sys.argv[2]
data = json.load(open(path))
changes = (data.get("result") or {}).get("effects", {}).get("object_changes", [])
for change in changes:
    typ = change.get("type") or ""
    if type_suffix not in typ:
        continue
    obj_id = change.get("object_id")
    if obj_id:
        print(obj_id)
        raise SystemExit(0)
raise SystemExit(f"object id not found for type {type_suffix}")
PY
}

ORACLE_ID=$(extract_id "$INIT_OUT" "::oracle::Oracle")

print_header "Create demo coins"
USD_OUT="${OUT_DIR}/convertible_usd.json"
ETH_OUT="${OUT_DIR}/convertible_eth.json"
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool configure --input "{\"action\":\"set_sender\",\"params\":{\"address\":\"${LENDER}\"}}"
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool create_asset --input "{\"type\":\"custom_coin\",\"amount\":1000000000,\"type_tag\":\"${PKG_ID}::tokens::USD\"}" > "$USD_OUT"
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool configure --input "{\"action\":\"set_sender\",\"params\":{\"address\":\"${BORROWER}\"}}"
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool create_asset --input "{\"type\":\"custom_coin\",\"amount\":1000000000,\"type_tag\":\"${PKG_ID}::tokens::ETH\"}" > "$ETH_OUT"

USD_COIN_LENDER=$(python3 - "$USD_OUT" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
print(data["result"]["object_id"])
PY
)
ETH_COIN_BORROWER=$(python3 - "$ETH_OUT" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
print(data["result"]["object_id"])
PY
)

print_header "Borrower creates offer (locks ETH collateral)"
OFFER_OUT="${OUT_DIR}/convertible_offer.json"
OFFER_PAYLOAD=$(cat <<JSON
{
  "inputs": [
    {"imm_or_owned_object": "${ETH_COIN_BORROWER}"},
    {"shared_object": {"id": "${ORACLE_ID}", "mutable": false}}
  ],
  "calls": [
    {
      "target": "${PKG_ID}::convertible_debt::create_offer",
      "args": [
        {"input": 0},
        {"u64": 1000000000},
        {"u64": 500},
        {"u64": 0},
        {"input": 1}
      ]
    }
  ],
  "sender": "${BORROWER}"
}
JSON
)
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool ptb --input "$OFFER_PAYLOAD" > "$OFFER_OUT"
OFFER_ID=$(extract_id "$OFFER_OUT" "::convertible_debt::Offer")

print_header "Lender takes offer (receives shared note)"
TAKE_OUT="${OUT_DIR}/convertible_take.json"
TAKE_PAYLOAD=$(cat <<JSON
{
  "inputs": [
    {"shared_object": {"id": "${OFFER_ID}", "mutable": true}},
    {"imm_or_owned_object": "${USD_COIN_LENDER}"}
  ],
  "calls": [
    {
      "target": "${PKG_ID}::convertible_debt::take_offer",
      "args": [
        {"input": 0},
        {"input": 1}
      ]
    }
  ],
  "sender": "${LENDER}"
}
JSON
)
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool ptb --input "$TAKE_PAYLOAD" > "$TAKE_OUT"
NOTE_ID=$(extract_id "$TAKE_OUT" "::convertible_debt::Note")

print_header "Lender converts to ETH at strike (upside scenario)"
CONVERT_OUT="${OUT_DIR}/convertible_convert.json"
CONVERT_PAYLOAD=$(cat <<JSON
{
  "inputs": [
    {"shared_object": {"id": "${NOTE_ID}", "mutable": true}}
  ],
  "calls": [
    {
      "target": "${PKG_ID}::convertible_debt::convert",
      "args": [
        {"input": 0}
      ]
    }
  ],
  "sender": "${LENDER}"
}
JSON
)
"$SUI_SANDBOX" --state-file "$STATE_FILE" tool ptb --input "$CONVERT_PAYLOAD" > "$CONVERT_OUT"

print_header "Done"
cat <<MSG
Convertible note flow completed.

Artifacts:
- Package: ${PKG_ID}
- Oracle: ${ORACLE_ID}
- Offer: ${OFFER_ID}
- Note: ${NOTE_ID}

Output JSON files in ${OUT_DIR}
MSG
