#!/bin/bash
# walrus_ptb_universe.sh — Build a Walrus PTB universe and execute mock local PTBs.
#
# This is an external-facing flow:
# - Pull latest checkpoints from Walrus
# - Build package/function universe
# - Fetch package closure
# - Generate + execute mock PTBs
# - Emit JSON artifacts
#
# Usage:
#   ./examples/walrus_ptb_universe.sh
#   ./examples/walrus_ptb_universe.sh --latest 10 --top-packages 8 --max-ptbs 20
#   ./examples/walrus_ptb_universe.sh --out-dir /tmp/walrus-ptb-universe

set -euo pipefail

usage() {
    cat <<'HELP'
Walrus PTB universe example.

Usage:
  ./examples/walrus_ptb_universe.sh [OPTIONS]

Options:
  --latest N         Number of latest checkpoints to analyze (default: 10)
  --top-packages N   Number of top packages to fetch (default: 8)
  --max-ptbs N       Max generated PTBs to execute (default: 20)
  --out-dir PATH     Output directory (default: examples/out/walrus_ptb_universe)
  --help             Show this help
HELP
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

echo "=== Walrus PTB Universe ==="
echo "Running example with args: $*"

cargo run --example walrus_ptb_universe -- "$@"

OUT_DIR="examples/out/walrus_ptb_universe"
for ((i=1; i<=$#; i++)); do
    if [[ "${!i}" == "--out-dir" ]]; then
        j=$((i+1))
        OUT_DIR="${!j}"
    fi
done

echo ""
echo "Artifacts:" 
echo "  ${OUT_DIR}/README.md"
echo "  ${OUT_DIR}/universe_summary.json"
echo "  ${OUT_DIR}/package_downloads.json"
echo "  ${OUT_DIR}/function_candidates.json"
echo "  ${OUT_DIR}/ptb_execution_results.json"
echo "  ${OUT_DIR}/ptb_specs/"
