#!/bin/bash
# ==============================================================================
# Obfuscated Package Analysis (CLI)
# ==============================================================================
# Reverse-engineers an obfuscated Sui Move package by combining bytecode
# inspection with PTB replay. Defaults to the Stonker market-making bot.
#
# Usage:
#   ./examples/obfuscated_package_analysis/cli_obfuscated_analysis.sh              # Stonker (default)
#   ./examples/obfuscated_package_analysis/cli_obfuscated_analysis.sh <PACKAGE_ID> # Any package
#
# Prerequisites:
#   - Static analysis (fetch + view): no special config needed
#   - Transaction replay: gRPC endpoint with historical data in .env
# ==============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
SUI_SANDBOX="${PROJECT_ROOT}/target/release/sui-sandbox"

# Default: Stonker market-making bot
STONKER_PKG="0xe3b9bd64ba2fb3256293c3fc0119994ec6fc7c96541680959de4d7052be65973"
PKG_ID="${1:-$STONKER_PKG}"

REPORT_DIR="/tmp/sui-sandbox-obfuscated-analysis"
REPORT_FILE="${REPORT_DIR}/report.jsonl"

# Known Stonker transaction digests for replay
STONKER_DIGESTS=(
  "7kMBy9LW6NshWEGi6TvAjeMUu1mc9TugY93RmRRbwf2r"   # Order cancellation (5 inputs)
  "DsgegauRVGMvVxEMeySoRceJTSGZa15F63pFxHMA2Hzr"     # Config update (2 inputs)
  "HXAygNqYf7AP1JgtTXT4SmJAJ8Q6vG5TWjb6aBgkfgGY"     # Multi-DEX rebalance (21 inputs)
)
STONKER_LABELS=(
  "Order cancellation (5 inputs, 10 mutated)"
  "Config update (2 inputs)"
  "Multi-DEX rebalance (21 inputs, 22 args)"
)

GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

print_header() {
  echo ""
  echo -e "${BLUE}════════════════════════════════════════════════════════════════${NC}"
  echo -e "${BLUE}  $1${NC}"
  echo -e "${BLUE}════════════════════════════════════════════════════════════════${NC}"
  echo ""
}

print_step() {
  echo -e "${GREEN}▶ $1${NC}"
}

print_note() {
  echo -e "${YELLOW}  Note: $1${NC}"
}

print_error() {
  echo -e "${RED}  Error: $1${NC}"
}

# Build if needed
if [ ! -f "$SUI_SANDBOX" ]; then
  print_step "Building CLI (release)"
  cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
fi

mkdir -p "$REPORT_DIR"
rm -f "$REPORT_FILE"

echo "{\"package\":\"$PKG_ID\",\"event\":\"start\",\"is_stonker\":$([ "$PKG_ID" = "$STONKER_PKG" ] && echo true || echo false)}" >> "$REPORT_FILE"

# ── Step 1: Fetch Package ────────────────────────────────────────────────────

print_header "Step 1: Fetch Package Bytecode + Dependencies"
print_step "sui-sandbox fetch package $PKG_ID --with-deps"
"$SUI_SANDBOX" fetch package "$PKG_ID" --with-deps

echo "{\"package\":\"$PKG_ID\",\"event\":\"fetched\"}" >> "$REPORT_FILE"

# ── Step 2: List Modules ─────────────────────────────────────────────────────

print_header "Step 2: List Modules (Obfuscated Names)"
print_step "sui-sandbox view modules $PKG_ID"

MODULES_JSON=$("$SUI_SANDBOX" --json view modules "$PKG_ID" 2>/dev/null || echo "{}")
MODULES=$(python3 -c "
import json, sys
try:
    j = json.loads('''$MODULES_JSON''')
    mods = j.get('modules', [])
    for m in mods:
        name = m.get('name') if isinstance(m, dict) else m
        if name:
            print(name)
except:
    pass
")

MODULE_COUNT=$(echo "$MODULES" | sed '/^$/d' | wc -l | tr -d ' ')
print_step "Modules found: $MODULE_COUNT"

if [ "$MODULE_COUNT" -gt 0 ]; then
  echo "$MODULES" | while read -r mod; do
    echo "    $mod"
  done
fi

echo "{\"package\":\"$PKG_ID\",\"event\":\"modules_listed\",\"count\":$MODULE_COUNT}" >> "$REPORT_FILE"

# ── Step 3: View Module Interfaces ───────────────────────────────────────────

print_header "Step 3: Inspect Module Interfaces"

# Pick a representative module to show typed signatures
FIRST_MODULE=$(echo "$MODULES" | head -1)

if [ -n "$FIRST_MODULE" ]; then
  print_step "sui-sandbox view module $PKG_ID::$FIRST_MODULE --json"
  print_note "Showing first module. Despite obfuscated names, parameter types are visible."
  echo ""

  MOD_DETAIL=$("$SUI_SANDBOX" --json view module "$PKG_ID::$FIRST_MODULE" 2>/dev/null || echo "{}")

  # Extract function summary
  python3 -c "
import json, sys
try:
    j = json.loads('''$MOD_DETAIL''')
    funcs = j.get('functions', [])
    structs = j.get('structs', [])
    friends = j.get('friends', [])
    print(f'  Structs: {len(structs)}')
    print(f'  Functions: {len(funcs)}')
    print(f'  Friend modules: {len(friends)}')
    print()
    for f in funcs:
        name = f.get('name', '?')
        vis = f.get('visibility', '?')
        params = f.get('params', f.get('parameters', '?'))
        type_params = f.get('type_params', f.get('type_parameters', '?'))
        print(f'    {vis:8s} {name}  (params: {params}, type_params: {type_params})')
except Exception as e:
    print(f'  (parse error: {e})')
"

  echo "{\"package\":\"$PKG_ID\",\"module\":\"$FIRST_MODULE\",\"event\":\"module_inspected\"}" >> "$REPORT_FILE"
else
  print_note "No modules found to inspect"
fi

# ── Step 4: Transaction Replay ───────────────────────────────────────────────

print_header "Step 4: Transaction Replay"

if [ "$PKG_ID" = "$STONKER_PKG" ]; then
  print_note "Replaying known Stonker transactions to validate bytecode analysis."
  print_note "Requires gRPC endpoint with historical data in .env"
  echo ""

  REPLAY_SUCCESS=0
  REPLAY_FAIL=0

  for i in "${!STONKER_DIGESTS[@]}"; do
    DIGEST="${STONKER_DIGESTS[$i]}"
    LABEL="${STONKER_LABELS[$i]}"

    print_step "$LABEL"
    echo "    Digest: $DIGEST"

    set +e
    OUTPUT=$("$SUI_SANDBOX" replay "$DIGEST" --compare --verbose 2>&1)
    STATUS=$?
    set -e

    if [ $STATUS -eq 0 ]; then
      # Check for match in output
      if echo "$OUTPUT" | grep -q "match"; then
        echo -e "    ${GREEN}MATCH${NC} — local effects match on-chain"
        REPLAY_SUCCESS=$((REPLAY_SUCCESS + 1))
        echo "{\"digest\":\"$DIGEST\",\"label\":\"$LABEL\",\"event\":\"replay\",\"status\":\"match\"}" >> "$REPORT_FILE"
      else
        echo -e "    ${YELLOW}COMPLETED${NC} — replay finished (check output for details)"
        REPLAY_SUCCESS=$((REPLAY_SUCCESS + 1))
        echo "{\"digest\":\"$DIGEST\",\"label\":\"$LABEL\",\"event\":\"replay\",\"status\":\"completed\"}" >> "$REPORT_FILE"
      fi
    else
      echo -e "    ${RED}FAILED${NC} — replay error (gRPC endpoint may not have historical data)"
      REPLAY_FAIL=$((REPLAY_FAIL + 1))
      echo "{\"digest\":\"$DIGEST\",\"label\":\"$LABEL\",\"event\":\"replay\",\"status\":\"failed\"}" >> "$REPORT_FILE"
    fi
    echo ""
  done

  print_step "Replay summary: $REPLAY_SUCCESS succeeded, $REPLAY_FAIL failed"
else
  print_note "Transaction replay requires known digests. For custom packages,"
  print_note "find transaction digests via a block explorer (e.g., SuiScan)"
  print_note "filtered by the package address, then replay with:"
  echo ""
  echo "    sui-sandbox replay <DIGEST> --compare --verbose"
  echo ""
  echo "{\"package\":\"$PKG_ID\",\"event\":\"replay_skipped\",\"reason\":\"custom_package\"}" >> "$REPORT_FILE"
fi

# ── Summary ──────────────────────────────────────────────────────────────────

print_header "Done"
print_step "Report: $REPORT_FILE"
print_note "Static analysis (Steps 1-3) works for any package."
print_note "Transaction replay (Step 4) requires gRPC + known digests."

if [ "$PKG_ID" = "$STONKER_PKG" ]; then
  echo ""
  print_note "Full Stonker analysis: https://github.com/RandyPen/Stonker/pull/1"
fi
