#!/usr/bin/env bash
#!/usr/bin/env bash
set -euo pipefail

# Run a multi-model benchmark suite.
#
# Defaults to Phase II targeted (signal-only packages, targets>=2).
# Use --a2a to run the local A2A pipeline per-model (preflight/smoke/validate).

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

BASE_MANIFEST="manifests/datasets/packages_with_keys.txt"

usage() {
    cat <<'EOF'
Usage:
  ./scripts/run_multi_model.sh --env-file <path> [--phase2-targeted|--a2a] [options]

Required:
  --env-file <path>      Dotenv file to load (required; never falls back to benchmark/.env)

Modes:
  --phase2-targeted      Run Phase II targeted (signal-only, targets>=2) [default]
  --a2a                  Run local A2A pipeline (preflight + smoke + validate)

Options:
  --models "m1,m2"       Comma-separated OpenRouter model ids
  --parallel N           Parallel jobs (default: 2)
  --corpus-root <path>
  --min-targets <int>                 (default: 2)
  --scan-samples <int>                (default: 500)
  --run-samples <int>                 (default: 50)
  --per-package-timeout-seconds <num> (default: 90)
  --max-plan-attempts <int>           (default: 2)
  --rpc-url <url>                     (default: https://fullnode.mainnet.sui.io:443)
  --resume
  --scenario <dir>       (default: scenario_smi) [--a2a]
  --samples <int>        (default: 1)            [--a2a]

Examples:
  # Safe default to avoid RPC rate limits: start with --parallel 1, then increase gradually.
  ./scripts/run_multi_model.sh --env-file .env.openrouter --models "openai/gpt-5.2,google/gemini-3-flash-preview" --parallel 1
  ./scripts/run_multi_model.sh --env-file .env.openrouter --a2a --models "openai/gpt-5.2" --corpus-root <CORPUS_ROOT>
EOF
}

ENV_FILE=""
MODE="phase2-targeted"
MODELS_CSV="openai/gpt-5.2,google/gemini-3-flash-preview"
PARALLEL=2

CORPUS_ROOT=""
MIN_TARGETS=2
SCAN_SAMPLES=500
RUN_SAMPLES=50
PER_PKG_TIMEOUT=90
MAX_PLAN_ATTEMPTS=2
RPC_URL="https://fullnode.mainnet.sui.io:443"
RESUME=0

SCENARIO="scenario_smi"
SAMPLES=1

while [[ $# -gt 0 ]]; do
    case "$1" in
        --env-file) ENV_FILE="$2"; shift 2 ;;
        --phase2-targeted) MODE="phase2-targeted"; shift ;;
        --a2a) MODE="a2a"; shift ;;
        --models) MODELS_CSV="$2"; shift 2 ;;
        --parallel) PARALLEL="$2"; shift 2 ;;
        --corpus-root) CORPUS_ROOT="$2"; shift 2 ;;
        --min-targets) MIN_TARGETS="$2"; shift 2 ;;
        --scan-samples) SCAN_SAMPLES="$2"; shift 2 ;;
        --run-samples) RUN_SAMPLES="$2"; shift 2 ;;
        --per-package-timeout-seconds) PER_PKG_TIMEOUT="$2"; shift 2 ;;
        --max-plan-attempts) MAX_PLAN_ATTEMPTS="$2"; shift 2 ;;
        --rpc-url) RPC_URL="$2"; shift 2 ;;
        --resume) RESUME=1; shift ;;
        --scenario) SCENARIO="$2"; shift 2 ;;
        --samples) SAMPLES="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown arg: $1" >&2; usage; exit 2 ;;
    esac
done

# Optional hard wall-clock cap (seconds). If set, it is passed to the underlying runner(s).
MAX_RUN_SECONDS="${MAX_RUN_SECONDS:-}"

if [ -z "$ENV_FILE" ] || [ ! -f "$ENV_FILE" ]; then
    echo "Error: --env-file is required and must exist" >&2
    usage
    exit 2
fi

if [ ! -f "$BASE_MANIFEST" ]; then
    echo "Error: base manifest not found: $BASE_MANIFEST" >&2
    exit 2
fi

IFS=',' read -r -a MODELS <<< "$MODELS_CSV"
TOTAL_MODELS=${#MODELS[@]}
START_TIME=$(date +%s)

if [ -z "$CORPUS_ROOT" ]; then
    for candidate in \
        "../sui-packages/packages/mainnet_most_used" \
        "../../sui-packages/packages/mainnet_most_used" \
        "$HOME/sui-packages/packages/mainnet_most_used"; do
        if [ -d "$candidate" ]; then
            CORPUS_ROOT="$candidate"
            break
        fi
    done
fi

if [ "$MODE" = "a2a" ] && [ -z "$CORPUS_ROOT" ]; then
    echo "Error: --corpus-root is required for --a2a" >&2
    exit 2
fi

OUTPUT_DIR="results/multi_model_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$OUTPUT_DIR"

echo "Running $MODE on $TOTAL_MODELS models"
echo "env_file=$ENV_FILE"
echo "parallel_jobs=$PARALLEL"
echo "results_dir=$OUTPUT_DIR"
if [ "$MODE" = "phase2-targeted" ]; then
    echo "corpus_root=$CORPUS_ROOT"
    echo "min_targets=$MIN_TARGETS scan_samples=$SCAN_SAMPLES run_samples=$RUN_SAMPLES"
    echo "rpc_url=$RPC_URL per_package_timeout_seconds=$PER_PKG_TIMEOUT max_plan_attempts=$MAX_PLAN_ATTEMPTS"
else
    echo "corpus_root=$CORPUS_ROOT scenario=$SCENARIO samples=$SAMPLES"
fi
echo ""

# Function to run a single model evaluation
run_model() {
    local model="$1"
    local output_dir="$2"

    local filename
    filename=$(echo "$model" | tr '/:' '__')

    echo "[$(date '+%H:%M:%S')] Starting: $model"

    if [ "$MODE" = "a2a" ]; then
        local smoked_out="$output_dir/${filename}.a2a_smoke_response.json"
        local smoke_log="$output_dir/${filename}.a2a_smoke.log"

        SMI_MODEL="$model" \
        SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
        uv run smi-a2a-preflight --scenario "$SCENARIO" --corpus-root "$CORPUS_ROOT" \
            > "$output_dir/${filename}.a2a_preflight.log" 2>&1

        SMI_MODEL="$model" \
        SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
        uv run smi-a2a-smoke \
            --env-file "$ENV_FILE" \
            --parent-pid "$$" \
            $( [ -n "$MAX_RUN_SECONDS" ] && echo --max-run-seconds "$MAX_RUN_SECONDS" ) \
            --scenario "$SCENARIO" \
            --corpus-root "$CORPUS_ROOT" \
            --package-ids-file "$BASE_MANIFEST" \
            --samples "$SAMPLES" \
            --out-response "$smoked_out" \
            > "$smoke_log" 2>&1

        uv run smi-a2a-validate-bundle "$smoked_out" \
            > "$output_dir/${filename}.a2a_validate.log" 2>&1

        echo "[$(date '+%H:%M:%S')] Completed: $model | a2a_smoke=$smoked_out"
        return 0
    fi

    mkdir -p "$output_dir/${filename}"
    local run_log="$output_dir/${filename}/phase2_targeted.log"
    SMI_MODEL="$model" \
    SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
    uv run smi-phase2-targeted-run \
        --env-file "$ENV_FILE" \
        --parent-pid "$$" \
        $( [ -n "$MAX_RUN_SECONDS" ] && echo --max-run-seconds "$MAX_RUN_SECONDS" ) \
        --corpus-root "$CORPUS_ROOT" \
        --base-manifest "$BASE_MANIFEST" \
        --min-targets "$MIN_TARGETS" \
        --scan-samples "$SCAN_SAMPLES" \
        --run-samples "$RUN_SAMPLES" \
        --rpc-url "$RPC_URL" \
        --per-package-timeout-seconds "$PER_PKG_TIMEOUT" \
        --max-plan-attempts "$MAX_PLAN_ATTEMPTS" \
        --out-dir "$output_dir/${filename}" \
        $( [ $RESUME -eq 1 ] && echo --resume ) \
        > "$run_log" 2>&1

    echo "[$(date '+%H:%M:%S')] Completed: $model | outputs=$output_dir/${filename}"
}

export -f run_model
export OUTPUT_DIR
export ENV_FILE MODE CORPUS_ROOT MIN_TARGETS SCAN_SAMPLES RUN_SAMPLES PER_PKG_TIMEOUT MAX_PLAN_ATTEMPTS RPC_URL RESUME
export SCENARIO SAMPLES

# Check if GNU parallel is available
if command -v parallel &> /dev/null; then
    echo "Using GNU parallel with $PARALLEL jobs..."
    echo ""
    printf '%s\n' "${MODELS[@]}" | parallel --line-buffer -j "$PARALLEL" run_model {} "$OUTPUT_DIR"
else
    echo "GNU parallel not found, using background jobs..."
    echo ""

    # Run with background jobs and job control
    running=0
    for model in "${MODELS[@]}"; do
        # Wait if we've hit the parallel limit
        while [ $running -ge $PARALLEL ]; do
            wait -n 2>/dev/null || true
            running=$((running - 1))
        done

        run_model "$model" "$OUTPUT_DIR" &
        running=$((running + 1))
    done

    # Wait for all remaining jobs
    wait
fi

END_TIME=$(date +%s)
TOTAL_ELAPSED=$((END_TIME - START_TIME))
ELAPSED_MINS=$((TOTAL_ELAPSED / 60))
ELAPSED_SECS=$((TOTAL_ELAPSED % 60))

echo ""
echo "=============================================="
echo "All evaluations complete!"
echo "Finished at: $(date)"
echo "Total time: ${ELAPSED_MINS}m ${ELAPSED_SECS}s"
echo "Results saved to $OUTPUT_DIR"
echo "=============================================="

echo ""
echo "Detailed logs in: $OUTPUT_DIR/"
