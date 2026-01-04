#!/bin/bash
# ========================================================================================
# Sui Move Interface Extractor - Top 25 Quickstart (GPT-5.2)
# ========================================================================================
# This script runs a full Phase II benchmark on the Curated Top 25 dataset
# using the high-performance GPT-5.2 model.
#
# Metrics tracked: Hit Rate, Dry Run Success, Reasoning/Parse OK, Tokens, and Timing.
#
# Usage:
#   ./scripts/top25_quickstart.sh
# ========================================================================================

set -e

# --- Colors for Output ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== Sui Move Interface Extractor: Top 25 Evaluation ===${NC}\n"

REPO_ROOT="$(git rev-parse --show-toplevel)"
BENCHMARK_DIR="$REPO_ROOT/benchmark"
ENV_FILE="$BENCHMARK_DIR/.env"

# 1. API Key Validation
# Check current env, then .env file
if [ -z "$OPENROUTER_API_KEY" ] && [ -f "$ENV_FILE" ]; then
    # Try to extract from .env
    OPENROUTER_API_KEY=$(grep "^OPENROUTER_API_KEY=" "$ENV_FILE" | cut -d'=' -f2-)
fi

if [ -z "$OPENROUTER_API_KEY" ] && [ "$SMI_AGENT" != "mock-empty" ] && [ "$SMI_AGENT" != "mock-perfect" ]; then
    echo -e "${YELLOW}Missing OPENROUTER_API_KEY.${NC}"
    echo -e "This key is required to run real LLM benchmarks via OpenRouter."
    echo -n "Please enter your OpenRouter API Key (input hidden): "
    read -s API_KEY
    echo ""
    export OPENROUTER_API_KEY="$API_KEY"
fi

# 2. Environment Setup
if [ ! -f "$ENV_FILE" ]; then
    echo -e "Creating $ENV_FILE..."
    cat > "$ENV_FILE" <<EOF
OPENROUTER_API_KEY=$OPENROUTER_API_KEY
SMI_MODEL=openai/gpt-5.2
SMI_TEMPERATURE=0
SMI_MAX_TOKENS=4096
SMI_SENDER=0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0
EOF
    echo -e "${GREEN}✓ Environment configured.${NC}"
else
    # Verify the existing .env has the key if we aren't in mock mode
    if [ "$SMI_AGENT" != "mock-empty" ] && [ "$SMI_AGENT" != "mock-perfect" ] && ! grep -q "^OPENROUTER_API_KEY=" "$ENV_FILE"; then
        echo -e "${YELLOW}Warning: $ENV_FILE exists but is missing OPENROUTER_API_KEY.${NC}"
        echo "Appending provided key to $ENV_FILE..."
        echo "OPENROUTER_API_KEY=$OPENROUTER_API_KEY" >> "$ENV_FILE"
    fi
fi

# 3. Dependencies check
if ! command -v uv &> /dev/null; then
    echo -e "${RED}Error: 'uv' not found. Please install uv first.${NC}"
    exit 1
fi

# 4. Corpus Detection
REAL_CORPUS_PARENT="$REPO_ROOT/../sui-packages/packages/mainnet_most_used"
if [ ! -d "$REAL_CORPUS_PARENT" ]; then
    echo -e "${RED}Error: Mainnet corpus not found at $REAL_CORPUS_PARENT${NC}"
    echo -e "Please run: git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages"
    exit 1
fi

# 5. Run Benchmark
RUN_ID="top25_gpt52_$(date +%Y%m%d_%H%M%S)"
echo -e "${GREEN}✓ Environment and corpus verified.${NC}"
echo -e "${BLUE}Starting comprehensive run on Top 25 dataset using openai/gpt-5.2...${NC}"

cd "$BENCHMARK_DIR"
SMI_MODEL="openai/gpt-5.2" uv run smi-inhabit \
    --corpus-root "$REAL_CORPUS_PARENT" \
    --dataset type_inhabitation_top25 \
    --samples 25 \
    --agent "${SMI_AGENT:-real-openai-compatible}" \
    --simulation-mode dry-run \
    --out "results/$RUN_ID.json" \
    --per-package-timeout-seconds 180 \
    --checkpoint-every 5

# 6. Comprehensive Results Analysis
echo -e "\n${BLUE}=== Evaluation Summary ===${NC}"
RESULT_FILE="results/$RUN_ID.json"

if [ -f "$RESULT_FILE" ]; then
    "$REPO_ROOT/scripts/analyze_run.py" "$RESULT_FILE"
    echo -e "Full JSON results: benchmark/results/$RUN_ID.json"
else
    echo -e "${RED}× Failure: Result file was not generated.${NC}"
    exit 1
fi

echo -e "\n${GREEN}Evaluation complete!${NC}"