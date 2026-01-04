#!/bin/bash
# ========================================================================================
# Sui Move Interface Extractor - Docker Quickstart
# ========================================================================================
# This script is the canonical entry point for new users.
# It sets up your environment, builds the Docker image, and runs a sample benchmark.
# 
# Usage:
#   ./scripts/docker_quickstart.sh
# ========================================================================================

set -e

# --- Colors for Output ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== Sui Move Interface Extractor: Docker Quickstart ===${NC}\n"

REPO_ROOT="$(git rev-parse --show-toplevel)"
BENCHMARK_DIR="$REPO_ROOT/benchmark"
ENV_FILE="$BENCHMARK_DIR/.env"

# 1. API Key Validation
if [ -z "$OPENROUTER_API_KEY" ] && [ ! -f "$ENV_FILE" ]; then
    echo -e "${YELLOW}Missing OPENROUTER_API_KEY environment variable.${NC}"
    echo -n "Please enter your OpenRouter API Key: "
    read -s API_KEY
    echo ""
    export OPENROUTER_API_KEY="$API_KEY"
fi

# 2. Environment Setup
if [ ! -f "$ENV_FILE" ]; then
    echo -e "Creating $ENV_FILE..."
    cat > "$ENV_FILE" <<EOF
OPENROUTER_API_KEY=$OPENROUTER_API_KEY
SMI_MODEL=google/gemini-3-flash-preview
SMI_TEMPERATURE=0
SMI_MAX_TOKENS=4096
SMI_SENDER=0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0
EOF
    echo -e "${GREEN}✓ Environment configured.${NC}"
fi

# 3. Docker Verification
if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: Docker is not installed.${NC}"
    exit 1
fi

echo -e "Checking Docker image..."
if [[ "$(docker images -q smi-bench:latest 2> /dev/null)" == "" ]]; then
    echo -e "${YELLOW}Building Docker image (this may take a few minutes)...${NC}"
    docker build -t smi-bench:latest "$REPO_ROOT"
    echo -e "${GREEN}✓ Image built.${NC}"
else
    echo -e "${GREEN}✓ Docker image found.${NC}"
fi

# 4. Corpus Setup
REAL_CORPUS_PARENT="$REPO_ROOT/../sui-packages/packages"
DOCKER_TEST_CORPUS="$BENCHMARK_DIR/.docker_test_corpus"

if [ -d "$REAL_CORPUS_PARENT/mainnet_most_used" ]; then
   echo -e "${GREEN}✓ Using full mainnet corpus.${NC}"
   CORPUS_HOST="$REAL_CORPUS_PARENT"
   CORPUS_ROOT_IN_CONTAINER="/app/corpus/mainnet_most_used"
else
   echo -e "${YELLOW}! Full corpus not found. Setting up minimal test corpus...${NC}"
   mkdir -p "$DOCKER_TEST_CORPUS/0x00/fixture/bytecode_modules"
   echo '{"id": "0x0000000000000000000000000000000000000000000000000000000000000001"}' > "$DOCKER_TEST_CORPUS/0x00/fixture/metadata.json"
   # Create a dummy file if real fixture build not available
   if [ ! -f "$REPO_ROOT/tests/fixture/build/fixture/bytecode_modules/fixture.mv" ]; then
       touch "$DOCKER_TEST_CORPUS/0x00/fixture/bytecode_modules/dummy.mv"
   fi
   CORPUS_HOST="$DOCKER_TEST_CORPUS"
   CORPUS_ROOT_IN_CONTAINER="/app/corpus"
fi

# 5. Run Sample Benchmark
RUN_ID="quickstart_$(date +%Y%m%d_%H%M%S)"
echo -e "\n${BLUE}Starting sample benchmark run (ID: $RUN_ID)...${NC}"

# We use docker run --rm for the quickstart to keep things clean
docker run --rm \
    --name "smi-quickstart" \
    --env-file "$ENV_FILE" \
    -v "$CORPUS_HOST:/app/corpus" \
    -v "$BENCHMARK_DIR/results:/app/results" \
    smi-bench:latest smi-inhabit \
        --corpus-root "$CORPUS_ROOT_IN_CONTAINER" \
        --samples 1 \
        --agent real-openai-compatible \
        --simulation-mode build-only \
        --out "/app/results/$RUN_ID.json" \
        --no-log

# 6. Results Analysis
echo -e "\n${BLUE}=== Summary ===${NC}"
RESULT_FILE="$BENCHMARK_DIR/results/$RUN_ID.json"

if [ -f "$RESULT_FILE" ]; then
    echo -e "${GREEN}✓ Success! Benchmark finished successfully.${NC}"
    echo -e "Results saved to: benchmark/results/$RUN_ID.json"
    
    # Extract hit rate using python for clean output
    python3 -c "import json; d=json.load(open('$RESULT_FILE')); print(f'Avg Hit Rate: {d[\"aggregate\"]}.get(\"avg_hit_rate\", 0.0):.2%')"
    python3 -c "import json; d=json.load(open('$RESULT_FILE')); print(f'Errors: {d[\"aggregate\"]}.get(\"errors\", 0))')"
else
    echo -e "${RED}× Failure: Result file was not generated.${NC}"
    exit 1
fi

echo -e "\n${GREEN}Quickstart complete! You're ready to use the full harness.${NC}"
echo -e "To run more models, try: ./scripts/run_docker_benchmark.sh"
