#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

usage() {
  cat <<'EOF'
Usage: scripts/phase1_validation.sh [--quick|--offline]

Profiles:
  --quick    Skip Rust test gates and scan gate, run replay canaries only
  --offline  Skip network replay gates, run local JSON contract/test gates only

Environment:
  SUI_SANDBOX_BIN        Path to CLI binary (default: ./target/debug/sui-sandbox)
  CANARY_DIGEST          Walrus canary digest (default: g4cJ7a...Fnn)
  CANARY_CHECKPOINT      Walrus canary checkpoint (default: 239615933)
  UPGRADE_DIGEST         Offline export canary digest (default: same as CANARY_DIGEST)
  UPGRADE_CHECKPOINT     Offline export canary checkpoint (default: same as CANARY_CHECKPOINT)
  SCAN_COUNT             Latest checkpoints to scan (default: 1)
  MIN_SCAN_SUCCESS_PCT   Minimum allowed scan pass rate (default: 35)
  RUN_HYBRID_CANARY      Set to 1 to require hybrid replay canary pass (default: 0)
  GATE_TIMEOUT_SECS      Per-gate timeout in seconds (default: 420)
  SCAN_REQUIRED          Set to 1 to fail when latest-scan command times out/fails (default: 0)
EOF
}

PROFILE="full"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --quick)
      PROFILE="quick"
      shift
      ;;
    --offline)
      PROFILE="offline"
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

SUI_SANDBOX_BIN="${SUI_SANDBOX_BIN:-$ROOT/target/debug/sui-sandbox}"
CANARY_DIGEST="${CANARY_DIGEST:-g4cJ7a5WXmtA8Rmz176pR4e3sN3QFBjfBTrksqBMFnn}"
CANARY_CHECKPOINT="${CANARY_CHECKPOINT:-239615933}"
UPGRADE_DIGEST="${UPGRADE_DIGEST:-$CANARY_DIGEST}"
UPGRADE_CHECKPOINT="${UPGRADE_CHECKPOINT:-$CANARY_CHECKPOINT}"
SCAN_COUNT="${SCAN_COUNT:-1}"
MIN_SCAN_SUCCESS_PCT="${MIN_SCAN_SUCCESS_PCT:-35}"
RUN_HYBRID_CANARY="${RUN_HYBRID_CANARY:-0}"
GATE_TIMEOUT_SECS="${GATE_TIMEOUT_SECS:-420}"
SCAN_REQUIRED="${SCAN_REQUIRED:-0}"

RUN_TESTS=1
RUN_NETWORK=1
RUN_SCAN=1

if [[ "$PROFILE" == "quick" ]]; then
  RUN_TESTS=0
  RUN_SCAN=0
fi

if [[ "$PROFILE" == "offline" ]]; then
  RUN_NETWORK=0
  RUN_SCAN=0
fi

mkdir -p "$ROOT/logs_local/phase1_validation"
STAMP="$(date +%Y%m%d_%H%M%S)"
LOG_FILE="$ROOT/logs_local/phase1_validation/phase1_${STAMP}.log"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sui-sandbox-phase1-XXXXXX")"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

PASS_COUNT=0
FAIL_COUNT=0
FAILURES=()
WARN_COUNT=0
WARNINGS=()

log() {
  printf '[phase1] %s\n' "$*" | tee -a "$LOG_FILE"
}

pass() {
  PASS_COUNT=$((PASS_COUNT + 1))
  log "PASS: $1"
}

fail() {
  FAIL_COUNT=$((FAIL_COUNT + 1))
  FAILURES+=("$1")
  log "FAIL: $1"
}

warn() {
  WARN_COUNT=$((WARN_COUNT + 1))
  WARNINGS+=("$1")
  log "WARN: $1"
}

run_gate() {
  local name="$1"
  shift
  log "==> $name"
  if "$@" 2>&1 | tee -a "$LOG_FILE"; then
    pass "$name"
    return 0
  else
    fail "$name"
    return 1
  fi
}

run_capture_gate() {
  local name="$1"
  local outfile="$2"
  shift 2
  log "==> $name"
  if "$@" 2>&1 | tee "$outfile" | tee -a "$LOG_FILE"; then
    pass "$name"
    return 0
  else
    fail "$name"
    return 1
  fi
}

run_capture_stdout_gate() {
  local name="$1"
  local outfile="$2"
  shift 2
  log "==> $name"
  if "$@" > >(tee "$outfile" | tee -a "$LOG_FILE") 2> >(tee -a "$LOG_FILE" >&2); then
    pass "$name"
    return 0
  else
    fail "$name"
    return 1
  fi
}

run_optional_capture_gate() {
  local name="$1"
  local outfile="$2"
  shift 2
  log "==> $name (optional)"
  if "$@" 2>&1 | tee "$outfile" | tee -a "$LOG_FILE"; then
    pass "$name"
  else
    warn "$name"
  fi
  return 0
}

run_gate_timeout() {
  local timeout_secs="$1"
  shift
  local name="$1"
  shift
  run_gate "$name" timeout --foreground -k 10 "$timeout_secs" "$@"
}

run_capture_gate_timeout() {
  local timeout_secs="$1"
  shift
  local name="$1"
  local outfile="$2"
  shift 2
  run_capture_gate "$name" "$outfile" timeout --foreground -k 10 "$timeout_secs" "$@"
}

run_capture_stdout_gate_timeout() {
  local timeout_secs="$1"
  shift
  local name="$1"
  local outfile="$2"
  shift 2
  run_capture_stdout_gate "$name" "$outfile" timeout --foreground -k 10 "$timeout_secs" "$@"
}

run_optional_gate_timeout() {
  local timeout_secs="$1"
  shift
  local name="$1"
  shift
  log "==> $name (optional)"
  if timeout --foreground -k 10 "$timeout_secs" "$@" 2>&1 | tee -a "$LOG_FILE"; then
    pass "$name"
  else
    warn "$name"
  fi
  return 0
}

run_optional_capture_gate_timeout() {
  local timeout_secs="$1"
  shift
  local name="$1"
  local outfile="$2"
  shift 2
  run_optional_capture_gate "$name" "$outfile" timeout --foreground -k 10 "$timeout_secs" "$@"
}

run_python_gate() {
  local name="$1"
  local input_path="$2"
  local mode="$3"
  local py_out="$TMP_DIR/py_gate_${RANDOM}.log"
  log "==> $name"
  if python3 - "$input_path" "$mode" "$MIN_SCAN_SUCCESS_PCT" >"$py_out" 2>&1 <<'PY'
import json
import re
import sys

path = sys.argv[1]
mode = sys.argv[2]
min_pct = float(sys.argv[3])

if mode == "execution_path_contract":
    data = json.load(open(path))
    ep = data.get("execution_path")
    if not isinstance(ep, dict):
        raise SystemExit("missing execution_path object")
    required_str = ["requested_source", "effective_source", "dependency_fetch_mode"]
    required_bool = [
        "vm_only",
        "allow_fallback",
        "auto_system_objects",
        "fallback_used",
        "dynamic_field_prefetch",
    ]
    required_u64 = ["prefetch_depth", "prefetch_limit", "dependency_packages_fetched", "synthetic_inputs"]
    for key in required_str:
        if not isinstance(ep.get(key), str):
            raise SystemExit(f"execution_path.{key} must be string")
    for key in required_bool:
        if not isinstance(ep.get(key), bool):
            raise SystemExit(f"execution_path.{key} must be bool")
    for key in required_u64:
        if not isinstance(ep.get(key), int):
            raise SystemExit(f"execution_path.{key} must be integer")
    print("execution_path schema OK")
elif mode == "walrus_canary":
    data = json.load(open(path))
    if data.get("local_success") is not True:
        raise SystemExit("walrus canary local_success is not true")
    ep = data.get("execution_path", {})
    if ep.get("requested_source") != "walrus":
        raise SystemExit("walrus canary requested_source != walrus")
    if ep.get("effective_source") != "walrus_checkpoint":
        raise SystemExit("walrus canary effective_source != walrus_checkpoint")
    if data.get("comparison", {}).get("status_match") is not True:
        raise SystemExit("walrus canary status_match != true")
    if ep.get("dependency_packages_fetched", 0) <= 0:
        raise SystemExit("walrus canary fetched zero dependency packages")
    print("walrus canary checks OK")
elif mode == "offline_upgrade_canary":
    data = json.load(open(path))
    if data.get("local_success") is not True:
        raise SystemExit("offline upgrade canary local_success is not true")
    ep = data.get("execution_path", {})
    if ep.get("requested_source") != "json":
        raise SystemExit("offline canary requested_source != json")
    if ep.get("effective_source") != "state_json":
        raise SystemExit("offline canary effective_source != state_json")
    print("offline upgrade canary checks OK")
elif mode == "scan_non_regression":
    raw = open(path).read()
    text = re.sub(r"\x1b\[[0-9;]*m", "", raw)
    if "LOOKUP_FAILED" in text:
        raise SystemExit("regression detected: LOOKUP_FAILED present in scan output")
    if "FUNCTION_RESOLUTION_FAILURE" in text:
        raise SystemExit("regression detected: FUNCTION_RESOLUTION_FAILURE present in scan output")
    m = re.search(r"Result:\s*(\d+)\s+passed\s*/\s*(\d+)\s+failed.*\(([\d.]+)%\)", text)
    if not m:
        raise SystemExit("could not parse scan summary result line")
    pct = float(m.group(3))
    if pct < min_pct:
        raise SystemExit(f"scan success rate {pct:.1f}% below threshold {min_pct:.1f}%")
    print(f"scan non-regression checks OK ({pct:.1f}% >= {min_pct:.1f}%)")
else:
    raise SystemExit(f"unknown validation mode: {mode}")
PY
  then
    cat "$py_out" | tee -a "$LOG_FILE"
    pass "$name"
    return 0
  else
    cat "$py_out" | tee -a "$LOG_FILE"
    fail "$name"
    return 1
  fi
}

log "Profile=$PROFILE"
log "Binary=$SUI_SANDBOX_BIN"
log "Log=$LOG_FILE"
log "GateTimeoutSecs=$GATE_TIMEOUT_SECS"
log "ScanRequired=$SCAN_REQUIRED"

if [[ ! -x "$SUI_SANDBOX_BIN" ]]; then
  run_gate "build sui-sandbox binary" cargo build --bin sui-sandbox
fi

if [[ "$RUN_TESTS" -eq 1 ]]; then
  run_gate "relocation regression unit test" \
    cargo test -p sui-sandbox-core test_relocate_keeps_runtime_id_for_self_calls -- --nocapture
  run_gate "replay execution_path contract test" \
    cargo test --test sandbox_cli_tests test_replay_json_output_execution_path_contract_from_state_json -- --nocapture
  run_gate "replay auto-system-objects bool test" \
    cargo test --test sandbox_cli_tests test_replay_auto_system_objects_explicit_bool_true_false -- --nocapture
fi

MINIMAL_STATE_JSON="$TMP_DIR/minimal_state.json"
cat > "$MINIMAL_STATE_JSON" <<'JSON'
{
  "transaction": {
    "digest": "dummy_digest",
    "sender": "0x1",
    "gas_budget": 1000000,
    "gas_price": 1000,
    "commands": [],
    "inputs": [],
    "effects": null,
    "timestamp_ms": null,
    "checkpoint": null
  },
  "objects": {},
  "packages": {},
  "protocol_version": 64,
  "epoch": 0,
  "reference_gas_price": null,
  "checkpoint": null
}
JSON

OFFLINE_CONTRACT_OUT="$TMP_DIR/offline_contract.json"
if run_capture_stdout_gate "offline state-json replay contract run" \
  "$OFFLINE_CONTRACT_OUT" \
  "$SUI_SANDBOX_BIN" --json replay anydigest --state-json "$MINIMAL_STATE_JSON"; then
  run_python_gate "offline execution_path schema check" "$OFFLINE_CONTRACT_OUT" "execution_path_contract"
else
  warn "offline execution_path schema check (skipped: state-json run failed)"
fi

if [[ "$RUN_NETWORK" -eq 1 ]]; then
  WALRUS_CANARY_OUT="$TMP_DIR/walrus_canary.json"
  if run_capture_stdout_gate_timeout "$GATE_TIMEOUT_SECS" "walrus replay canary" \
    "$WALRUS_CANARY_OUT" \
    env SUI_DF_STRICT_CHECKPOINT=1 "$SUI_SANDBOX_BIN" --json replay "$CANARY_DIGEST" --source walrus --checkpoint "$CANARY_CHECKPOINT" --compare; then
    run_python_gate "walrus canary semantic checks" "$WALRUS_CANARY_OUT" "walrus_canary"
  else
    warn "walrus canary semantic checks (skipped: replay canary failed)"
  fi

  EXPORTED_STATE="$TMP_DIR/upgrade_canary_state.json"
  run_optional_gate_timeout "$GATE_TIMEOUT_SECS" "export upgrade canary state" \
    env SUI_DF_STRICT_CHECKPOINT=1 "$SUI_SANDBOX_BIN" replay "$UPGRADE_DIGEST" --source walrus --checkpoint "$UPGRADE_CHECKPOINT" --export-state "$EXPORTED_STATE"

  if [[ -f "$EXPORTED_STATE" ]]; then
    OFFLINE_UPGRADE_OUT="$TMP_DIR/offline_upgrade_canary.json"
    if run_capture_stdout_gate "offline replay from exported upgrade canary state" \
      "$OFFLINE_UPGRADE_OUT" \
      "$SUI_SANDBOX_BIN" --json replay "$UPGRADE_DIGEST" --state-json "$EXPORTED_STATE"; then
      run_python_gate "offline upgrade canary checks" "$OFFLINE_UPGRADE_OUT" "offline_upgrade_canary"
    else
      warn "offline upgrade canary checks (skipped: exported-state replay failed)"
    fi
  else
    warn "offline replay from exported upgrade canary state (skipped: export file missing)"
  fi

  if [[ "$RUN_HYBRID_CANARY" == "1" ]]; then
    run_gate_timeout "$GATE_TIMEOUT_SECS" "hybrid replay canary (archive endpoint required)" \
      "$SUI_SANDBOX_BIN" replay "$CANARY_DIGEST" --source hybrid --compare
  fi
fi

if [[ "$RUN_SCAN" -eq 1 ]]; then
  SCAN_OUT="$TMP_DIR/latest_scan.log"
  if [[ "$SCAN_REQUIRED" == "1" ]]; then
    run_capture_gate_timeout "$GATE_TIMEOUT_SECS" "latest checkpoint scan" \
      "$SCAN_OUT" \
      env SUI_DF_STRICT_CHECKPOINT=1 "$SUI_SANDBOX_BIN" replay "*" --source walrus --latest "$SCAN_COUNT" --compare
  else
    run_optional_capture_gate_timeout "$GATE_TIMEOUT_SECS" "latest checkpoint scan" \
      "$SCAN_OUT" \
      env SUI_DF_STRICT_CHECKPOINT=1 "$SUI_SANDBOX_BIN" replay "*" --source walrus --latest "$SCAN_COUNT" --compare
  fi
  if rg -q "Result:" "$SCAN_OUT"; then
    run_python_gate "scan non-regression checks" "$SCAN_OUT" "scan_non_regression"
  else
    warn "scan non-regression checks (skipped: no scan summary)"
  fi
fi

log "----------------------------------------"
log "Validation complete: pass=$PASS_COUNT fail=$FAIL_COUNT warn=$WARN_COUNT"
if [[ "$FAIL_COUNT" -gt 0 ]]; then
  log "Failed gates:"
  for gate in "${FAILURES[@]}"; do
    log "  - $gate"
  done
  log "See log for details: $LOG_FILE"
  exit 1
fi

if [[ "$WARN_COUNT" -gt 0 ]]; then
  log "Warning gates:"
  for gate in "${WARNINGS[@]}"; do
    log "  - $gate"
  done
fi

log "All gates passed."
