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
#   ./scripts/run_docker_benchmark.sh google/gemini-3-flash-preview 25 9999 --dataset achievable_5  # Use specific dataset
#   ./scripts/run_docker_benchmark.sh google/gemini-3-flash-preview 25 9999 --simulation-mode build-only  # Build-only mode
#
# New v0.4.0 Features:
#   --dataset NAME           Use a named dataset from manifests/datasets/<NAME>.txt
#   --simulation-mode MODE   Set simulation mode: dry-run (default), dev-inspect, build-only
#   --max-plan-attempts N    Max PTB replanning attempts per package (default: 5)
#   --per-package-timeout N  Timeout per package in seconds (default: 120)
#   --single-shot            Disable repair loop - single attempt only
#   --direct                 Run smi-inhabit directly instead of A2A server
# ========================================================================================

REPO_ROOT="$(git rev-parse --show-toplevel)"
BENCHMARK_DIR="$REPO_ROOT/benchmark"

# --- Configuration ---
DEFAULT_MODEL="google/gemini-3-flash-preview"
DEFAULT_SENDER="0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0"
DEFAULT_SAMPLES=25
DEFAULT_PORT=9999
DEFAULT_DATASET="quickstart_top5"
DEFAULT_SIMULATION_MODE="dry-run"
DEFAULT_MAX_PLAN_ATTEMPTS=5
DEFAULT_PER_PACKAGE_TIMEOUT=120

# Override from arguments or env vars
MODEL="${1:-${SMI_MODEL:-$DEFAULT_MODEL}}"
SAMPLES="${2:-$DEFAULT_SAMPLES}"
PORT="${3:-$DEFAULT_PORT}"
SENDER="${SMI_SENDER:-$DEFAULT_SENDER}"
AGENT="${SMI_AGENT:-real-openai-compatible}"

# New configurable options
DATASET="$DEFAULT_DATASET"
SIMULATION_MODE="$DEFAULT_SIMULATION_MODE"
MAX_PLAN_ATTEMPTS="$DEFAULT_MAX_PLAN_ATTEMPTS"
PER_PACKAGE_TIMEOUT="$DEFAULT_PER_PACKAGE_TIMEOUT"
SINGLE_SHOT=0
DIRECT_MODE=0

# Parse optional flags
RESTART_CONTAINER=0
CLEANUP_ONLY=0
STATUS_ONLY=0
MODEL_OVERRIDE=""
shift 3 2>/dev/null || true  # Shift past positional args (if provided)
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
        --dataset)
            DATASET="$2"
            shift 2
            ;;
        --simulation-mode)
            SIMULATION_MODE="$2"
            shift 2
            ;;
        --max-plan-attempts)
            MAX_PLAN_ATTEMPTS="$2"
            shift 2
            ;;
        --per-package-timeout)
            PER_PACKAGE_TIMEOUT="$2"
            shift 2
            ;;
        --single-shot)
            SINGLE_SHOT=1
            shift
            ;;
        --direct)
            DIRECT_MODE=1
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [MODEL] [SAMPLES] [PORT] [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --restart              Force container restart"
            echo "  --cleanup              Stop and remove container"
            echo "  --status               Show container status"
            echo "  --model-override MODEL Override model in JSON payload"
            echo "  --dataset NAME         Use dataset from manifests/datasets/<NAME>.txt"
            echo "  --simulation-mode MODE dry-run, dev-inspect, or build-only"
            echo "  --max-plan-attempts N  Max PTB replanning attempts (default: 5)"
            echo "  --per-package-timeout  Timeout per package in seconds (default: 120)"
            echo "  --single-shot          Single attempt only, no repair loop"
            echo "  --direct               Run smi-inhabit directly (not via A2A server)"
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
# Use specified dataset (default: type_inhabitation_top25)
DATASET_SOURCE="$BENCHMARK_DIR/manifests/datasets/${DATASET}.txt"
if [ -f "$DATASET_SOURCE" ]; then
    echo "Using dataset: $DATASET"
    cp "$DATASET_SOURCE" "$MANIFEST_HOST"
else
    echo "Warning: Dataset '$DATASET' not found at $DATASET_SOURCE"
    echo "Available datasets:"
    ls -1 "$BENCHMARK_DIR/manifests/datasets/"*.txt 2>/dev/null | xargs -n1 basename | sed 's/\.txt$//' | sed 's/^/  - /'
    echo ""
    echo "Falling back to dummy manifest."
    echo "0x0000000000000000000000000000000000000000000000000000000000000001" > "$MANIFEST_HOST"
fi

# --- Run ID Generation ---
# Unique ID for this specific execution
SAFE_MODEL=$(echo "$MODEL" | tr '/' '_')
RUN_ID="bench_${SAFE_MODEL}_$(date +%Y%m%d_%H%M%S)"
echo "================================================================"
echo "Starting Benchmark Run (v0.4.0)"
echo "================================================================"
echo "Run ID:          $RUN_ID"
echo "Model:           $MODEL"
echo "Dataset:         $DATASET"
echo "Samples:         $SAMPLES"
echo "Simulation Mode: $SIMULATION_MODE"
echo "Max Attempts:    $MAX_PLAN_ATTEMPTS"
echo "Timeout/pkg:     ${PER_PACKAGE_TIMEOUT}s"
echo "Single-shot:     $([ $SINGLE_SHOT -eq 1 ] && echo 'yes' || echo 'no')"
echo "Direct mode:     $([ $DIRECT_MODE -eq 1 ] && echo 'yes' || echo 'no')"
echo "Results:         $RESULTS_HOST/$RUN_ID.json"
echo "Logs:            $LOGS_HOST/$RUN_ID/"
echo "================================================================"

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

# --- Direct Mode vs A2A Server Mode ---
if [ $DIRECT_MODE -eq 1 ]; then
    echo "Running in direct mode (smi-inhabit CLI)..."

    # Build single-shot flag
    SINGLE_SHOT_FLAG=""
    if [ $SINGLE_SHOT -eq 1 ]; then
        SINGLE_SHOT_FLAG="--max-plan-attempts 1"
    fi

    # Run smi-inhabit directly instead of A2A server
    docker run --rm \
        --name "smi-direct-${RUN_ID}" \
        --entrypoint "/usr/bin/tini" \
        --env-file "$BENCHMARK_DIR/.env" \
        -e SMI_MODEL="$MODEL" \
        ${SMI_MAX_TOKENS:+-e SMI_MAX_TOKENS="$SMI_MAX_TOKENS"} \
        -v "$CORPUS_HOST:/app/corpus" \
        -v "$MANIFEST_HOST:/app/manifest.txt" \
        -v "$RESULTS_HOST:/app/results" \
        -v "$LOGS_HOST:/app/benchmark/logs" \
        smi-bench:latest -- smi-inhabit \
            --corpus-root "$CONTAINER_CORPUS_PATH" \
            --package-ids-file "/app/manifest.txt" \
            --samples "$SAMPLES" \
            --agent "$AGENT" \
            --simulation-mode "$SIMULATION_MODE" \
            --max-plan-attempts "$MAX_PLAN_ATTEMPTS" \
            --per-package-timeout-seconds "$PER_PACKAGE_TIMEOUT" \
            --out "/app/results/$RUN_ID.json" \
            --run-id "$RUN_ID" \
            --checkpoint-every 1 \
            --continue-on-error \
            $SINGLE_SHOT_FLAG

    echo ""
    echo "================================================================"
    echo "Benchmark complete! Results saved to:"
    echo "  $RESULTS_HOST/$RUN_ID.json"
    echo "================================================================"
else
    echo "Submitting task to localhost:$PORT (A2A server mode)..."
    # Build additional config options
    EXTRA_CONFIG=""
    [ -n "$MODEL_OVERRIDE" ] && EXTRA_CONFIG="$EXTRA_CONFIG, \"model\": \"$MODEL_OVERRIDE\""
    [ "$SIMULATION_MODE" != "dry-run" ] && EXTRA_CONFIG="$EXTRA_CONFIG, \"simulation_mode\": \"$SIMULATION_MODE\""
    [ "$MAX_PLAN_ATTEMPTS" != "5" ] && EXTRA_CONFIG="$EXTRA_CONFIG, \"max_plan_attempts\": $MAX_PLAN_ATTEMPTS"
    [ "$PER_PACKAGE_TIMEOUT" != "120" ] && EXTRA_CONFIG="$EXTRA_CONFIG, \"per_package_timeout_seconds\": $PER_PACKAGE_TIMEOUT"

    # JSON-RPC payload with v0.4.0 features
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
          "text": "{\"config\": { \"corpus_root\": \"$CONTAINER_CORPUS_PATH\", \"package_ids_file\": \"/app/manifest.txt\", \"agent\": \"$AGENT\", \"samples\": $SAMPLES, \"simulation_mode\": \"$SIMULATION_MODE\", \"run_id\": \"$RUN_ID\", \"continue_on_error\": true, \"resume\": false, \"sender\": \"$SENDER\", \"checkpoint_every\": 1, \"max_plan_attempts\": $MAX_PLAN_ATTEMPTS, \"per_package_timeout_seconds\": $PER_PACKAGE_TIMEOUT$EXTRA_CONFIG}, \"out_dir\": \"/app/results\"}"
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
fi
