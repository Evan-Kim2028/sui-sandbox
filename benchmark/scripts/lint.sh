#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

PYTHONPATH=src python3 -m pytest -q

if command -v ruff >/dev/null 2>&1; then
  ruff check .
  ruff format --check .
else
  echo "ruff not installed; skipping ruff check/format"
fi

