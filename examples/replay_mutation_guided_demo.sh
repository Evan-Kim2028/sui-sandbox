#!/bin/bash
# replay_mutation_guided_demo.sh — One-command guided replay mutation demo.
#
# Uses a pinned fixture candidate set so selection is deterministic and fast.

set -euo pipefail

FIXTURE="examples/data/replay_mutation_fixture_v1.json"
OUT_DIR="examples/out/replay_mutation_guided_demo"
MAX_TRANSACTIONS="4"
REPLAY_TIMEOUT_SECS="35"

usage() {
  cat <<'HELP'
Replay Mutation Guided Demo

Usage:
  ./examples/replay_mutation_guided_demo.sh [OPTIONS]

Options:
  --fixture PATH         Fixture dataset JSON (default: examples/data/replay_mutation_fixture_v1.json)
  --max-transactions N   Max fixture candidates to test (default: 4)
  --replay-timeout N     Per-replay timeout in seconds (default: 35)
  --out-dir PATH         Output root (default: examples/out/replay_mutation_guided_demo)
  --help                 Show help
HELP
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fixture)
      FIXTURE="${2:-}"
      shift 2
      ;;
    --max-transactions)
      MAX_TRANSACTIONS="${2:-}"
      shift 2
      ;;
    --replay-timeout)
      REPLAY_TIMEOUT_SECS="${2:-}"
      shift 2
      ;;
    --out-dir)
      OUT_DIR="${2:-}"
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

if [[ ! -f "$FIXTURE" ]]; then
  echo "ERROR: fixture file not found: $FIXTURE" >&2
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

echo "=== Replay Mutation Guided Demo ==="
echo "Step 1/3: run replay mutation lab on deterministic fixture candidates"
echo "Fixture:           $FIXTURE"
echo "Max candidates:    $MAX_TRANSACTIONS"
echo "Replay timeout:    ${REPLAY_TIMEOUT_SECS}s"
echo "Output root:       $OUT_DIR"
echo ""

./examples/replay_mutation_lab.sh \
  --fixture "$FIXTURE" \
  --max-transactions "$MAX_TRANSACTIONS" \
  --replay-timeout "$REPLAY_TIMEOUT_SECS" \
  --out-dir "$OUT_DIR"

RUN_DIR="$(ls -dt "${OUT_DIR%/}"/run_* 2>/dev/null | head -n 1 || true)"
if [[ -z "$RUN_DIR" ]]; then
  echo "ERROR: unable to find generated run directory under $OUT_DIR" >&2
  exit 1
fi

echo ""
echo "Step 2/3: summarize key result"
python3 - <<'PY' "$RUN_DIR"
import json
import sys
from pathlib import Path

run_dir = Path(sys.argv[1])
report_path = run_dir / "report.json"
attempts_path = run_dir / "attempts.json"

report = json.loads(report_path.read_text(encoding="utf-8"))
attempts = json.loads(attempts_path.read_text(encoding="utf-8"))

print(f"Run dir: {run_dir}")
print(f"Status: {report.get('status')}")
print(f"Candidate source: {report.get('candidate_source')}")
print(f"Transactions tested: {len(attempts)}")

chosen = report.get("chosen")
if isinstance(chosen, dict):
    print("Winning case:")
    if chosen.get("source"):
        print(f"  source={chosen.get('source')}")
    print(f"  digest={chosen.get('digest')}")
    print(f"  checkpoint={chosen.get('checkpoint')}")
    baseline = chosen.get("baseline") or {}
    heal = chosen.get("heal") or {}
    print(f"  baseline_success={baseline.get('local_success')}")
    print(f"  heal_success={heal.get('local_success')}")
    print(f"  heal_synthetic_inputs={heal.get('synthetic_inputs')}")
    print(f"  heal_commands_executed={heal.get('commands_executed')}")
else:
    print("No winning case selected in this run.")
PY

echo ""
echo "Step 3/3: inspect artifacts"
echo "  $RUN_DIR/README.md"
echo "  $RUN_DIR/report.json"
echo "  $RUN_DIR/attempts.json"
echo ""
echo "Next manual drill-down:" 
echo "  sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare"
