#!/bin/bash
# ==============================================================================
# Sui Sandbox CLI Workflow Example
# ==============================================================================
#
# This script demonstrates a typical development workflow using the sui-sandbox
# CLI. It shows how to:
#
# 1. View built-in framework modules
# 2. Publish a local package
# 3. Execute functions
# 4. Inspect session state
#
# Unlike the Rust examples, this uses the CLI binary directly, which is useful
# for scripting, CI/CD pipelines, or interactive exploration.
#
# Prerequisites:
#   - Build the CLI: cargo build --release --bin sui-sandbox
#   - Ensure the binary is in PATH or use full path
#
# Usage:
#   ./examples/cli_workflow.sh
#
# ==============================================================================

set -e  # Exit on error

# Configuration
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
SUI_SANDBOX="${PROJECT_ROOT}/target/release/sui-sandbox"
STATE_FILE="/tmp/sui-sandbox-example/state.json"
FIXTURE_DIR="${PROJECT_ROOT}/tests/fixture/build/fixture"

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

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

# ==============================================================================
# Setup
# ==============================================================================

print_header "Setup"

# Check if binary exists
if [ ! -f "$SUI_SANDBOX" ]; then
    echo "Building sui-sandbox CLI..."
    cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
fi

# Clean up any previous state
rm -rf /tmp/sui-sandbox-example
mkdir -p /tmp/sui-sandbox-example

print_step "Using state file: $STATE_FILE"
print_step "Using fixture: $FIXTURE_DIR"

# ==============================================================================
# Step 1: Explore Framework Modules
# ==============================================================================

print_header "Step 1: Explore Framework Modules"

print_step "Listing modules in the Sui framework (0x2)..."
$SUI_SANDBOX --state-file "$STATE_FILE" view modules 0x2 2>/dev/null | head -20
echo "  ... (truncated)"

print_step "Viewing the coin module interface..."
$SUI_SANDBOX --state-file "$STATE_FILE" view module 0x2::coin

# ==============================================================================
# Step 2: Check Session Status
# ==============================================================================

print_header "Step 2: Check Session Status"

print_step "Viewing session status..."
$SUI_SANDBOX --state-file "$STATE_FILE" status

print_step "Getting status as JSON..."
$SUI_SANDBOX --state-file "$STATE_FILE" --json status | head -10

# ==============================================================================
# Step 3: Publish a Test Package
# ==============================================================================

print_header "Step 3: Publish a Test Package"

if [ -d "$FIXTURE_DIR" ]; then
    print_step "Publishing test fixture package..."
    $SUI_SANDBOX --state-file "$STATE_FILE" publish "$FIXTURE_DIR" \
        --bytecode-only \
        --address fixture=0x100

    print_step "Verifying package was loaded..."
    $SUI_SANDBOX --state-file "$STATE_FILE" view packages
else
    print_note "Fixture directory not found, skipping publish step"
fi

# ==============================================================================
# Step 4: Execute a Function
# ==============================================================================

print_header "Step 4: Execute Framework Functions"

print_step "Creating a zero-value SUI coin..."
$SUI_SANDBOX --state-file "$STATE_FILE" run 0x2::coin::zero \
    --type-arg 0x2::sui::SUI \
    --sender 0x1

print_note "The coin::zero function creates an empty Coin<SUI> object"

# ==============================================================================
# Step 5: JSON Output for Scripting
# ==============================================================================

print_header "Step 5: JSON Output for Scripting"

print_step "Getting module info as JSON (useful for scripts)..."
$SUI_SANDBOX --state-file "$STATE_FILE" --json view module 0x2::coin 2>/dev/null | head -30
echo "  ... (truncated)"

# ==============================================================================
# Step 6: Bridge to Sui Client (Deployment Transition)
# ==============================================================================

print_header "Step 6: Bridge to Sui Client"

print_step "Generate sui client publish command..."
$SUI_SANDBOX bridge publish "$FIXTURE_DIR" --quiet

print_step "Generate sui client call command..."
$SUI_SANDBOX bridge call 0x2::coin::zero --type-arg 0x2::sui::SUI --quiet

print_note "The bridge command helps you transition from sandbox to real deployment"

# ==============================================================================
# Step 7: Clean Up
# ==============================================================================

print_header "Step 7: Clean Up"

print_step "Cleaning session state..."
$SUI_SANDBOX --state-file "$STATE_FILE" clean

print_step "Verifying state was removed..."
if [ ! -f "$STATE_FILE" ]; then
    echo "  State file successfully removed"
else
    echo "  Warning: State file still exists"
fi

# ==============================================================================
# Summary
# ==============================================================================

print_header "Summary"

cat << 'EOF'
This example demonstrated the sui-sandbox CLI workflow:

  1. view modules <package>  - List modules in a package
  2. view module <path>      - Inspect module interface (structs, functions)
  3. status                  - Check session state
  4. publish <path>          - Deploy a Move package locally
  5. run <target>            - Execute a function
  6. bridge <subcommand>     - Generate sui client commands for deployment
  7. clean                   - Reset session state

For more advanced workflows:

  # Fetch mainnet packages
  sui-sandbox fetch package 0x1eabed72c53feb73...

  # Execute a PTB from JSON
  sui-sandbox ptb --spec transaction.json --sender 0x1

  # Replay a mainnet transaction
  sui-sandbox replay 9V3xKMnFpXyz... --compare

  # Generate deployment commands when ready
  sui-sandbox bridge publish ./my_package
  sui-sandbox bridge call 0xPKG::module::func --arg 42

See docs/reference/CLI_REFERENCE.md for complete documentation.
EOF

echo ""
echo -e "${GREEN}✓ Example completed successfully${NC}"
