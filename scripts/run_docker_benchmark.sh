#!/bin/bash
set -e

# ========================================================================================
# Sui Move Interface Extractor - Production Benchmark Runner (Docker)
# ========================================================================================
# This script runs benchmark in a Docker container with persistent logging and results.
# It implements smart container reuse to avoid spamming new containers for each run.
#
# Usage:
#   ./scripts/run_docker_benchmark.sh [MODEL_NAME] [SAMPLES] [PORT] [OPTIONS]
#
# Examples:
#   ./scripts/run_docker_benchmark.sh google/gemini-3-flash-preview 25 9999
#   ./scripts/run_docker_benchmark.sh openai/gpt-5.2 25 9998
#   ./scripts/run_docker_benchmark.sh openai/gpt-5.2 25 9999 --restart  # Force container restart
#   ./scripts/run_docker_benchmark.sh google/gemini-3-flash-preview 25 9999 --cleanup  # Stop and remove container
# ========================================================================================

REPO_ROOT="$(git rev-parse --show-toplevel)"
BENCHMARK_DIR="$REPO_ROOT/benchmark"

# --- Configuration ---
DEFAULT_MODEL="google/gemini-3-flash-preview"
DEFAULT_SENDER="0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0"
DEFAULT_SAMPLES=25
DEFAULT_PORT=9999

# Override from arguments or env vars
MODEL="${1:-${SMI_MODEL:-$DEFAULT_MODEL}}"
SAMPLES="${2:-$DEFAULT_SAMPLES}"
PORT="${3:-$DEFAULT_PORT}"
SENDER="${SMI_SENDER:-$DEFAULT_SENDER}"
AGENT="${SMI_AGENT:-real-openai-compatible}"

# Parse optional flags
RESTART_CONTAINER=0
CLEANUP_ONLY=0
STATUS_ONLY=0
MODEL_OVERRIDE=""
shift 3  # Shift past positional args
while [[ $# -gt 0 ]]; do
    case "$1" in
        --restart)
            RESTART_CONTAINER=1
            shift
            ;;
        --cleanup)
            CLEANUP_ONLY=1
            shift
            ;;
        --status)
            STATUS_ONLY=1
            shift
            ;;
        --model-override)
            MODEL_OVERRIDE="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [MODEL] [SAMPLES] [PORT] [--restart] [--cleanup] [--status] [--model-override MODEL]"
            exit 1
            ;;
    esac
done

# --- Container Naming ---
# Use consistent naming based on port for easy reuse
CONTAINER_NAME="smi-bench-${PORT}"

# --- Paths & Directories ---
# We use standard directories for persistent storage
RESULTS_HOST="$BENCHMARK_DIR/results"
LOGS_HOST="$BENCHMARK_DIR/logs"
MANIFEST_HOST="$BENCHMARK_DIR/manifests/active_run_manifest.txt"

# Ensure directories exist
mkdir -p "$RESULTS_HOST"
mkdir -p "$LOGS_HOST"
mkdir -p "$(dirname "$MANIFEST_HOST")"

# --- Corpus Detection ---
# Logic to find the real corpus or fall back to the docker test corpus
REAL_CORPUS_PARENT="$REPO_ROOT/../sui-packages/packages"
DOCKER_TEST_CORPUS="$BENCHMARK_DIR/.docker_test_corpus"

if [ -d "$REAL_CORPUS_PARENT/mainnet_most_used" ]; then
   echo "Using full mainnet corpus from: $REAL_CORPUS_PARENT"
   CORPUS_HOST="$REAL_CORPUS_PARENT"
   CONTAINER_CORPUS_PATH="/app/corpus/mainnet_most_used"
else
   echo "Full corpus not found. Using local test corpus..."
   CORPUS_HOST="$DOCKER_TEST_CORPUS"
   CONTAINER_CORPUS_PATH="/app/corpus"
   
   # Ensure fallback exists
   if [ ! -d "$CORPUS_HOST" ]; then
       echo "Creating minimal fallback corpus..."
       mkdir -p "$CORPUS_HOST/0x00/fixture/bytecode_modules"
       echo '{"id": "0x0000000000000000000000000000000000000000000000000000000000000001"}' > "$CORPUS_HOST/0x00/fixture/metadata.json"
       touch "$CORPUS_HOST/0x00/fixture/bytecode_modules/dummy.mv"
   fi
fi

# --- Manifest Setup ---
# Use Top 25 dataset by default
DATASET_SOURCE="$BENCHMARK_DIR/manifests/datasets/type_inhabitation_top25.txt"
if [ -f "$DATASET_SOURCE" ]; then
    cp "$DATASET_SOURCE" "$MANIFEST_HOST"
else
    echo "Warning: Top 25 dataset not found. Using a dummy manifest."
    echo "0x0000000000000000000000000000000000000000000000000000000000000001" > "$MANIFEST_HOST"
fi

# --- Run ID Generation ---
# Unique ID for this specific execution
SAFE_MODEL=$(echo "$MODEL" | tr '/' '_')
RUN_ID="bench_${SAFE_MODEL}_$(date +%Y%m%d_%H%M%S)"
echo "----------------------------------------------------------------"
echo "Starting Benchmark Run"
echo "Run ID:    $RUN_ID"
echo "Model:     $MODEL"
echo "Samples:   $SAMPLES"
echo "Results:   $RESULTS_HOST/$RUN_ID.json"
echo "Logs:      $LOGS_HOST/$RUN_ID/"
echo "----------------------------------------------------------------"

# --- Docker Setup ---
echo "Checking Docker image..."
if [[ "$(docker images -q smi-bench:latest 2> /dev/null)" == "" ]]; then
    echo "Building smi-bench:latest..."
    docker build -t smi-bench:latest "$REPO_ROOT"
fi

# --- Container Management Functions ---

get_existing_container() {
    docker ps -a --filter "name=${CONTAINER_NAME}" --filter "ancestor=smi-bench:latest" -q
}

is_container_running() {
    local cid="$1"
    [ -n "$cid" ] && docker inspect --format='{{.State.Running}}' "$cid" 2>/dev/null | grep -q "true"
}

stop_container() {
    local cid="$1"
    if is_container_running "$cid"; then
        echo "Stopping container $CONTAINER_NAME..."
        docker stop "$cid" > /dev/null 2>&1
    fi
}

remove_container() {
    local cid="$1"
    if [ -n "$cid" ]; then
        echo "Removing container $CONTAINER_NAME..."
        docker rm -f "$cid" > /dev/null 2>&1 || true
    fi
}

restart_container() {
    local cid="$1"
    echo "Restarting container $CONTAINER_NAME..."
    docker restart "$cid"
}

# --- Cleanup Mode ---
if [ $CLEANUP_ONLY -eq 1 ]; then
    echo "Running cleanup for container on port $PORT..."
    EXISTING_CID=$(get_existing_container)
    if [ -n "$EXISTING_CID" ]; then
        remove_container "$EXISTING_CID"
        echo "Container stopped and removed successfully."
    else
        echo "No container found for port $PORT."
    fi
    exit 0
fi

# --- Status Mode ---
if [ $STATUS_ONLY -eq 1 ]; then
    echo "Querying status for container on port $PORT..."
    EXISTING_CID=$(get_existing_container)
    if [ -z "$EXISTING_CID" ]; then
        echo "No container found for port $PORT."
        exit 1
    fi

    echo "Container: $CONTAINER_NAME ($EXISTING_CID)"
    if is_container_running "$EXISTING_CID"; then
        echo "Status: RUNNING"
        echo "Port: $PORT"
        echo ""
        echo "Health Check:"
        if curl -s http://localhost:$PORT/health > /dev/null; then
            echo "  Health: OK (endpoint responding)"
            HEALTH_JSON=$(curl -s http://localhost:$PORT/health 2>/dev/null || echo "{}")
            echo "$HEALTH_JSON" | jq . 2>/dev/null || echo "$HEALTH_JSON"
        else
            echo "  Health: DEGRADED (health endpoint not responding)"
        fi
    else
        echo "Status: STOPPED"
    fi
    exit 0
fi

# --- Container Lifecycle Management ---
EXISTING_CID=$(get_existing_container)

# Handle existing container
if [ -n "$EXISTING_CID" ]; then
    echo "Found existing container: $CONTAINER_NAME ($EXISTING_CID)"
    
    if is_container_running "$EXISTING_CID"; then
        if [ $RESTART_CONTAINER -eq 1 ]; then
            restart_container "$EXISTING_CID"
        else
            echo "Reusing existing running container (use --restart to force restart)"
        fi
    else
        echo "Existing container is stopped, starting it..."
        docker start "$EXISTING_CID"
    fi
    
    CONTAINER_ID="$EXISTING_CID"
else
    echo "No existing container found, creating new one..."
    
    # Launch new container (without --rm for reuse)
    CONTAINER_ID=$(docker run -d \
        --name "$CONTAINER_NAME" \
        --env-file "$BENCHMARK_DIR/.env" \
        -e SMI_MODEL="$MODEL" \
        ${SMI_MAX_TOKENS:+-e SMI_MAX_TOKENS="$SMI_MAX_TOKENS"} \
        -v "$CORPUS_HOST:/app/corpus" \
        -v "$MANIFEST_HOST:/app/manifest.txt" \
        -v "$RESULTS_HOST:/app/results" \
        -v "$LOGS_HOST:/app/benchmark/logs" \
        -v "$BENCHMARK_DIR/src:/app/benchmark/src" \
        -p $PORT:9999 \
        smi-bench:latest)
fi

# function cleanup {
#     echo "Stopping container..."
#     docker stop "$CONTAINER_ID" > /dev/null 2>&1 || true
# }
# trap cleanup EXIT

echo "Waiting for service health on port $PORT..."
for i in {1..30}; do
    if curl -s http://localhost:$PORT/health > /dev/null; then
        break
    fi
    sleep 1
done

echo "Submitting task to localhost:$PORT..."
# JSON-RPC payload
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
          "text": "{\"config\": { \"corpus_root\": \"$CONTAINER_CORPUS_PATH\", \"package_ids_file\": \"/app/manifest.txt\", \"agent\": \"$AGENT\", \"samples\": $SAMPLES, \"simulation_mode\": \"dry-run\", \"run_id\": \"$RUN_ID\", \"continue_on_error\": true, \"resume\": false, \"sender\": \"$SENDER\", \"checkpoint_every\": 1${MODEL_OVERRIDE:+, \"model\": \"$MODEL_OVERRIDE\"}}, \"out_dir\": \"/app/results\"}"
        }
      ]
    }
  }
}
EOF
)

if [ -n "$MODEL_OVERRIDE" ]; then
    echo "Model override active: $MODEL_OVERRIDE"
fi

curl -s -X POST http://localhost:$PORT/ -H "Content-Type: application/json" -d "$PAYLOAD" > /dev/null

echo "Benchmark running. Tailing logs (Ctrl+C to detach, run continues)..."
docker logs -f "$CONTAINER_ID"
