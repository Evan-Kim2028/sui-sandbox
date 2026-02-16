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

has_required_api() {
  "$1" - <<'PY' >/dev/null 2>&1
import sui_sandbox
assert hasattr(sui_sandbox, "context_run")
assert hasattr(sui_sandbox, "context_prepare")
assert hasattr(sui_sandbox, "historical_view_from_versions")
PY
}

if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  echo "[python-smoke] interpreter not found: $PYTHON_BIN" >&2
  exit 1
fi

if ! has_required_api "$PYTHON_BIN"; then
  # Fallback to local venv interpreters when available.
  for candidate in \
    "$ROOT/.venv/bin/python" \
    "$ROOT/crates/sui-python/.venv313/bin/python" \
    "$ROOT/crates/sui-python/.venv/bin/python"
  do
    if [[ -x "$candidate" ]] && has_required_api "$candidate"; then
      echo "[python-smoke] using fallback interpreter: $candidate"
      PYTHON_BIN="$candidate"
      break
    fi
  done
fi

if ! has_required_api "$PYTHON_BIN"; then
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
  cat >&2 <<'EOF'
[python-smoke] Required API not found: expected `context_run`, `context_prepare`, and `historical_view_from_versions`.
Make sure the installed extension was built from this workspace revision.
EOF
  if [[ "$PY_MINOR" == "3.14" ]]; then
    cat >&2 <<'EOF'
[python-smoke] Note: PyO3 in this repo currently targets Python <= 3.13.
Use a Python 3.13 virtualenv when building the extension.
EOF
  fi
  exit 1
fi

echo "[python-smoke] Syntax check examples 01/02/03/04"
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/01_walrus_checkpoint.py
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/02_extract_interface.py
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/03_deepbook_context_safety.py
"$PYTHON_BIN" -m py_compile python_sui_sandbox/examples/04_deepbook_margin_state_native.py

echo "[python-smoke] Offline execution check for example 02 (local bytecode)"
BYTECODE_DIR=tests/fixture/build/fixture \
  "$PYTHON_BIN" python_sui_sandbox/examples/02_extract_interface.py >/dev/null

if [[ "$RUN_NETWORK" == "1" ]]; then
  echo "[python-smoke] Network execution checks for examples 01 and 04"
  "$PYTHON_BIN" python_sui_sandbox/examples/01_walrus_checkpoint.py >/dev/null
  "$PYTHON_BIN" python_sui_sandbox/examples/04_deepbook_margin_state_native.py >/dev/null
fi

echo "[python-smoke] PASS"
