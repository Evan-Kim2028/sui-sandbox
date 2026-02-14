#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

usage() {
  cat <<'EOF'
Usage: scripts/rust_examples_smoke.sh [--network]

Modes:
  default    Offline-safe compile/run checks
  --network  Also run networked DeepBook examples

Requirements:
  - Rust toolchain
  - For --network: reachable Sui endpoint
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

if ! command -v cargo >/dev/null 2>&1; then
  echo "[rust-smoke] cargo not found" >&2
  exit 1
fi

for ex in \
  ptb_basics \
  state_json_offline_replay \
  deepbook_margin_state \
  deepbook_spot_offline_ptb
do
  echo "[rust-smoke] cargo check --example $ex"
  cargo check --example "$ex" >/dev/null
done

echo "[rust-smoke] offline run check for state_json_offline_replay"
cargo run --quiet --example state_json_offline_replay -- \
  --state-json examples/data/state_json_synthetic_ptb_demo.json >/dev/null

if [[ "$RUN_NETWORK" == "1" ]]; then
  if [[ -z "${SUI_GRPC_ENDPOINT:-}" ]]; then
    export SUI_GRPC_ENDPOINT="https://archive.mainnet.sui.io:443"
  fi
  echo "[rust-smoke] network run check for deepbook_margin_state"
  cargo run --quiet --example deepbook_margin_state >/dev/null
  echo "[rust-smoke] network run check for deepbook_spot_offline_ptb"
  cargo run --quiet --example deepbook_spot_offline_ptb >/dev/null
fi

echo "[rust-smoke] PASS"
