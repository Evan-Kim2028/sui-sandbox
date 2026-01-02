#!/usr/bin/env bash
set -euo pipefail

# Run Phase II in sequential batches (e.g., 5 at a time) against a fixed id manifest + PTB planfile.
#
# Usage:
#   ./scripts/run_phase2_manifest_batches.sh <CORPUS_ROOT> <MANIFEST_IDS_FILE> <PLAN_FILE> <SENDER> <OUT_JSON>
#
# Example:
#   ./scripts/run_phase2_manifest_batches.sh \
#     /path/to/sui-packages/packages/mainnet_most_used \
#     results/manifests/phase2_executable_ids_n1000.txt \
#     results/manifests/phase2_executable_plans_n1000.json \
#     0xYOUR_MAINNET_ADDRESS \
#     results/phase2_exec_subset_glm47.json

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

CORPUS_ROOT="${1:?missing CORPUS_ROOT}"
MANIFEST_IDS_FILE="${2:?missing MANIFEST_IDS_FILE}"
PLAN_FILE="${3:?missing PLAN_FILE}"
SENDER="${4:?missing SENDER}"
OUT_JSON="${5:?missing OUT_JSON}"

if [[ "${CORPUS_ROOT}" == "-h" || "${CORPUS_ROOT}" == "--help" ]]; then
  echo "Usage: $0 <CORPUS_ROOT> <MANIFEST_IDS_FILE> <PLAN_FILE> <SENDER> <OUT_JSON>"
  exit 0
fi

# Tuneables via env vars.
BATCH_SIZE="${BATCH_SIZE:-5}"
SEED="${SEED:-0}"
RPC_URL="${RPC_URL:-https://fullnode.mainnet.sui.io:443}"
GAS_BUDGET="${GAS_BUDGET:-10000000}"
GAS_COIN="${GAS_COIN:-}"
PER_PACKAGE_TIMEOUT_SECONDS="${PER_PACKAGE_TIMEOUT_SECONDS:-120}"
MAX_ERRORS="${MAX_ERRORS:-200}"
REQUIRE_DRY_RUN="${REQUIRE_DRY_RUN:-1}"
SIMULATION_MODE="${SIMULATION_MODE:-dry-run}"
SLEEP_BETWEEN_BATCHES_SECONDS="${SLEEP_BETWEEN_BATCHES_SECONDS:-0}"

if command -v uv >/dev/null 2>&1; then
  RUNNER=(uv run smi-inhabit)
else
  RUNNER=(env PYTHONPATH=src python3 -m smi_bench.inhabit_runner)
fi

while true; do
  echo "[$(date -Iseconds)] running Phase II batch size=$BATCH_SIZE (resume -> next batch)..."
  args=(
    --corpus-root "$CORPUS_ROOT"
    --package-ids-file "$MANIFEST_IDS_FILE"
    --samples "$BATCH_SIZE"
    --seed "$SEED"
    --agent mock-planfile
    --plan-file "$PLAN_FILE"
    --rpc-url "$RPC_URL"
    --sender "$SENDER"
    --gas-budget "$GAS_BUDGET"
    --simulation-mode "$SIMULATION_MODE"
    --per-package-timeout-seconds "$PER_PACKAGE_TIMEOUT_SECONDS"
    --continue-on-error
    --max-errors "$MAX_ERRORS"
    --checkpoint-every 1
    --out "$OUT_JSON"
    --resume
  )
  if [[ -n "$GAS_COIN" ]]; then
    args+=(--gas-coin "$GAS_COIN")
  fi
  if [[ "$REQUIRE_DRY_RUN" == "1" && "$SIMULATION_MODE" == "dry-run" ]]; then
    args+=(--require-dry-run)
  fi

  "${RUNNER[@]}" "${args[@]}"

  remaining="$(python3 scripts/manifest_remaining.py --manifest "$MANIFEST_IDS_FILE" --out-json "$OUT_JSON" --remaining-only)"
  if [[ "${remaining}" == "0" ]]; then
    echo "[$(date -Iseconds)] done: $OUT_JSON"
    exit 0
  fi

  if [[ "$SLEEP_BETWEEN_BATCHES_SECONDS" != "0" ]]; then
    sleep "$SLEEP_BETWEEN_BATCHES_SECONDS"
  fi
done
