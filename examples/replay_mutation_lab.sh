#!/bin/bash
# replay_mutation_lab.sh — Thin wrapper over native `sui-sandbox replay mutate`.

set -euo pipefail

LATEST="5"
MAX_TRANSACTIONS="60"
OUT_DIR="examples/out/replay_mutation_lab"
DIGEST=""
CHECKPOINT=""
REPLAY_TIMEOUT_SECS="45"
REPLAY_SOURCE="walrus"
JOBS="1"
RETRIES="0"
KEEP_GOING="false"
DIFFERENTIAL_SOURCE=""
CORPUS_IN=""
CORPUS_OUT=""
FIXTURE=""
TARGETS_FILE=""
DEMO="false"

usage() {
  cat <<'HELP'
Replay Mutation Lab

Usage:
  ./examples/replay_mutation_lab.sh [OPTIONS]

Options:
  --latest N            Latest checkpoint window to scan for candidates (default: 5)
  --max-transactions N  Max transactions to test from that window (default: 60)
  --digest DIGEST       Run lab for one specific digest
  --checkpoint CP       Required with --digest for Walrus replay
  --fixture PATH        Deterministic candidate dataset JSON
  --targets-file PATH   Target list JSON (array or {targets/candidates/discovered})
  --demo                Guided deterministic demo mode
  --out-dir PATH        Output directory (default: examples/out/replay_mutation_lab)
  --replay-timeout N    Per-replay timeout in seconds (default: 45)
  --replay-source SRC   Replay source adapter: walrus|grpc|hybrid (default: walrus)
  --jobs N              Concurrent targets per batch (default: 1)
  --retries N           Retry budget for transient replay failures (default: 0)
  --keep-going          Continue after first fail->heal hit
  --differential-source SRC  Secondary source for heal differential comparison
  --corpus-in PATH      Add targets from a replay-mutate corpus file
  --corpus-out PATH     Write/update corpus file after run
  --help                Show help

Env:
  SUI_SANDBOX_BIN       Optional path to sui-sandbox binary
HELP
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --latest)
      LATEST="${2:-}"
      shift 2
      ;;
    --max-transactions)
      MAX_TRANSACTIONS="${2:-}"
      shift 2
      ;;
    --digest)
      DIGEST="${2:-}"
      shift 2
      ;;
    --checkpoint)
      CHECKPOINT="${2:-}"
      shift 2
      ;;
    --fixture)
      FIXTURE="${2:-}"
      shift 2
      ;;
    --targets-file)
      TARGETS_FILE="${2:-}"
      shift 2
      ;;
    --demo)
      DEMO="true"
      shift
      ;;
    --out-dir)
      OUT_DIR="${2:-}"
      shift 2
      ;;
    --replay-timeout)
      REPLAY_TIMEOUT_SECS="${2:-}"
      shift 2
      ;;
    --replay-source)
      REPLAY_SOURCE="${2:-}"
      shift 2
      ;;
    --jobs)
      JOBS="${2:-}"
      shift 2
      ;;
    --retries)
      RETRIES="${2:-}"
      shift 2
      ;;
    --keep-going)
      KEEP_GOING="true"
      shift
      ;;
    --differential-source)
      DIFFERENTIAL_SOURCE="${2:-}"
      shift 2
      ;;
    --corpus-in)
      CORPUS_IN="${2:-}"
      shift 2
      ;;
    --corpus-out)
      CORPUS_OUT="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument '$1'" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -n "$DIGEST" && -z "$CHECKPOINT" ]]; then
  echo "ERROR: --checkpoint is required when --digest is provided" >&2
  exit 1
fi

if ! [[ "$MAX_TRANSACTIONS" =~ ^[0-9]+$ ]] || [[ "$MAX_TRANSACTIONS" -lt 1 ]]; then
  echo "ERROR: --max-transactions must be a positive integer" >&2
  exit 1
fi

if ! [[ "$REPLAY_TIMEOUT_SECS" =~ ^[0-9]+$ ]] || [[ "$REPLAY_TIMEOUT_SECS" -lt 5 ]]; then
  echo "ERROR: --replay-timeout must be an integer >= 5" >&2
  exit 1
fi

if ! [[ "$JOBS" =~ ^[0-9]+$ ]] || [[ "$JOBS" -lt 1 ]]; then
  echo "ERROR: --jobs must be a positive integer" >&2
  exit 1
fi

if ! [[ "$RETRIES" =~ ^[0-9]+$ ]]; then
  echo "ERROR: --retries must be a non-negative integer" >&2
  exit 1
fi

if [[ "$REPLAY_SOURCE" != "walrus" && "$REPLAY_SOURCE" != "grpc" && "$REPLAY_SOURCE" != "hybrid" ]]; then
  echo "ERROR: --replay-source must be one of: walrus, grpc, hybrid" >&2
  exit 1
fi
if [[ -n "$DIFFERENTIAL_SOURCE" && "$DIFFERENTIAL_SOURCE" != "walrus" && "$DIFFERENTIAL_SOURCE" != "grpc" && "$DIFFERENTIAL_SOURCE" != "hybrid" ]]; then
  echo "ERROR: --differential-source must be one of: walrus, grpc, hybrid" >&2
  exit 1
fi
if [[ -n "$CORPUS_IN" && ! -f "$CORPUS_IN" ]]; then
  echo "ERROR: corpus file not found: $CORPUS_IN" >&2
  exit 1
fi

if [[ -n "$FIXTURE" && ! -f "$FIXTURE" ]]; then
  echo "ERROR: fixture file not found: $FIXTURE" >&2
  exit 1
fi

if [[ -n "$TARGETS_FILE" && ! -f "$TARGETS_FILE" ]]; then
  echo "ERROR: targets file not found: $TARGETS_FILE" >&2
  exit 1
fi

if [[ -n "${SUI_SANDBOX_BIN:-}" ]]; then
  BIN="$SUI_SANDBOX_BIN"
elif [[ -x "./target/debug/sui-sandbox" ]]; then
  BIN="./target/debug/sui-sandbox"
elif [[ -x "./target/release/sui-sandbox" ]]; then
  BIN="./target/release/sui-sandbox"
else
  echo "Building sui-sandbox binary..."
  cargo build --bin sui-sandbox >/dev/null
  BIN="./target/debug/sui-sandbox"
fi

CMD=(
  "$BIN"
  replay
  mutate
  --out-dir "$OUT_DIR"
  --max-transactions "$MAX_TRANSACTIONS"
  --replay-timeout "$REPLAY_TIMEOUT_SECS"
  --replay-source "$REPLAY_SOURCE"
  --jobs "$JOBS"
  --retries "$RETRIES"
)
if [[ "$KEEP_GOING" == "true" ]]; then
  CMD+=(--keep-going)
fi
if [[ -n "$DIFFERENTIAL_SOURCE" ]]; then
  CMD+=(--differential-source "$DIFFERENTIAL_SOURCE")
fi
if [[ -n "$CORPUS_IN" ]]; then
  CMD+=(--corpus-in "$CORPUS_IN")
fi
if [[ -n "$CORPUS_OUT" ]]; then
  CMD+=(--corpus-out "$CORPUS_OUT")
fi

if [[ "$DEMO" == "true" ]]; then
  CMD+=(--demo)
fi

if [[ -n "$DIGEST" ]]; then
  CMD+=(--digest "$DIGEST" --checkpoint "$CHECKPOINT")
elif [[ -n "$TARGETS_FILE" ]]; then
  CMD+=(--targets-file "$TARGETS_FILE")
elif [[ -n "$FIXTURE" ]]; then
  CMD+=(--fixture "$FIXTURE")
elif [[ "$DEMO" != "true" ]]; then
  CMD+=(--latest "$LATEST")
fi

echo "=== Replay Mutation Lab ==="
echo "Binary:            $BIN"
echo "Output root:       $OUT_DIR"
echo "Max transactions:  $MAX_TRANSACTIONS"
echo "Replay timeout:    ${REPLAY_TIMEOUT_SECS}s"
echo "Replay source:     $REPLAY_SOURCE"
echo "Jobs:              $JOBS"
echo "Retries:           $RETRIES"
echo "Keep going:        $KEEP_GOING"
if [[ -n "$DIFFERENTIAL_SOURCE" ]]; then
  echo "Differential src:  $DIFFERENTIAL_SOURCE"
fi
if [[ -n "$DIGEST" ]]; then
  echo "Pinned digest:      $DIGEST"
  echo "Pinned checkpoint:  $CHECKPOINT"
fi
if [[ -n "$FIXTURE" ]]; then
  echo "Fixture dataset:    $FIXTURE"
fi
if [[ -n "$TARGETS_FILE" ]]; then
  echo "Targets file:       $TARGETS_FILE"
fi
if [[ "$DEMO" == "true" ]]; then
  echo "Mode:               demo"
fi
echo ""

"${CMD[@]}"

REPORT_PATH="${OUT_DIR%/}/replay_mutate_report.json"
RUN_DIR=""
if [[ -f "$REPORT_PATH" ]]; then
  RUN_DIR="$(python3 - "$REPORT_PATH" <<'PY'
import json
import sys
from pathlib import Path
report = json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))
records = report.get('run_records') or []
if records and isinstance(records[0], dict):
    run_dir = records[0].get('run_dir')
    if isinstance(run_dir, str):
        print(run_dir)
PY
)"
fi

echo ""
echo "Artifacts:"
echo "  $REPORT_PATH"
if [[ -n "$RUN_DIR" ]]; then
  echo "  $RUN_DIR/README.md"
  echo "  $RUN_DIR/candidate_pool.json"
  echo "  $RUN_DIR/attempts.json"
  echo "  $RUN_DIR/report.json"
fi
