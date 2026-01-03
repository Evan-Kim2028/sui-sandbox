#!/bin/bash
set -e

# Local Quickstart Verification Script
# This script validates the environment and runs a small benchmark sample
# using the high-performance Gemini 3 Flash model.

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT/benchmark"

echo "=== Sui Move Interface Extractor: Local Quickstart ==="

# 1. Verify Dependencies
if ! command -v uv &> /dev/null;
    then
    echo "Error: 'uv' not found. Please install uv first."
    exit 1
fi

# 2. Verify Corpus
# Expected location relative to this project
CORPUS_DIR="../../sui-packages/packages/mainnet_most_used"
if [ ! -d "$CORPUS_DIR" ]; then
    echo "Error: Corpus not found at $CORPUS_DIR"
    echo "Please run: git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages"
    exit 1
fi

# 3. Setup Environment
if [ ! -f .env ]; then
    echo "Creating .env from .env.example..."
    cp .env.example .env
fi

# Standard funded sender for mainnet simulation (public)
FUNDED_SENDER="0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0"
export SMI_SENDER="$FUNDED_SENDER"

# 4. Model Selection (Gemini 3 Flash - Dec 2025 Release)
export SMI_MODEL="google/gemini-3-flash-preview"
echo "Target Model: $SMI_MODEL"

# 5. Run the Benchmark
# Using the standardized 'type_inhabitation_top25' dataset for high-signal testing.
echo "Running sample benchmark (2 packages from Top-25)..."

uv run smi-inhabit \
  --corpus-root "$CORPUS_DIR" \
  --dataset type_inhabitation_top25 \
  --samples 2 \
  --agent real-openai-compatible \
  --simulation-mode dry-run \
  --out "results/local_quickstart_results.json" \
  --per-package-timeout-seconds 90 \
  --checkpoint-every 1

# 6. Verify Results
echo ""
echo "=== Results Verification ==="
if [ -f "results/local_quickstart_results.json" ]; then
    COUNT=$(grep -c "package_id" "results/local_quickstart_results.json")
    if [ "$COUNT" -ge 1 ]; then
        echo "SUCCESS: Processed $COUNT packages."
        # Extract and display summary hit rates
        grep "aggregate" -A 5 "results/local_quickstart_results.json"
    else
        echo "FAILURE: Result file found but contains no package results."
        exit 1
    fi
else
    echo "FAILURE: Result file 'results/local_quickstart_results.json' was not created."
    exit 1
fi

echo ""
echo "Quickstart verification complete!"