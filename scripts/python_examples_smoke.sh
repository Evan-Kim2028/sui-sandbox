#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

usage() {
  cat <<'EOF'
Usage: scripts/python_examples_smoke.sh [--network]

Modes:
  default    Offline-safe checks only
  --network  Also execute networked Walrus/replay-analyze examples

Requirements:
  - Python interpreter (default: PYTHON_BIN=python3)
  - sui_sandbox Python module installed (PyO3 extension)
EOF
}

RUN_NETWORK=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --network)
      RUN_NETWORK=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

PYTHON_BIN="${PYTHON_BIN:-python3}"

if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  echo "[python-smoke] interpreter not found: $PYTHON_BIN" >&2
  exit 1
fi

if ! "$PYTHON_BIN" -c "import sui_sandbox" >/dev/null 2>&1; then
  # Fallback to local venv interpreters when available.
  for candidate in \
    "$ROOT/crates/sui-python/.venv313/bin/python" \
    "$ROOT/crates/sui-python/.venv/bin/python"
  do
    if [[ -x "$candidate" ]] && "$candidate" -c "import sui_sandbox" >/dev/null 2>&1; then
      echo "[python-smoke] using fallback interpreter: $candidate"
      PYTHON_BIN="$candidate"
      break
    fi
  done
fi

if ! "$PYTHON_BIN" -c "import sui_sandbox" >/dev/null 2>&1; then
  PY_MINOR="$("$PYTHON_BIN" - <<'PY'
import sys
print(f"{sys.version_info.major}.{sys.version_info.minor}")
PY
)"
  cat >&2 <<'EOF'
[python-smoke] sui_sandbox module not importable.
Build/install it first, for example:
  cd crates/sui-python
  pip install maturin
  maturin develop --release
EOF
  if [[ "$PY_MINOR" == "3.14" ]]; then
    cat >&2 <<'EOF'
[python-smoke] Note: PyO3 in this repo currently targets Python <= 3.13.
Use a Python 3.13 virtualenv when building the extension.
EOF
  fi
  exit 1
fi

echo "[python-smoke] Syntax check examples 01/02/03/04/05/07"
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/01_walrus_checkpoint.py
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/02_extract_interface.py
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/03_replay_analyze.py
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/04_workflow_passthrough.py
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/05_workflow_init_template.py
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/07_workflow_auto_from_package.py

echo "[python-smoke] Offline execution check for example 02 (local bytecode)"
"$PYTHON_BIN" python_sui_sandbox/examples/02_extract_interface.py \
  --bytecode-dir tests/fixture/build/fixture \
  --module-limit 1 >/dev/null

echo "[python-smoke] Offline CLI parse checks for examples 01/03/04/05/07"
"$PYTHON_BIN" python_sui_sandbox/examples/01_walrus_checkpoint.py --help >/dev/null
"$PYTHON_BIN" python_sui_sandbox/examples/03_replay_analyze.py --help >/dev/null
"$PYTHON_BIN" python_sui_sandbox/examples/04_workflow_passthrough.py --help >/dev/null
"$PYTHON_BIN" python_sui_sandbox/examples/05_workflow_init_template.py --help >/dev/null
"$PYTHON_BIN" python_sui_sandbox/examples/07_workflow_auto_from_package.py --help >/dev/null

if [[ "$RUN_NETWORK" == "1" ]]; then
  echo "[python-smoke] Network execution checks for examples 01 and 03"
  "$PYTHON_BIN" python_sui_sandbox/examples/01_walrus_checkpoint.py --tx-limit 1 >/dev/null
  "$PYTHON_BIN" python_sui_sandbox/examples/03_replay_analyze.py >/dev/null
fi

echo "[python-smoke] PASS"
