#!/usr/bin/env bash
set -euo pipefail

# Run a single-model benchmark workflow.
#
# Defaults to Phase II targeted (signal-only packages, targets>=2).
# Use --a2a to run the local A2A pipeline preflight/smoke/validate.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Check if models.yaml exists
if [ ! -f "models.yaml" ]; then
    echo "Error: models.yaml not found"
    exit 1
fi

usage() {
    cat <<'EOF'
Usage:
  ./scripts/run_model.sh --env-file <path> --model <openrouter_model> [--phase2-targeted|--a2a] [options]

Required:
  --env-file <path>    Dotenv file to load (required; never falls back to benchmark/.env)
  --model <string>     OpenRouter model id (e.g. openai/gpt-5.2)

Modes:
  --phase2-targeted    Run Phase II targeted (signal-only, targets>=2) [default]
  --a2a                Run local A2A pipeline (preflight + smoke + validate)

Phase II targeted options:
  --corpus-root <path>
  --min-targets <int>                 (default: 2)
  --scan-samples <int>                (default: 500)
  --run-samples <int>                 (default: 50)
  --per-package-timeout-seconds <num> (default: 90)
  --max-plan-attempts <int>           (default: 2)
  --rpc-url <url>                     (default: https://fullnode.mainnet.sui.io:443)
  --resume

A2A options:
  --scenario <dir>     (default: scenario_smi)
  --samples <int>      (default: 1)

Examples:
  ./scripts/run_model.sh --env-file .env.openrouter --model openai/gpt-5.2
  ./scripts/run_model.sh --env-file .env.openrouter --model google/gemini-3-flash-preview --a2a --corpus-root <CORPUS_ROOT> --scan-samples 500 --run-samples 999999
EOF
}

ENV_FILE=""
MODEL=""
MODE="phase2-targeted"

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

BASE_MANIFEST="manifests/datasets/packages_with_keys.txt"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --env-file)
            ENV_FILE="$2"; shift 2 ;;
        --model)
            MODEL="$2"; shift 2 ;;
        --phase2-targeted)
            MODE="phase2-targeted"; shift ;;
        --a2a)
            MODE="a2a"; shift ;;
        --corpus-root)
            CORPUS_ROOT="$2"; shift 2 ;;
        --min-targets)
            MIN_TARGETS="$2"; shift 2 ;;
        --scan-samples)
            SCAN_SAMPLES="$2"; shift 2 ;;
        --run-samples)
            RUN_SAMPLES="$2"; shift 2 ;;
        --per-package-timeout-seconds)
            PER_PKG_TIMEOUT="$2"; shift 2 ;;
        --max-plan-attempts)
            MAX_PLAN_ATTEMPTS="$2"; shift 2 ;;
        --rpc-url)
            RPC_URL="$2"; shift 2 ;;
        --resume)
            RESUME=1; shift ;;
        --scenario)
            SCENARIO="$2"; shift 2 ;;
        --samples)
            SAMPLES="$2"; shift 2 ;;
        -h|--help)
            usage; exit 0 ;;
        *)
            echo "Unknown arg: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [ -z "$ENV_FILE" ] || [ ! -f "$ENV_FILE" ]; then
    echo "Error: --env-file is required and must exist" >&2
    usage
    exit 2
fi

if [ -z "$MODEL" ]; then
    echo "Error: --model is required (OpenRouter model id)" >&2
    usage
    exit 2
fi

MODEL_SLUG=$(echo "$MODEL" | tr '/:' '__')
OUT_DIR="results/models/${MODEL_SLUG}"
mkdir -p "$OUT_DIR"

if [ -z "$CORPUS_ROOT" ]; then
    # Determine corpus root (try common locations)
    for candidate in \
        "../../sui-packages/packages/mainnet_most_used" \
        "../../../sui-packages/packages/mainnet_most_used" \
        "$HOME/sui-packages/packages/mainnet_most_used"; do
        if [ -d "$candidate" ]; then
            CORPUS_ROOT="$candidate"
            break
        fi
    done
fi

if [ -z "$CORPUS_ROOT" ]; then
    echo "Error: --corpus-root is required (or clone sui-packages in a common location)" >&2
    exit 2
fi

if [ ! -f "$BASE_MANIFEST" ]; then
    echo "Error: base manifest not found: $BASE_MANIFEST" >&2
    exit 2
fi

# Optional hard wall-clock cap (seconds). If set, it is passed to the underlying runner(s).
MAX_RUN_SECONDS="${MAX_RUN_SECONDS:-}"

if [ "$MODE" = "a2a" ]; then
    echo "Running A2A pipeline with:"
    echo "  env_file=$ENV_FILE"
    echo "  model=$MODEL"
    echo "  corpus_root=$CORPUS_ROOT"
    echo "  scenario=$SCENARIO"
    echo "  samples=$SAMPLES"
    echo "  out_dir=$OUT_DIR"
    echo ""

    # Build a signal-only manifest (targets>=MIN_TARGETS) first, then run the A2A request against that manifest.
    TS=$(date +%Y%m%d_%H%M%S)
    SIGNAL_DIR="$OUT_DIR/a2a_signal_${TS}"
    mkdir -p "$SIGNAL_DIR"

    SCAN_OUT="$SIGNAL_DIR/phase2_targets_scan.json"
    MANIFEST_OUT="$SIGNAL_DIR/manifest_targets_ge${MIN_TARGETS}.txt"
    MANIFEST_2_OUT="$SIGNAL_DIR/manifest_targets_ge${MIN_TARGETS}_first2.txt"
    RESPONSE_OUT="$SIGNAL_DIR/a2a_response.json"
    SMOKE_LOG="$SIGNAL_DIR/a2a.log"

    SMI_MODEL="$MODEL" \
    SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
    uv run smi-inhabit \
        --env-file "$ENV_FILE" \
        --parent-pid "$$" \
        $( [ -n "$MAX_RUN_SECONDS" ] && echo --max-run-seconds "$MAX_RUN_SECONDS" ) \
        --corpus-root "$CORPUS_ROOT" \
        --package-ids-file "$BASE_MANIFEST" \
        --samples "$SCAN_SAMPLES" \
        --agent baseline-search \
        --simulation-mode build-only \
        --continue-on-error \
        --out "$SCAN_OUT"

    SMI_MODEL="$MODEL" \
    SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
    uv run smi-phase2-filter-manifest "$SCAN_OUT" \
        --min-targets "$MIN_TARGETS" \
        --out-manifest "$MANIFEST_OUT"

    # For fast iteration, only run the first 2 packages from the signal manifest.
    # (The full manifest is still written to $MANIFEST_OUT for reproducibility.)
    head -n 2 "$MANIFEST_OUT" > "$MANIFEST_2_OUT"

    SMI_MODEL="$MODEL" \
    SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
    uv run smi-a2a-smoke \
        --env-file "$ENV_FILE" \
        --scenario "$SCENARIO" \
        --corpus-root "$CORPUS_ROOT" \
        --package-ids-file "$MANIFEST_2_OUT" \
        --samples 2 \
        --per-package-timeout-seconds "$PER_PKG_TIMEOUT" \
        --max-plan-attempts "$MAX_PLAN_ATTEMPTS" \
        --rpc-url "$RPC_URL" \
        --out-response "$RESPONSE_OUT" \
        > "$SMOKE_LOG" 2>&1

    SMI_MODEL="$MODEL" \
    SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
    uv run smi-a2a-validate-bundle "$RESPONSE_OUT"

    echo ""
    echo "A2A Phase II run complete"
    echo "  scan_out=$SCAN_OUT"
    echo "  manifest_out=$MANIFEST_OUT"
    echo "  manifest_2_out=$MANIFEST_2_OUT"
    echo "  response_out=$RESPONSE_OUT"
    echo "  log=$SMOKE_LOG"
    exit 0
fi

echo "Running Phase II targeted (signal-only) with:"
echo "  env_file=$ENV_FILE"
echo "  model=$MODEL"
echo "  corpus_root=$CORPUS_ROOT"
echo "  min_targets=$MIN_TARGETS"
echo "  scan_samples=$SCAN_SAMPLES"
echo "  run_samples=$RUN_SAMPLES"
echo "  per_package_timeout_seconds=$PER_PKG_TIMEOUT"
echo "  max_plan_attempts=$MAX_PLAN_ATTEMPTS"
echo "  rpc_url=$RPC_URL"
echo "  out_dir=$OUT_DIR"
echo ""

RUN_JSON="$OUT_DIR/phase2_targeted_run_$(date +%Y%m%d_%H%M%S).json"

CMD=(
    uv run smi-phase2-targeted-run
    --corpus-root "$CORPUS_ROOT"
    --base-manifest "$BASE_MANIFEST"
    --min-targets "$MIN_TARGETS"
    --scan-samples "$SCAN_SAMPLES"
    --run-samples "$RUN_SAMPLES"
    --rpc-url "$RPC_URL"
    --per-package-timeout-seconds "$PER_PKG_TIMEOUT"
    --max-plan-attempts "$MAX_PLAN_ATTEMPTS"
    --out-dir "$OUT_DIR"
)
if [ $RESUME -eq 1 ]; then
    CMD+=(--resume)
fi

SMI_MODEL="$MODEL" \
SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
uv run smi-a2a-validate-bundle --help >/dev/null 2>&1 || true

SMI_MODEL="$MODEL" \
SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
uv run smi-inhabit --help >/dev/null 2>&1 || true

SMI_MODEL="$MODEL" \
SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
uv run smi-phase2-targeted-run --help >/dev/null 2>&1 || true

SMI_MODEL="$MODEL" \
SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
uv run smi-a2a-preflight --help >/dev/null 2>&1 || true

SMI_MODEL="$MODEL" \
SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
uv run smi-a2a-smoke --help >/dev/null 2>&1 || true

SMI_MODEL="$MODEL" \
SMI_API_BASE_URL="https://openrouter.ai/api/v1" \
uv run smi-phase2-targeted-run \
    --env-file "$ENV_FILE" \
    $( [ -n "$MAX_RUN_SECONDS" ] && echo --max-run-seconds "$MAX_RUN_SECONDS" ) \
    --corpus-root "$CORPUS_ROOT" \
    --base-manifest "$BASE_MANIFEST" \
    --min-targets "$MIN_TARGETS" \
    --scan-samples "$SCAN_SAMPLES" \
    --run-samples "$RUN_SAMPLES" \
    --rpc-url "$RPC_URL" \
    --per-package-timeout-seconds "$PER_PKG_TIMEOUT" \
    --max-plan-attempts "$MAX_PLAN_ATTEMPTS" \
    --out-dir "$OUT_DIR" \
    $( [ $RESUME -eq 1 ] && echo --resume )

echo ""
echo "Phase II targeted run complete"
echo "  outputs_dir=$OUT_DIR"
echo "  metrics: python scripts/phase2_metrics.py <run_json>"
