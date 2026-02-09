#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

STATE_FILE="${TMPDIR:-/tmp}/sui-sandbox-phase0-state-$$.json"
FLOW_FILE="${TMPDIR:-/tmp}/sui-sandbox-phase0-flow-$$.yaml"

cleanup() {
  rm -f "$STATE_FILE" "$FLOW_FILE"
}
trap cleanup EXIT

cat > "$FLOW_FILE" <<'YAML'
version: 1
name: phase0
steps:
  - command: ["status"]
  - command:
      [
        "publish",
        "tests/fixture/build/fixture",
        "--bytecode-only",
        "--address",
        "fixture=0x100",
      ]
  - command: ["view", "packages"]
YAML

start_ns=$(date +%s%N)

./target/debug/sui-sandbox --state-file "$STATE_FILE" run-flow "$FLOW_FILE" --json || true

end_ns=$(date +%s%N)
elapsed_ms=$(( (end_ns - start_ns) / 1000000 ))

echo "phase0_tfs_ms=$elapsed_ms"
