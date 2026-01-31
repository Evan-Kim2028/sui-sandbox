#!/bin/bash
# ==============================================================================
# Package Analysis (CLI)
# ==============================================================================
# Fetches a package bytecode + deps, lists modules/functions, and attempts to run
# entry functions with 0 params and 0 type params.
# ==============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
SUI_SANDBOX="${PROJECT_ROOT}/target/release/sui-sandbox"
PKG_ID="${1:-}"
REPORT_DIR="/tmp/sui-sandbox-package-analysis"
REPORT_FILE="${REPORT_DIR}/report.jsonl"

GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
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

if [ -z "$PKG_ID" ]; then
  echo "Usage: $0 <PACKAGE_ID>"
  exit 1
fi

if [ ! -f "$SUI_SANDBOX" ]; then
  print_step "Building CLI (release)"
  cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
fi

mkdir -p "$REPORT_DIR"
rm -f "$REPORT_FILE"

echo "{\"package\":\"$PKG_ID\",\"event\":\"start\"}" >> "$REPORT_FILE"

print_header "Fetch Package"
print_step "sui-sandbox fetch package $PKG_ID --with-deps"
"$SUI_SANDBOX" fetch package "$PKG_ID" --with-deps

echo "{\"package\":\"$PKG_ID\",\"event\":\"fetched\"}" >> "$REPORT_FILE"

print_header "List Modules"
MODULES_JSON=$($SUI_SANDBOX --json view modules "$PKG_ID")
MODULES=$(python - <<PY
import json
j=json.loads('''$MODULES_JSON''')
mods=j.get('modules',[])
for m in mods:
    name = m.get('name') if isinstance(m, dict) else m
    if name:
        print(name)
PY
)

MODULE_COUNT=$(echo "$MODULES" | sed '/^$/d' | wc -l | tr -d ' ')
print_step "Modules found: $MODULE_COUNT"

print_header "Attempt Entry Functions (0 args)"

for MODULE in $MODULES; do
  print_step "Module: $MODULE"
  MOD_JSON=$($SUI_SANDBOX --json view module "$PKG_ID::$MODULE")
  FUNCS=$(python - <<PY
import json
j=json.loads('''$MOD_JSON''')
for f in j.get('functions',[]):
    if f.get('is_entry') and f.get('params')==0 and f.get('type_params')==0 and f.get('visibility')=='public':
        print(f['name'])
PY
)

  if [ -z "$FUNCS" ]; then
    print_note "No callable entry functions with 0 params in $MODULE"
    echo "{\"package\":\"$PKG_ID\",\"module\":\"$MODULE\",\"event\":\"no_callable\"}" >> "$REPORT_FILE"
    continue
  fi

  for FN in $FUNCS; do
    print_step "Calling: $PKG_ID::$MODULE::$FN"
    set +e
    OUTPUT=$($SUI_SANDBOX run "$PKG_ID::$MODULE::$FN" --json 2>&1)
    STATUS=$?
    set -e
    if [ $STATUS -eq 0 ]; then
      echo "{\"package\":\"$PKG_ID\",\"module\":\"$MODULE\",\"function\":\"$FN\",\"status\":\"ok\"}" >> "$REPORT_FILE"
      print_note "Success"
    else
      echo "{\"package\":\"$PKG_ID\",\"module\":\"$MODULE\",\"function\":\"$FN\",\"status\":\"error\",\"error\":$(python - <<'PY'
import json,sys
err=sys.stdin.read().strip()
print(json.dumps(err))
PY
)" >> "$REPORT_FILE"
      print_note "Failed (see report)"
    fi
  done

done

print_header "Done"
print_step "Report: $REPORT_FILE"
print_note "Only entry functions with 0 params were attempted. Others were skipped."
