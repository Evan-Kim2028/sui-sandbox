#!/bin/bash
# ==============================================================================
# Self-Heal Replay Example (CLI)
# ==============================================================================
# Demonstrates a two-pass replay:
# 1) Strict replay (no synthesis) – expected to fail when data is missing
# 2) Self-heal replay – synthesizes missing inputs + dynamic-field values
# ==============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
SUI_SANDBOX="${PROJECT_ROOT}/target/release/sui-sandbox"
DIGEST="${1:-}"

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

if [ ! -f "$SUI_SANDBOX" ]; then
  print_step "Building CLI (release)"
  cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
fi

if [ -z "$DIGEST" ]; then
  if [ -f "${PROJECT_ROOT}/selected_digests.txt" ]; then
    DIGEST=$(head -n 1 "${PROJECT_ROOT}/selected_digests.txt")
  fi
fi

if [ -z "$DIGEST" ]; then
  echo "Missing digest. Usage: $0 <DIGEST>"
  exit 1
fi

print_header "Self-Heal Replay (CLI)"
print_step "Digest: ${DIGEST}"

print_header "Pass 1: Strict replay (expected to fail if data missing)"
set +e
"$SUI_SANDBOX" replay "$DIGEST" --compare --fetch-strategy full
STATUS=$?
set -e
if [ $STATUS -ne 0 ]; then
  print_note "Strict replay failed (expected when data is missing)."
else
  print_note "Strict replay succeeded – self-heal may still be useful for edge cases."
fi

print_header "Pass 2: Self-heal replay (synthesize missing inputs + dynamic fields)"
SUI_SELF_HEAL_LOG=1 "$SUI_SANDBOX" replay "$DIGEST" \
  --compare \
  --fetch-strategy full \
  --synthesize-missing \
  --self-heal-dynamic-fields

print_note "Self-heal uses synthetic placeholders and is intended for testing only."
