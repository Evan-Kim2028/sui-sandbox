#!/bin/bash
# ==============================================================================
# Sui Sandbox MCP Tool Workflow Example (CLI parity)
# ==============================================================================
#
# This script mirrors examples/cli_workflow.sh, but uses the MCP tool interface
# via: sui-sandbox tool <name> --input '{...}'
#
# It demonstrates:
# 1) Inspecting framework interfaces
# 2) Reading sandbox state
# 3) Creating + editing a Move project
# 4) Building + deploying locally
# 5) Calling a function
#
# Prerequisites:
#   - Build the CLI: cargo build --release --bin sui-sandbox
#
# Usage:
#   ./examples/mcp_cli_workflow.sh
# ==============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
SUI_SANDBOX="${PROJECT_ROOT}/target/release/sui-sandbox"
STATE_DIR="/tmp/sui-sandbox-mcp-example"
STATE_FILE="${STATE_DIR}/state.json"

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

pretty_json() {
    python3 -c $'import json,sys\ntry:\n    data=json.load(sys.stdin)\n    json.dump(data, sys.stdout, indent=2)\nexcept Exception:\n    sys.stdout.write(sys.stdin.read())'
}

# ==============================================================================
# Setup
# ==============================================================================

print_header "Setup"

if [ ! -f "$SUI_SANDBOX" ]; then
    echo "Building sui-sandbox CLI..."
    cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
fi

rm -rf "$STATE_DIR"
mkdir -p "$STATE_DIR"

print_step "Using MCP state file: $STATE_FILE"

# ==============================================================================
# Step 1: Inspect Framework Interfaces
# ==============================================================================

print_header "Step 1: Inspect Framework Interfaces"

print_step "Reading 0x2::coin interface via MCP..."
INTERFACE_JSON=$($SUI_SANDBOX --state-file "$STATE_FILE" tool get_interface \
  --input '{"package":"0x2","module":"coin"}')
echo "$INTERFACE_JSON" | pretty_json | sed -n '1,40p'
echo "  ... (truncated)"

# ==============================================================================
# Step 2: Check Session Status
# ==============================================================================

print_header "Step 2: Check Session Status"

print_step "Getting sandbox summary..."
$SUI_SANDBOX --state-file "$STATE_FILE" tool get_state | pretty_json

# ==============================================================================
# Step 3: Create a Move Project
# ==============================================================================

print_header "Step 3: Create a Move Project"

print_step "Creating project mcp_demo..."
PROJECT_JSON=$($SUI_SANDBOX --state-file "$STATE_FILE" tool create_move_project \
  --input '{"name":"mcp_demo","persist":true}')
PROJECT_ID=$(echo "$PROJECT_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["project_id"])')
PROJECT_PATH=$(echo "$PROJECT_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["path"])')

echo "  project_id: $PROJECT_ID"
echo "  path: $PROJECT_PATH"

print_step "Reading the default module..."
$SUI_SANDBOX --state-file "$STATE_FILE" tool read_move_file \
  --input "{\"project_id\":\"$PROJECT_ID\",\"file\":\"sources/mcp_demo.move\"}" \
  | pretty_json

# ==============================================================================
# Step 4: Build + Deploy
# ==============================================================================

print_header "Step 4: Build + Deploy"

print_step "Building project..."
$SUI_SANDBOX --state-file "$STATE_FILE" tool build_project \
  --input "{\"project_id\":\"$PROJECT_ID\"}" | pretty_json

print_step "Deploying project locally..."
DEPLOY_JSON=$($SUI_SANDBOX --state-file "$STATE_FILE" tool deploy_project \
  --input "{\"project_id\":\"$PROJECT_ID\"}")
PACKAGE_ID=$(echo "$DEPLOY_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["package_id"])')

echo "  package_id: $PACKAGE_ID"

# ==============================================================================
# Step 5: Call a Function
# ==============================================================================

print_header "Step 5: Call a Function"

print_step "Calling mcp_demo::add(1, 2)..."
$SUI_SANDBOX --state-file "$STATE_FILE" tool call_function \
  --input "{\"package\":\"$PACKAGE_ID\",\"module\":\"mcp_demo\",\"function\":\"add\",\"args\":[1,2]}" \
  | pretty_json

# ==============================================================================
# Step 6: State Summary
# ==============================================================================

print_header "Step 6: State Summary"

print_step "Getting updated sandbox summary..."
$SUI_SANDBOX --state-file "$STATE_FILE" tool get_state | pretty_json

# ==============================================================================
# Step 7: Clean Up
# ==============================================================================

print_header "Step 7: Clean Up"

print_step "Removing MCP state file..."
rm -f "$STATE_FILE"

# ==============================================================================
# Summary
# ==============================================================================

print_header "Summary"

cat << 'SUMMARY'
This MCP workflow mirrored the classic CLI example using:

  - tool get_interface  (module inspection)
  - tool get_state      (session summary)
  - tool create_move_project / build_project / deploy_project
  - tool call_function  (execute Move)

Because all MCP tools share a single JSON state file, this script preserves
state across invocations just like the CLI.
SUMMARY

print_note "See examples/cli_workflow.sh for the classic CLI flow."
