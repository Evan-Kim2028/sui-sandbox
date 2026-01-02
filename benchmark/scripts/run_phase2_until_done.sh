#!/usr/bin/env bash
set -euo pipefail

# Runs Phase II repeatedly until it finishes successfully, resuming from --out if present.
#
# Usage:
#   ./scripts/run_phase2_until_done.sh <CORPUS_ROOT> <MANIFEST_IDS_FILE> <PLAN_FILE> <SENDER> <OUT_JSON>

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
SLEEP_ON_FAIL_SECONDS="${SLEEP_ON_FAIL_SECONDS:-30}"

while true; do
  set +e
  ./scripts/run_phase2_manifest_batches.sh "$CORPUS_ROOT" "$MANIFEST_IDS_FILE" "$PLAN_FILE" "$SENDER" "$OUT_JSON"
  status="$?"
  set -e

  if [[ "$status" -eq 0 ]]; then
    exit 0
  fi

  echo "[$(date -Iseconds)] Phase II exited with status=$status; sleeping ${SLEEP_ON_FAIL_SECONDS}s then retrying..."
  sleep "$SLEEP_ON_FAIL_SECONDS"
done

