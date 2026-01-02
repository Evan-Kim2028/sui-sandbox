#!/usr/bin/env bash
set -euo pipefail

# Run Phase I in sequential batches (e.g., 5 at a time) against a fixed id manifest.
#
# Usage:
#   ./scripts/run_phase1_manifest_batches.sh <CORPUS_ROOT> <MANIFEST_IDS_FILE> <OUT_JSON>
#
# Example:
#   ./scripts/run_phase1_manifest_batches.sh \
#     /path/to/sui-packages/packages/mainnet_most_used \
#     results/manifests/mainnet_most_used_first500_ids.txt \
#     results/phase1_first500_glm47.json

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

CORPUS_ROOT="${1:?missing CORPUS_ROOT}"
MANIFEST_IDS_FILE="${2:?missing MANIFEST_IDS_FILE}"
OUT_JSON="${3:?missing OUT_JSON}"

if [[ "${CORPUS_ROOT}" == "-h" || "${CORPUS_ROOT}" == "--help" ]]; then
  echo "Usage: $0 <CORPUS_ROOT> <MANIFEST_IDS_FILE> <OUT_JSON>"
  exit 0
fi

# Tuneables via env vars.
BATCH_SIZE="${BATCH_SIZE:-5}"
SEED="${SEED:-0}"
MAX_STRUCTS_IN_PROMPT="${MAX_STRUCTS_IN_PROMPT:-50}"
MAX_ERRORS="${MAX_ERRORS:-200}"
PER_PACKAGE_TIMEOUT_SECONDS="${PER_PACKAGE_TIMEOUT_SECONDS:-120}"
SLEEP_BETWEEN_BATCHES_SECONDS="${SLEEP_BETWEEN_BATCHES_SECONDS:-0}"

while true; do
  echo "[$(date -Iseconds)] running batch size=$BATCH_SIZE (resume -> next batch)..."
  uv run smi-bench \
    --corpus-root "$CORPUS_ROOT" \
    --package-ids-file "$MANIFEST_IDS_FILE" \
    --agent real-openai-compatible \
    --samples "$BATCH_SIZE" \
    --seed "$SEED" \
    --max-structs-in-prompt "$MAX_STRUCTS_IN_PROMPT" \
    --per-package-timeout-seconds "$PER_PACKAGE_TIMEOUT_SECONDS" \
    --continue-on-error \
    --max-errors "$MAX_ERRORS" \
    --checkpoint-every 1 \
    --out "$OUT_JSON" \
    --resume

  # Stop when the manifest is exhausted.
  remaining="$(python scripts/manifest_remaining.py --manifest "$MANIFEST_IDS_FILE" --out-json "$OUT_JSON" --remaining-only)"
  if [[ "${remaining}" == "0" ]]; then
    echo "[$(date -Iseconds)] done: $OUT_JSON"
    exit 0
  fi

  if [[ "$SLEEP_BETWEEN_BATCHES_SECONDS" != "0" ]]; then
    sleep "$SLEEP_BETWEEN_BATCHES_SECONDS"
  fi
done
