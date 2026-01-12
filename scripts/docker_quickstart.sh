#!/bin/bash
# ========================================================================================
# Sui Move Interface Extractor - Docker Quickstart (v0.4.0)
# ========================================================================================
# This script is the canonical entry point for new users.
# It sets up your environment, builds the Docker image, and runs a sample benchmark
# using the A2A (Agent-to-Agent) server mode.
#
# Usage:
#   ./scripts/docker_quickstart.sh [OPTIONS]
#
# Options:
#   --dataset NAME    Use a specific dataset (default: quickstart_top5)
#   --samples N       Number of samples to run (default: 5)
#   --skip-build      Skip Docker image build check
#   --mock            Run with mock agent (no API key needed)
#   --port PORT       Port for A2A server (default: 9999)
#
# Available Datasets:
#   quickstart_top5         - 5 packages (1 easy, 2 medium, 2 hard) - DEFAULT
#   quickstart_2            - 2 packages, minimal test
#   type_inhabitation_top25 - Full benchmark (25 packages)
#
# Examples:
#   ./scripts/docker_quickstart.sh                          # Default quickstart (5 packages)
#   ./scripts/docker_quickstart.sh --dataset quickstart_2   # Quick 2-package test
#   ./scripts/docker_quickstart.sh --mock                   # No API key needed
# ========================================================================================

set -e

# --- Colors for Output ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# --- Default Configuration ---
DATASET="quickstart_top5"
SAMPLES=5
SKIP_BUILD=0
MOCK_MODE=0
PORT=9999

# --- Parse Arguments ---
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dataset)
            DATASET="$2"
            shift 2
            ;;
        --samples)
            SAMPLES="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --mock)
            MOCK_MODE=1
            export SMI_AGENT="mock-empty"
            shift
            ;;
        --port)
            PORT="$2"
            shift 2
            ;;
        --help|-h)
            head -30 "$0" | tail -28
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            echo "Run with --help for usage information"
            exit 1
            ;;
    esac
done

echo -e "${BLUE}=== Sui Move Interface Extractor: Docker Quickstart (v0.4.0) ===${NC}\n"

REPO_ROOT="$(git rev-parse --show-toplevel)"
BENCHMARK_DIR="$REPO_ROOT/benchmark"
ENV_FILE="$BENCHMARK_DIR/.env"
CONTAINER_NAME="smi-quickstart-${PORT}"

# 1. API Key Validation
# Check current env, then .env file
if [ -z "$OPENROUTER_API_KEY" ] && [ -f "$ENV_FILE" ]; then
    # Try to extract from .env
    OPENROUTER_API_KEY=$(grep "^OPENROUTER_API_KEY=" "$ENV_FILE" | cut -d'=' -f2-)
fi

if [ -z "$OPENROUTER_API_KEY" ] && [ "$SMI_AGENT" != "mock-empty" ]; then
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
SMI_MODEL=google/gemini-3-flash-preview
SMI_TEMPERATURE=0
SMI_MAX_TOKENS=4096
SMI_SENDER=0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0
EOF
    echo -e "${GREEN}✓ Environment configured.${NC}"
else
    # Verify the existing .env has the key if we aren't in mock mode
    if [ "$SMI_AGENT" != "mock-empty" ] && ! grep -q "^OPENROUTER_API_KEY=" "$ENV_FILE"; then
        echo -e "${YELLOW}Warning: $ENV_FILE exists but is missing OPENROUTER_API_KEY.${NC}"
        echo "Appending provided key to $ENV_FILE..."
        echo "OPENROUTER_API_KEY=$OPENROUTER_API_KEY" >> "$ENV_FILE"
    fi
fi

# 3. Docker Verification
if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: Docker is not installed.${NC}"
    exit 1
fi

if [ $SKIP_BUILD -eq 0 ]; then
    echo -e "Checking Docker image..."
    if [[ "$(docker images -q smi-bench:latest 2> /dev/null)" == "" ]]; then
        echo -e "${YELLOW}Building Docker image (this may take a few minutes)...${NC}"
        docker build -t smi-bench:latest "$REPO_ROOT"
        echo -e "${GREEN}✓ Image built.${NC}"
    else
        echo -e "${GREEN}✓ Docker image found.${NC}"
    fi
else
    echo -e "${YELLOW}Skipping Docker image build check (--skip-build)${NC}"
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

# 5. Setup Manifest from Dataset
MANIFEST_FILE="$BENCHMARK_DIR/manifests/datasets/${DATASET}.txt"
MANIFEST_HOST="$BENCHMARK_DIR/manifests/active_quickstart_manifest.txt"
RESULTS_HOST="$BENCHMARK_DIR/results"
LOGS_HOST="$BENCHMARK_DIR/logs"
mkdir -p "$(dirname "$MANIFEST_HOST")" "$RESULTS_HOST" "$LOGS_HOST"

if [ -f "$MANIFEST_FILE" ]; then
    echo -e "Using dataset: ${GREEN}$DATASET${NC}"
    cp "$MANIFEST_FILE" "$MANIFEST_HOST"
else
    echo -e "${YELLOW}Dataset '$DATASET' not found at $MANIFEST_FILE${NC}"
    echo "Available datasets:"
    ls -1 "$BENCHMARK_DIR/manifests/datasets/"*.txt 2>/dev/null | xargs -n1 basename | sed 's/\.txt$//' | sed 's/^/  - /'
    exit 1
fi

# 6. Cleanup any existing container
EXISTING_CID=$(docker ps -a --filter "name=${CONTAINER_NAME}" -q 2>/dev/null || true)
if [ -n "$EXISTING_CID" ]; then
    echo -e "Cleaning up existing container..."
    docker rm -f "$EXISTING_CID" > /dev/null 2>&1 || true
fi

# 7. Start A2A Server Container
RUN_ID="quickstart_${DATASET}_$(date +%Y%m%d_%H%M%S)"
AGENT="${SMI_AGENT:-real-openai-compatible}"
SENDER="${SMI_SENDER:-0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0}"

echo -e "\n${BLUE}Starting A2A server on port $PORT...${NC}"
echo -e "  Dataset:  $DATASET"
echo -e "  Samples:  $SAMPLES"
echo -e "  Agent:    $AGENT"
echo -e "  Run ID:   $RUN_ID"
echo ""

CONTAINER_ID=$(docker run -d \
    --name "$CONTAINER_NAME" \
    --env-file "$ENV_FILE" \
    -v "$CORPUS_HOST:/app/corpus" \
    -v "$MANIFEST_HOST:/app/manifest.txt" \
    -v "$RESULTS_HOST:/app/results" \
    -v "$LOGS_HOST:/app/benchmark/logs" \
    -p $PORT:9999 \
    smi-bench:latest)

# 8. Wait for A2A server to be ready
echo -e "Waiting for A2A server health..."
for i in {1..60}; do
    if curl -s http://localhost:$PORT/health > /dev/null 2>&1; then
        echo -e "${GREEN}✓ A2A server is ready.${NC}"
        break
    fi
    if [ $i -eq 60 ]; then
        echo -e "${RED}× Timeout waiting for A2A server.${NC}"
        docker logs "$CONTAINER_ID" 2>&1 | tail -20
        docker rm -f "$CONTAINER_ID" > /dev/null 2>&1 || true
        exit 1
    fi
    sleep 1
done

# 9. Submit benchmark task via JSON-RPC
echo -e "\n${BLUE}Submitting benchmark task...${NC}"

PAYLOAD=$(cat <<EOF
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "message/send",
  "params": {
    "message": {
      "messageId": "$RUN_ID",
      "role": "user",
      "parts": [
        {
          "text": "{\"config\": { \"corpus_root\": \"$CORPUS_ROOT_IN_CONTAINER\", \"package_ids_file\": \"/app/manifest.txt\", \"agent\": \"$AGENT\", \"samples\": $SAMPLES, \"simulation_mode\": \"dry-run\", \"run_id\": \"$RUN_ID\", \"continue_on_error\": true, \"resume\": false, \"sender\": \"$SENDER\", \"checkpoint_every\": 1}, \"out_dir\": \"/app/results\"}"
        }
      ]
    }
  }
}
EOF
)

RESPONSE=$(curl -s -X POST http://localhost:$PORT/ -H "Content-Type: application/json" -d "$PAYLOAD")
if echo "$RESPONSE" | grep -q "error"; then
    echo -e "${RED}× Failed to submit task:${NC}"
    echo "$RESPONSE" | jq . 2>/dev/null || echo "$RESPONSE"
    docker rm -f "$CONTAINER_ID" > /dev/null 2>&1 || true
    exit 1
fi

echo -e "${GREEN}✓ Task submitted successfully.${NC}"
echo -e "\nBenchmark running. Tailing logs (Ctrl+C to detach)...\n"

# 10. Tail logs and wait for completion
# We'll watch for the result file to appear
docker logs -f "$CONTAINER_ID" &
LOG_PID=$!

# Monitor for completion
RESULT_FILE="$RESULTS_HOST/$RUN_ID.json"
TIMEOUT=600  # 10 minutes max
ELAPSED=0

while [ $ELAPSED -lt $TIMEOUT ]; do
    if [ -f "$RESULT_FILE" ]; then
        # Give it a moment to finish writing
        sleep 2
        break
    fi
    sleep 5
    ELAPSED=$((ELAPSED + 5))
done

# Stop log tailing
kill $LOG_PID 2>/dev/null || true

# 11. Cleanup container
echo -e "\n\nStopping A2A server..."
docker rm -f "$CONTAINER_ID" > /dev/null 2>&1 || true

# 12. Results Analysis
echo -e "\n${BLUE}=== Summary ===${NC}"

if [ -f "$RESULT_FILE" ]; then
    echo -e "${GREEN}✓ Success! Benchmark finished successfully.${NC}"
    echo -e "Results saved to: benchmark/results/$RUN_ID.json"

    # Extract hit rate using python for clean output
    echo -n "  • "
    python3 -c "import json; d=json.load(open('$RESULT_FILE')); print(f'Avg Hit Rate: {d[\"aggregate\"].get(\"avg_hit_rate\", 0.0):.2%}')" 2>/dev/null || echo "Avg Hit Rate: N/A"
    echo -n "  • "
    python3 -c "import json; d=json.load(open('$RESULT_FILE')); print(f'Errors: {d[\"aggregate\"].get(\"errors\", 0)}')" 2>/dev/null || echo "Errors: N/A"
    echo -n "  • "
    python3 -c "import json; d=json.load(open('$RESULT_FILE')); print(f'Packages: {d.get(\"samples\", len(d.get(\"packages\", [])))}')" 2>/dev/null || echo "Packages: N/A"
else
    echo -e "${RED}× Failure: Result file was not generated within timeout.${NC}"
    echo -e "Check logs in: benchmark/logs/"
    exit 1
fi

echo -e "\n${GREEN}Quickstart complete! You're ready to use the full harness.${NC}"
echo ""
echo -e "Next steps:"
echo -e "  • Run full benchmark:    ${BLUE}./scripts/run_docker_benchmark.sh${NC}"
echo -e "  • Quick 2-package test:  ${BLUE}./scripts/docker_quickstart.sh --dataset quickstart_2${NC}"
echo -e "  • Full 25-package eval:  ${BLUE}./scripts/docker_quickstart.sh --dataset type_inhabitation_top25${NC}"
echo ""
