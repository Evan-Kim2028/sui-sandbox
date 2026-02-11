#!/bin/bash
# entry_function_practical_fuzzer.sh — Practical replay-backed function fuzzer.
#
# Flow:
# 1) Scan recent Walrus checkpoints for real MoveCall targets.
# 2) Optionally ingest those checkpoints into the local Walrus index.
# 3) Replay representative targets in practical mode:
#    - baseline always
#    - heal pass only when baseline fails (default)
# 4) Optional metadata lookup (disabled by default to reduce RPC fan-out).
# 5) Emit coverage + failure taxonomy artifacts.
#
# Usage:
#   ./examples/entry_function_practical_fuzzer.sh
#   ./examples/entry_function_practical_fuzzer.sh --latest 10 --max-targets 30
#   ./examples/entry_function_practical_fuzzer.sh --replay-jobs 4
#   ./examples/entry_function_practical_fuzzer.sh --phase-mode phased
#   ./examples/entry_function_practical_fuzzer.sh --phase-mode single
#   ./examples/entry_function_practical_fuzzer.sh --mutation-budget 5 --mutation-jobs 4
#   ./examples/entry_function_practical_fuzzer.sh --metadata-lookup --include-public
#   ./examples/entry_function_practical_fuzzer.sh --heal-mode always

set -euo pipefail

LATEST="5"
MAX_TRANSACTIONS="120"
MAX_TARGETS="20"
OUT_DIR="examples/out/entry_function_practical_fuzzer"
REPLAY_TIMEOUT_SECS="35"
INCLUDE_PUBLIC="false"
METADATA_LOOKUP="false"
HEAL_MODE="on-failure"
INGEST_CHECKPOINTS="true"
INGEST_CONCURRENCY="4"
DISABLE_GRAPHQL_CHECKPOINT_LOOKUP="true"
REPLAY_JOBS="2"
PHASE_MODE="phased"
PHASE_A_TIMEOUT_SECS="12"
PHASE_A_TARGETS="0"
PHASE_A_INCLUDE_TIMEOUTS="false"
RUN_MUTATIONS="true"
MUTATION_BUDGET="3"
MUTATION_JOBS="0"
STABILITY_RUNS="1"
TYPED_MUTATORS="true"
RUN_MINIMIZATION="true"
MINIMIZE_MAX_TRIALS="12"

usage() {
  cat <<'HELP'
Entry Function Practical Fuzzer

Usage:
  ./examples/entry_function_practical_fuzzer.sh [OPTIONS]

Options:
  --latest N             Latest checkpoint window to scan (default: 5)
  --max-transactions N   Max transactions to inspect from window (default: 120)
  --max-targets N        Max function targets to fuzz (default: 20)
  --replay-jobs N        Parallel replay workers (default: 2)
  --phase-mode MODE      Pipeline: phased|single (default: phased)
  --phase-a-timeout N    Phase A baseline timeout seconds (default: 12)
  --phase-a-targets N    Phase A candidate count (0=auto, default: 0)
  --phase-a-include-timeouts
                         Allow phase-A timeout cases into phase-B shortlist
  --no-mutations         Disable mutation operators/oracle stage
  --mutation-budget N    Number of phase-B targets for mutation stage (default: 3)
  --mutation-jobs N      Parallel mutation workers (0=use replay-jobs, default: 0)
  --stability-runs N     Repeat each mutation operator N times for stability (default: 1)
  --no-typed-mutators    Disable signature-aware pure argument mutation path
  --no-minimize          Disable automatic mutation minimization
  --minimize-max-trials N
                         Max replay trials per minimization attempt (default: 12)
  --replay-timeout N     Per replay timeout in seconds (default: 35)
  --heal-mode MODE       Heal strategy: on-failure|always|off (default: on-failure)
  --metadata-lookup      Enable package/module metadata lookup (slower, more RPC)
  --include-public       Include callable public non-entry functions
  --no-ingest            Skip local Walrus checkpoint ingest prepass
  --ingest-concurrency N Concurrency for fetch checkpoints ingest (default: 4)
  --allow-graphql-checkpoint-lookup
                         Allow GraphQL lookup fallback (checkpoint + package resolution)
  --out-dir PATH         Output directory (default: examples/out/entry_function_practical_fuzzer)
  --help                 Show help

Env:
  SUI_SANDBOX_BIN        Optional path to sui-sandbox binary
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
    --max-targets)
      MAX_TARGETS="${2:-}"
      shift 2
      ;;
    --replay-jobs)
      REPLAY_JOBS="${2:-}"
      shift 2
      ;;
    --phase-mode)
      PHASE_MODE="${2:-}"
      shift 2
      ;;
    --phase-a-timeout)
      PHASE_A_TIMEOUT_SECS="${2:-}"
      shift 2
      ;;
    --phase-a-targets)
      PHASE_A_TARGETS="${2:-}"
      shift 2
      ;;
    --phase-a-include-timeouts)
      PHASE_A_INCLUDE_TIMEOUTS="true"
      shift
      ;;
    --no-mutations)
      RUN_MUTATIONS="false"
      shift
      ;;
    --mutation-budget)
      MUTATION_BUDGET="${2:-}"
      shift 2
      ;;
    --mutation-jobs)
      MUTATION_JOBS="${2:-}"
      shift 2
      ;;
    --stability-runs)
      STABILITY_RUNS="${2:-}"
      shift 2
      ;;
    --no-typed-mutators)
      TYPED_MUTATORS="false"
      shift
      ;;
    --no-minimize)
      RUN_MINIMIZATION="false"
      shift
      ;;
    --minimize-max-trials)
      MINIMIZE_MAX_TRIALS="${2:-}"
      shift 2
      ;;
    --replay-timeout)
      REPLAY_TIMEOUT_SECS="${2:-}"
      shift 2
      ;;
    --heal-mode)
      HEAL_MODE="${2:-}"
      shift 2
      ;;
    --metadata-lookup)
      METADATA_LOOKUP="true"
      shift
      ;;
    --include-public)
      INCLUDE_PUBLIC="true"
      shift
      ;;
    --no-ingest)
      INGEST_CHECKPOINTS="false"
      shift
      ;;
    --ingest-concurrency)
      INGEST_CONCURRENCY="${2:-}"
      shift 2
      ;;
    --allow-graphql-checkpoint-lookup)
      DISABLE_GRAPHQL_CHECKPOINT_LOOKUP="false"
      shift
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

if ! [[ "$LATEST" =~ ^[0-9]+$ ]] || [[ "$LATEST" -lt 1 ]]; then
  echo "ERROR: --latest must be a positive integer" >&2
  exit 1
fi

if ! [[ "$MAX_TRANSACTIONS" =~ ^[0-9]+$ ]] || [[ "$MAX_TRANSACTIONS" -lt 1 ]]; then
  echo "ERROR: --max-transactions must be a positive integer" >&2
  exit 1
fi

if ! [[ "$MAX_TARGETS" =~ ^[0-9]+$ ]] || [[ "$MAX_TARGETS" -lt 1 ]]; then
  echo "ERROR: --max-targets must be a positive integer" >&2
  exit 1
fi

if ! [[ "$REPLAY_JOBS" =~ ^[0-9]+$ ]] || [[ "$REPLAY_JOBS" -lt 1 ]]; then
  echo "ERROR: --replay-jobs must be a positive integer" >&2
  exit 1
fi

if [[ "$PHASE_MODE" != "phased" && "$PHASE_MODE" != "single" ]]; then
  echo "ERROR: --phase-mode must be one of: phased, single" >&2
  exit 1
fi

if ! [[ "$PHASE_A_TIMEOUT_SECS" =~ ^[0-9]+$ ]] || [[ "$PHASE_A_TIMEOUT_SECS" -lt 5 ]]; then
  echo "ERROR: --phase-a-timeout must be an integer >= 5" >&2
  exit 1
fi

if ! [[ "$PHASE_A_TARGETS" =~ ^[0-9]+$ ]]; then
  echo "ERROR: --phase-a-targets must be a non-negative integer" >&2
  exit 1
fi

if ! [[ "$MUTATION_BUDGET" =~ ^[0-9]+$ ]]; then
  echo "ERROR: --mutation-budget must be a non-negative integer" >&2
  exit 1
fi

if ! [[ "$MUTATION_JOBS" =~ ^[0-9]+$ ]]; then
  echo "ERROR: --mutation-jobs must be a non-negative integer" >&2
  exit 1
fi

if ! [[ "$STABILITY_RUNS" =~ ^[0-9]+$ ]] || [[ "$STABILITY_RUNS" -lt 1 ]]; then
  echo "ERROR: --stability-runs must be a positive integer" >&2
  exit 1
fi

if ! [[ "$MINIMIZE_MAX_TRIALS" =~ ^[0-9]+$ ]] || [[ "$MINIMIZE_MAX_TRIALS" -lt 1 ]]; then
  echo "ERROR: --minimize-max-trials must be a positive integer" >&2
  exit 1
fi

if ! [[ "$REPLAY_TIMEOUT_SECS" =~ ^[0-9]+$ ]] || [[ "$REPLAY_TIMEOUT_SECS" -lt 5 ]]; then
  echo "ERROR: --replay-timeout must be an integer >= 5" >&2
  exit 1
fi

if [[ "$HEAL_MODE" != "on-failure" && "$HEAL_MODE" != "always" && "$HEAL_MODE" != "off" ]]; then
  echo "ERROR: --heal-mode must be one of: on-failure, always, off" >&2
  exit 1
fi

if ! [[ "$INGEST_CONCURRENCY" =~ ^[0-9]+$ ]] || [[ "$INGEST_CONCURRENCY" -lt 1 ]]; then
  echo "ERROR: --ingest-concurrency must be a positive integer" >&2
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

STAMP="$(date +%Y%m%d_%H%M%S)"
RUN_DIR="${OUT_DIR%/}/run_${STAMP}"
mkdir -p "$RUN_DIR"

echo "=== Entry Function Practical Fuzzer ==="
echo "Binary:            $BIN"
echo "Run dir:           $RUN_DIR"
echo "Latest window:     $LATEST"
echo "Max transactions:  $MAX_TRANSACTIONS"
echo "Max targets:       $MAX_TARGETS"
echo "Replay jobs:       $REPLAY_JOBS"
echo "Phase mode:        $PHASE_MODE"
echo "Phase A timeout:   ${PHASE_A_TIMEOUT_SECS}s"
echo "Phase A targets:   $PHASE_A_TARGETS (0=auto)"
echo "A include timeouts:$PHASE_A_INCLUDE_TIMEOUTS"
echo "Run mutations:     $RUN_MUTATIONS"
echo "Mutation budget:   $MUTATION_BUDGET"
echo "Mutation jobs:     $MUTATION_JOBS (0=use replay-jobs)"
echo "Stability runs:    $STABILITY_RUNS"
echo "Typed mutators:    $TYPED_MUTATORS"
echo "Auto minimize:     $RUN_MINIMIZATION (max trials=$MINIMIZE_MAX_TRIALS)"
echo "Metadata lookup:   $METADATA_LOOKUP"
echo "Include public:    $INCLUDE_PUBLIC"
echo "Heal mode:         $HEAL_MODE"
echo "Ingest checkpoints:$INGEST_CHECKPOINTS (concurrency=$INGEST_CONCURRENCY)"
echo "Disable GraphQL:   $DISABLE_GRAPHQL_CHECKPOINT_LOOKUP"
echo "Replay timeout:    ${REPLAY_TIMEOUT_SECS}s"

env \
  BIN="$BIN" \
  RUN_DIR="$RUN_DIR" \
  LATEST="$LATEST" \
  MAX_TRANSACTIONS="$MAX_TRANSACTIONS" \
  MAX_TARGETS="$MAX_TARGETS" \
  REPLAY_JOBS="$REPLAY_JOBS" \
  PHASE_MODE="$PHASE_MODE" \
  PHASE_A_TIMEOUT_SECS="$PHASE_A_TIMEOUT_SECS" \
  PHASE_A_TARGETS="$PHASE_A_TARGETS" \
  PHASE_A_INCLUDE_TIMEOUTS="$PHASE_A_INCLUDE_TIMEOUTS" \
  RUN_MUTATIONS="$RUN_MUTATIONS" \
  MUTATION_BUDGET="$MUTATION_BUDGET" \
  MUTATION_JOBS="$MUTATION_JOBS" \
  STABILITY_RUNS="$STABILITY_RUNS" \
  TYPED_MUTATORS="$TYPED_MUTATORS" \
  RUN_MINIMIZATION="$RUN_MINIMIZATION" \
  MINIMIZE_MAX_TRIALS="$MINIMIZE_MAX_TRIALS" \
  METADATA_LOOKUP="$METADATA_LOOKUP" \
  INCLUDE_PUBLIC="$INCLUDE_PUBLIC" \
  HEAL_MODE="$HEAL_MODE" \
  INGEST_CHECKPOINTS="$INGEST_CHECKPOINTS" \
  INGEST_CONCURRENCY="$INGEST_CONCURRENCY" \
  DISABLE_GRAPHQL_CHECKPOINT_LOOKUP="$DISABLE_GRAPHQL_CHECKPOINT_LOOKUP" \
  REPLAY_TIMEOUT_SECS="$REPLAY_TIMEOUT_SECS" \
  python3 - <<'PY'
import json
import os
import re
import subprocess
import sys
import time
import urllib.request
import base64
import hashlib
from collections import Counter
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

BIN = os.environ["BIN"]
RUN_DIR = Path(os.environ["RUN_DIR"])
LATEST = int(os.environ["LATEST"])
MAX_TRANSACTIONS = int(os.environ["MAX_TRANSACTIONS"])
MAX_TARGETS = int(os.environ["MAX_TARGETS"])
REPLAY_JOBS = int(os.environ["REPLAY_JOBS"])
METADATA_LOOKUP = os.environ["METADATA_LOOKUP"].lower() == "true"
INCLUDE_PUBLIC = os.environ["INCLUDE_PUBLIC"].lower() == "true"
HEAL_MODE = os.environ["HEAL_MODE"]
INGEST_CHECKPOINTS = os.environ["INGEST_CHECKPOINTS"].lower() == "true"
INGEST_CONCURRENCY = int(os.environ["INGEST_CONCURRENCY"])
DISABLE_GRAPHQL_CHECKPOINT_LOOKUP = (
    os.environ["DISABLE_GRAPHQL_CHECKPOINT_LOOKUP"].lower() == "true"
)
REPLAY_TIMEOUT_SECS = int(os.environ["REPLAY_TIMEOUT_SECS"])
PHASE_MODE = os.environ["PHASE_MODE"]
PHASE_A_TIMEOUT_SECS = int(os.environ["PHASE_A_TIMEOUT_SECS"])
PHASE_A_TARGETS = int(os.environ["PHASE_A_TARGETS"])
PHASE_A_INCLUDE_TIMEOUTS = os.environ["PHASE_A_INCLUDE_TIMEOUTS"].lower() == "true"
RUN_MUTATIONS = os.environ["RUN_MUTATIONS"].lower() == "true"
MUTATION_BUDGET = int(os.environ["MUTATION_BUDGET"])
MUTATION_JOBS = int(os.environ["MUTATION_JOBS"])
MUTATION_WORKERS = MUTATION_JOBS if MUTATION_JOBS > 0 else REPLAY_JOBS
STABILITY_RUNS = int(os.environ["STABILITY_RUNS"])
TYPED_MUTATORS = os.environ["TYPED_MUTATORS"].lower() == "true"
RUN_MINIMIZATION = os.environ["RUN_MINIMIZATION"].lower() == "true"
MINIMIZE_MAX_TRIALS = int(os.environ["MINIMIZE_MAX_TRIALS"])

CACHING_URL = "https://walrus-sui-archival.mainnet.walrus.space"
STATE_FILE = RUN_DIR / "fuzzer_state.bin"
RAW_DIR = RUN_DIR / "raw"
RAW_DIR.mkdir(parents=True, exist_ok=True)


def write_json(path: Path, obj):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2), encoding="utf-8")


def fetch_latest_checkpoint() -> int:
    url = f"{CACHING_URL}/v1/app_info_for_homepage"
    with urllib.request.urlopen(url, timeout=30) as r:
        data = json.load(r)
    return int(data["latest_checkpoint"])


def fetch_checkpoint(cp: int):
    url = f"{CACHING_URL}/v1/app_checkpoint?checkpoint={cp}&show_content=true"
    with urllib.request.urlopen(url, timeout=90) as r:
        data = json.load(r)
    return data.get("content", {}) if isinstance(data, dict) else {}


def canonical_pkg(pkg: str) -> str:
    s = (pkg or "").lower()
    if s.startswith("0x"):
        s = s[2:]
    s = s.strip()
    if not s:
        s = "0"
    return "0x" + s.rjust(64, "0")


def run_cmd(cmd, timeout, extra_env=None):
    started = time.time()
    timed_out = False
    env = os.environ.copy()
    if extra_env:
        env.update(extra_env)
    try:
        proc = subprocess.run(
            cmd,
            text=True,
            capture_output=True,
            timeout=timeout,
            env=env,
        )
        stdout = proc.stdout.strip()
        stderr = proc.stderr.strip()
        code = proc.returncode
    except subprocess.TimeoutExpired as e:
        timed_out = True
        stdout = (e.stdout or "").strip()
        stderr = (e.stderr or "").strip()
        code = None

    elapsed_ms = int((time.time() - started) * 1000)
    parsed = None
    parse_error = None
    if stdout:
        try:
            parsed = json.loads(stdout)
        except Exception as e:
            parse_error = str(e)

    return {
        "cmd": cmd,
        "exit_code": code,
        "elapsed_ms": elapsed_ms,
        "timed_out": timed_out,
        "stdout": stdout,
        "stderr": stderr,
        "parsed": parsed,
        "parse_error": parse_error,
    }


def classify_failure_plane_from_error(local_error: str, timed_out: bool, local_success):
    if timed_out:
        return "transport"
    if local_success is True:
        return "none"
    err = (local_error or "").lower()
    if not err:
        return "other"
    transport_tokens = [
        "grpc",
        "http",
        "network",
        "transport",
        "connection",
        "deadline",
        "timeout",
        "unavailable",
        "failed to fetch",
        "checkpoint lookup",
        "walrus",
    ]
    if any(tok in err for tok in transport_tokens):
        return "transport"
    vm_tokens = [
        "vmerror",
        "major_status",
        "execution failed",
        "failed_to_deserialize_argument",
        "aborted",
        "moveabort",
    ]
    if any(tok in err for tok in vm_tokens):
        return "vm"
    return "other"


def summarize_replay_result(run, mode: str):
    out = run.get("parsed") if isinstance(run, dict) else None
    success = None
    local_error = None
    commands_executed = None
    synthetic_inputs = 0
    execution_path = {}
    effects = {}
    gas_used = None

    if isinstance(out, dict):
        success = out.get("local_success")
        local_error = out.get("local_error")
        commands_executed = out.get("commands_executed")
        execution_path = out.get("execution_path", {}) or {}
        synthetic_inputs = execution_path.get("synthetic_inputs", 0) or 0
        effects = out.get("effects", {}) or {}
        gas_used = effects.get("gas_used")

    failure_plane = classify_failure_plane_from_error(
        local_error,
        bool(run.get("timed_out", False)),
        success,
    )

    return {
        "mode": mode,
        "exit_code": run.get("exit_code"),
        "elapsed_ms": run.get("elapsed_ms"),
        "timed_out": run.get("timed_out", False),
        "local_success": success,
        "local_error": local_error,
        "commands_executed": commands_executed,
        "synthetic_inputs": synthetic_inputs,
        "gas_used": gas_used,
        "effects_success": effects.get("success") if isinstance(effects, dict) else None,
        "commands_succeeded": (
            effects.get("commands_succeeded") if isinstance(effects, dict) else None
        ),
        "failure_plane": failure_plane,
        "execution_path": execution_path,
    }


latest_cp = fetch_latest_checkpoint()
start_cp = max(0, latest_cp - LATEST + 1)

checkpoint_ingest = {
    "enabled": INGEST_CHECKPOINTS,
    "start_checkpoint": start_cp,
    "end_checkpoint": latest_cp,
    "concurrency": INGEST_CONCURRENCY,
    "ok": False,
}
if INGEST_CHECKPOINTS:
    ingest_cmd = [
        BIN,
        "--json",
        "fetch",
        "checkpoints",
        str(start_cp),
        str(latest_cp),
        "--concurrency",
        str(INGEST_CONCURRENCY),
    ]
    ingest_run = run_cmd(
        ingest_cmd,
        timeout=max(90, LATEST * 30),
        extra_env={"SUI_WALRUS_ENABLED": "1"},
    )
    checkpoint_ingest["exit_code"] = ingest_run.get("exit_code")
    checkpoint_ingest["timed_out"] = ingest_run.get("timed_out", False)
    checkpoint_ingest["stderr_tail"] = (ingest_run.get("stderr") or "")[-1200:]
    if ingest_run.get("exit_code") == 0:
        checkpoint_ingest["ok"] = True
    if isinstance(ingest_run.get("parsed"), dict):
        checkpoint_ingest["result"] = ingest_run["parsed"]

write_json(RUN_DIR / "checkpoint_ingest.json", checkpoint_ingest)

function_counts = Counter()
function_samples = {}
transactions_seen = 0
checkpoint_scan = []

for cp in range(start_cp, latest_cp + 1):
    try:
        content = fetch_checkpoint(cp)
    except Exception as e:
        checkpoint_scan.append({"checkpoint": cp, "ok": False, "error": str(e)})
        continue

    txs = content.get("transactions", []) if isinstance(content, dict) else []
    checkpoint_scan.append({"checkpoint": cp, "ok": True, "transactions": len(txs)})

    for tx in txs:
        digest = (
            tx.get("effects", {})
            .get("V2", {})
            .get("transaction_digest")
        )
        if not isinstance(digest, str) or not digest:
            continue

        v1 = (
            tx.get("transaction", {})
            .get("data", [{}])[0]
            .get("intent_message", {})
            .get("value", {})
            .get("V1", {})
        )
        ptb = v1.get("kind", {}).get("ProgrammableTransaction", {})
        commands = ptb.get("commands", []) if isinstance(ptb, dict) else []

        has_move_call = False
        for cmd in commands:
            move_call = cmd.get("MoveCall") if isinstance(cmd, dict) else None
            if not isinstance(move_call, dict):
                continue

            pkg = canonical_pkg(move_call.get("package"))
            mod = move_call.get("module")
            fn = move_call.get("function")
            if not (isinstance(mod, str) and isinstance(fn, str)):
                continue

            key = (pkg, mod, fn)
            function_counts[key] += 1
            if key not in function_samples:
                function_samples[key] = {
                    "digest": digest,
                    "checkpoint": cp,
                }
            has_move_call = True

        if has_move_call:
            transactions_seen += 1
            if transactions_seen >= MAX_TRANSACTIONS:
                break
    if transactions_seen >= MAX_TRANSACTIONS:
        break

universe_rows = []
for (pkg, mod, fn), count in function_counts.most_common():
    sample = function_samples[(pkg, mod, fn)]
    universe_rows.append(
        {
            "package": pkg,
            "module": mod,
            "function": fn,
            "observed_calls": count,
            "sample_digest": sample["digest"],
            "sample_checkpoint": sample["checkpoint"],
        }
    )

write_json(
    RUN_DIR / "function_universe.json",
    {
        "latest_checkpoint": latest_cp,
        "start_checkpoint": start_cp,
        "transactions_seen": transactions_seen,
        "checkpoint_ingest": checkpoint_ingest,
        "checkpoint_scan": checkpoint_scan,
        "targets": universe_rows,
    },
)

if not universe_rows:
    write_json(
        RUN_DIR / "report.json",
        {
            "status": "no_targets",
            "message": "No MoveCall targets discovered in scan window.",
            "checkpoint_ingest": checkpoint_ingest,
        },
    )
    print("No function targets discovered.")
    sys.exit(1)

# Metadata cache: package/module -> function metadata from view module
fetched_packages = set()
module_meta_cache = {}


def ensure_package_loaded(package: str):
    if package in fetched_packages:
        return {"ok": True, "cached": True}
    cmd = [
        BIN,
        "--state-file",
        str(STATE_FILE),
        "fetch",
        "package",
        package,
        "--with-deps",
    ]
    run = run_cmd(cmd, timeout=max(30, REPLAY_TIMEOUT_SECS * 2))
    ok = run["exit_code"] == 0
    if ok:
        fetched_packages.add(package)
    return {
        "ok": ok,
        "cached": False,
        "exit_code": run["exit_code"],
        "timed_out": run["timed_out"],
        "stderr": run["stderr"][-1000:],
    }


def get_module_meta(package: str, module: str):
    key = f"{package}::{module}"
    if key in module_meta_cache:
        return module_meta_cache[key]

    cmd = [
        BIN,
        "--json",
        "--state-file",
        str(STATE_FILE),
        "view",
        "module",
        key,
    ]
    run = run_cmd(cmd, timeout=max(20, REPLAY_TIMEOUT_SECS))
    meta = {
        "ok": False,
        "functions": {},
        "error": None,
    }
    if run["exit_code"] == 0 and isinstance(run.get("parsed"), dict):
        functions = {}
        for f in run["parsed"].get("functions", []):
            if not isinstance(f, dict):
                continue
            name = f.get("name")
            if isinstance(name, str):
                functions[name] = {
                    "visibility": f.get("visibility"),
                    "is_entry": f.get("is_entry"),
                    "type_params": f.get("type_params"),
                    "params": f.get("params"),
                    "returns": f.get("returns"),
                }
        meta = {
            "ok": True,
            "functions": functions,
            "error": None,
        }
    else:
        meta = {
            "ok": False,
            "functions": {},
            "error": run.get("stderr")[-1000:] if run.get("stderr") else "view module failed",
        }

    module_meta_cache[key] = meta
    return meta


def normalize_move_type_hint(type_name: str):
    if not isinstance(type_name, str):
        return ""
    return "".join(type_name.lower().split())


def classify_move_type_hint(type_name: str):
    t = normalize_move_type_hint(type_name)
    if t in ("bool",):
        return "bool"
    if t in ("u8", "u16", "u32", "u64", "u128", "u256"):
        return t
    if t in ("address",):
        return "address"
    if t.startswith("vector<u8>"):
        return "vector_u8"
    return "unknown"


def infer_pure_hints_from_input(tx_inputs, input_idx):
    if not isinstance(input_idx, int):
        return []
    if not isinstance(tx_inputs, list):
        return []
    if input_idx < 0 or input_idx >= len(tx_inputs):
        return []
    item = tx_inputs[input_idx]
    if not isinstance(item, dict) or item.get("type") != "Pure":
        return []
    encoded = item.get("bytes")
    if not isinstance(encoded, str):
        return []
    try:
        raw = base64.b64decode(encoded, validate=True)
    except Exception:
        return []
    size = len(raw)
    if size == 0:
        return []
    if size == 1:
        return ["bool", "u8"]
    if size == 2:
        return ["u16"]
    if size == 4:
        return ["u32"]
    if size == 8:
        return ["u64"]
    if size == 16:
        return ["u128"]
    if size == 32:
        return ["address", "u256"]
    return ["vector<u8>"]


def build_signature_hints_for_pair(export_info):
    if not TYPED_MUTATORS:
        return {"enabled": False, "hints": {}, "errors": ["typed_mutators_disabled"]}

    state_path = export_info.get("state_path")
    if not state_path:
        return {"enabled": True, "hints": {}, "errors": ["missing_export_path"]}

    try:
        state_doc = json.loads(Path(state_path).read_text(encoding="utf-8"))
    except Exception as e:
        return {"enabled": True, "hints": {}, "errors": [f"state_load_failed: {e}"]}

    tx = state_doc.get("transaction")
    if not isinstance(tx, dict):
        return {"enabled": True, "hints": {}, "errors": ["missing_transaction"]}
    commands = tx.get("commands")
    if not isinstance(commands, list):
        return {"enabled": True, "hints": {}, "errors": ["missing_commands"]}
    tx_inputs = tx.get("inputs")

    hints = {}
    errors = []
    source_counts = {"inferred": 0, "module_typed": 0}
    metadata_enabled = METADATA_LOOKUP
    looked_up = set()

    def add_hints(input_idx, hint_values, source):
        if not isinstance(input_idx, int):
            return
        if not isinstance(hint_values, list):
            return
        for v in hint_values:
            if not isinstance(v, str) or not v:
                continue
            hints.setdefault(input_idx, []).append(v)
            if source in source_counts:
                source_counts[source] += 1

    for cmd in commands:
        if not isinstance(cmd, dict) or cmd.get("type") != "MoveCall":
            continue
        package = canonical_pkg(cmd.get("package"))
        module = cmd.get("module")
        function = cmd.get("function")
        arguments = cmd.get("arguments")
        if not isinstance(module, str) or not isinstance(function, str):
            continue
        if not isinstance(arguments, list):
            continue

        for arg in arguments:
            if not isinstance(arg, dict) or arg.get("type") != "Input":
                continue
            input_idx = arg.get("index")
            add_hints(input_idx, infer_pure_hints_from_input(tx_inputs, input_idx), "inferred")

        if not metadata_enabled:
            continue

        lookup_key = (package, module)
        if lookup_key not in looked_up:
            load_res = ensure_package_loaded(package)
            if not load_res.get("ok"):
                errors.append(
                    f"package_load_failed:{package}:{(load_res.get('stderr') or '')[:120]}"
                )
                looked_up.add(lookup_key)
                continue
            looked_up.add(lookup_key)

        meta = get_module_meta(package, module)
        if not meta.get("ok"):
            errors.append(f"module_meta_failed:{package}::{module}")
            continue
        fn_meta = (meta.get("functions") or {}).get(function)
        if not isinstance(fn_meta, dict):
            errors.append(f"function_meta_missing:{package}::{module}::{function}")
            continue
        params = fn_meta.get("params")
        if not isinstance(params, list):
            continue

        for param_idx, arg in enumerate(arguments):
            if not isinstance(arg, dict) or arg.get("type") != "Input":
                continue
            input_idx = arg.get("index")
            if not isinstance(input_idx, int):
                continue
            if param_idx >= len(params):
                continue
            type_name = params[param_idx]
            if not isinstance(type_name, str):
                continue
            add_hints(input_idx, [type_name], "module_typed")

    dedup_hints = {}
    for idx, values in hints.items():
        seen = []
        for v in values:
            if v not in seen:
                seen.append(v)
        dedup_hints[idx] = seen

    return {
        "enabled": True,
        "hints": dedup_hints,
        "errors": errors[:20],
        "metadata_lookup_enabled": metadata_enabled,
        "source_counts": source_counts,
    }


replay_env = {
    # Keep remote lookup enabled but route checkpoint lookup through gRPC,
    # not GraphQL, to reduce GraphQL bottlenecks.
    "SUI_CHECKPOINT_LOOKUP_REMOTE": "1",
    "SUI_CHECKPOINT_LOOKUP_GRPC": "1",
}
if DISABLE_GRAPHQL_CHECKPOINT_LOOKUP:
    replay_env["SUI_CHECKPOINT_LOOKUP_GRAPHQL"] = "0"
    replay_env["SUI_PACKAGE_LOOKUP_GRAPHQL"] = "0"


def replay_key(digest: str, checkpoint: int, mode: str):
    return f"{digest}|{checkpoint}|{mode}"


def build_replay_cmd(digest: str, checkpoint: int, mode: str):
    cmd = [
        BIN,
        "replay",
        digest,
        "--source",
        "walrus",
        "--checkpoint",
        str(checkpoint),
        "--compare",
        "--json",
    ]
    if mode == "baseline":
        cmd += [
            "--fetch-strategy",
            "eager",
            "--no-prefetch",
            "--allow-fallback",
            "false",
        ]
    else:
        cmd += [
            "--fetch-strategy",
            "full",
            "--allow-fallback",
            "true",
            "--synthesize-missing",
            "--self-heal-dynamic-fields",
        ]
    return cmd


def run_replay_once(
    digest: str,
    checkpoint: int,
    mode: str,
    timeout_secs: int,
    raw_prefix: str,
):
    cmd = build_replay_cmd(digest, checkpoint, mode)
    run = run_cmd(cmd, timeout=timeout_secs, extra_env=replay_env)
    summary = summarize_replay_result(run, mode)

    raw_parts = []
    if raw_prefix:
        raw_parts.append(raw_prefix)
    raw_parts.append(digest)
    raw_parts.append(str(checkpoint))
    raw_parts.append(mode)
    raw_name = "_".join(raw_parts) + ".json"
    write_json(
        RAW_DIR / raw_name,
        {
            "summary": summary,
            "timeout_secs": timeout_secs,
            "cmd": run["cmd"],
            "stdout": run["stdout"],
            "stderr": run["stderr"],
            "parse_error": run.get("parse_error"),
        },
    )
    return summary


def run_replay_batch(items, mode: str, timeout_secs: int, raw_prefix: str):
    results = {}
    seen = set()
    unique = []
    for digest, checkpoint in items:
        key = replay_key(digest, checkpoint, mode)
        if key in seen:
            continue
        seen.add(key)
        unique.append((key, digest, checkpoint))

    if not unique:
        return results

    with ThreadPoolExecutor(max_workers=REPLAY_JOBS) as pool:
        futures = {
            pool.submit(
                run_replay_once,
                digest,
                checkpoint,
                mode,
                timeout_secs,
                raw_prefix,
            ): key
            for key, digest, checkpoint in unique
        }
        for fut in as_completed(futures):
            key = futures[fut]
            try:
                results[key] = fut.result()
            except Exception as e:
                results[key] = {
                    "mode": mode,
                    "exit_code": None,
                    "elapsed_ms": 0,
                    "timed_out": False,
                    "local_success": None,
                    "local_error": f"replay_worker_error: {e}",
                    "commands_executed": None,
                    "synthetic_inputs": 0,
                    "execution_path": {},
                }

    return results


def replay_cmd_with_flags(digest: str, checkpoint: int, extra_flags):
    cmd = [
        BIN,
        "replay",
        digest,
        "--source",
        "walrus",
        "--checkpoint",
        str(checkpoint),
        "--compare",
        "--json",
    ]
    cmd.extend(extra_flags)
    return cmd


def replay_cmd_with_state_json(digest: str, state_json_path: Path, extra_flags):
    cmd = [
        BIN,
        "replay",
        digest,
        "--state-json",
        str(state_json_path),
        "--compare",
        "--json",
    ]
    cmd.extend(extra_flags)
    return cmd


def missing_operator_result(mode: str, reason: str):
    return {
        "mode": mode,
        "exit_code": None,
        "elapsed_ms": 0,
        "timed_out": False,
        "local_success": None,
        "local_error": reason,
        "commands_executed": None,
        "synthetic_inputs": 0,
        "execution_path": {},
    }


def export_state_for_mutation(digest: str, checkpoint: int):
    state_path = RAW_DIR / f"state_base_{digest}_{checkpoint}.json"
    cmd = [
        BIN,
        "replay",
        digest,
        "--source",
        "walrus",
        "--checkpoint",
        str(checkpoint),
        "--compare",
        "--json",
        "--fetch-strategy",
        "full",
        "--allow-fallback",
        "true",
        "--export-state",
        str(state_path),
    ]
    timeout_secs = max(REPLAY_TIMEOUT_SECS * 2, 45)
    run = run_cmd(cmd, timeout=timeout_secs, extra_env=replay_env)
    ok = state_path.exists()
    info = {
        "digest": digest,
        "checkpoint": checkpoint,
        "ok": ok,
        "state_path": str(state_path),
        "timeout_secs": timeout_secs,
        "exit_code": run.get("exit_code"),
        "timed_out": run.get("timed_out", False),
        "stderr_tail": (run.get("stderr") or "")[-1600:],
    }
    write_json(
        RAW_DIR / f"mut_state_export_{digest}_{checkpoint}.json",
        {
            "export": info,
            "cmd": run.get("cmd"),
            "stdout": run.get("stdout"),
            "stderr": run.get("stderr"),
            "parse_error": run.get("parse_error"),
        },
    )
    return info


def export_state_batch(pairs):
    exports = {}
    if not pairs:
        return exports

    with ThreadPoolExecutor(max_workers=MUTATION_WORKERS) as pool:
        futures = {
            pool.submit(export_state_for_mutation, digest, checkpoint): (digest, checkpoint)
            for digest, checkpoint in pairs
        }
        for fut in as_completed(futures):
            digest, checkpoint = futures[fut]
            key = (digest, checkpoint)
            try:
                exports[key] = fut.result()
            except Exception as e:
                exports[key] = {
                    "digest": digest,
                    "checkpoint": checkpoint,
                    "ok": False,
                    "state_path": None,
                    "timeout_secs": max(REPLAY_TIMEOUT_SECS * 2, 45),
                    "exit_code": None,
                    "timed_out": False,
                    "stderr_tail": f"state_export_worker_error: {e}",
                }

    return exports


def mutate_state_type_aware_pure(state_doc):
    tx = state_doc.get("transaction")
    if not isinstance(tx, dict):
        return {"ok": False, "reason": "missing_transaction"}
    inputs = tx.get("inputs")
    if not isinstance(inputs, list):
        return {"ok": False, "reason": "missing_inputs"}

    candidates = []
    for idx, item in enumerate(inputs):
        if not isinstance(item, dict) or item.get("type") != "Pure":
            continue
        encoded = item.get("bytes")
        if not isinstance(encoded, str):
            continue
        try:
            raw = base64.b64decode(encoded, validate=True)
        except Exception:
            continue
        if not raw:
            continue
        numeric_rank = 0 if len(raw) in (1, 2, 4, 8, 16, 32) else 1
        candidates.append((numeric_rank, len(raw), idx, raw, encoded))

    if not candidates:
        return {"ok": False, "reason": "no_pure_inputs"}

    candidates.sort(key=lambda t: (t[0], t[1], t[2]))
    _rank, byte_len, idx, raw, before = candidates[0]
    if byte_len in (1, 2, 4, 8, 16, 32):
        value = int.from_bytes(raw, byteorder="little", signed=False)
        mutated_value = (value + 1) % (1 << (byte_len * 8))
        mutated = mutated_value.to_bytes(byte_len, byteorder="little", signed=False)
        strategy = f"increment_le_u{byte_len * 8}"
    else:
        buf = bytearray(raw)
        buf[0] ^= 0x01
        mutated = bytes(buf)
        strategy = "flip_low_bit"

    if mutated == raw:
        return {"ok": False, "reason": "mutation_no_change"}

    after = base64.b64encode(mutated).decode("ascii")
    inputs[idx]["bytes"] = after
    return {
        "ok": True,
        "changes": [
            {
                "input_index": idx,
                "byte_len": byte_len,
                "strategy": strategy,
                "before_b64": before,
                "after_b64": after,
            }
        ],
    }


def mutate_pure_bytes_by_hint(raw: bytes, hint_kind: str):
    if not raw:
        return None, "empty"
    if hint_kind == "bool":
        if len(raw) >= 1:
            cur = raw[0] & 0x01
            nxt = 0x00 if cur else 0x01
            out = bytes([nxt]) + raw[1:]
            return out, "toggle_bool"
        return None, "bool_len_mismatch"
    if hint_kind in ("u8", "u16", "u32", "u64", "u128", "u256"):
        bit_width = int(hint_kind[1:])
        byte_len = bit_width // 8
        if len(raw) != byte_len:
            return None, f"{hint_kind}_len_mismatch"
        value = int.from_bytes(raw, byteorder="little", signed=False)
        mutated_value = (value + 1) % (1 << bit_width)
        out = mutated_value.to_bytes(byte_len, byteorder="little", signed=False)
        return out, f"increment_le_{hint_kind}"
    if hint_kind == "address":
        if len(raw) != 32:
            return None, "address_len_mismatch"
        buf = bytearray(raw)
        buf[-1] ^= 0x01
        return bytes(buf), "flip_address_low_bit"
    if hint_kind == "vector_u8":
        buf = bytearray(raw)
        buf[-1] ^= 0x01
        return bytes(buf), "flip_vector_u8_tail_bit"
    return None, "unsupported_hint"


def mutate_state_pure_signature_aware(state_doc, signature_hints):
    tx = state_doc.get("transaction")
    if not isinstance(tx, dict):
        return {"ok": False, "reason": "missing_transaction"}
    inputs = tx.get("inputs")
    if not isinstance(inputs, list):
        return {"ok": False, "reason": "missing_inputs"}
    if not isinstance(signature_hints, dict) or not signature_hints:
        return {"ok": False, "reason": "missing_signature_hints"}

    pure_indices = []
    for idx, item in enumerate(inputs):
        if isinstance(item, dict) and item.get("type") == "Pure":
            pure_indices.append(idx)
    if not pure_indices:
        return {"ok": False, "reason": "no_pure_inputs"}

    scored = []
    for idx in pure_indices:
        hints = signature_hints.get(idx) or []
        if not hints:
            continue
        classified = [classify_move_type_hint(h) for h in hints]
        recognized = [c for c in classified if c != "unknown"]
        if not recognized:
            continue
        score = 0
        if any(r.startswith("u") for r in recognized):
            score = 0
        elif "bool" in recognized:
            score = 1
        elif "address" in recognized:
            score = 2
        elif "vector_u8" in recognized:
            score = 3
        scored.append((score, idx, hints, recognized))

    if not scored:
        return {"ok": False, "reason": "no_recognized_signature_hints"}

    scored.sort(key=lambda t: (t[0], t[1]))
    _score, idx, hints, recognized = scored[0]
    hint_priority = {
        "bool": 0,
        "u8": 1,
        "u16": 2,
        "u32": 3,
        "u64": 4,
        "u128": 5,
        "u256": 6,
        "address": 7,
        "vector_u8": 8,
    }
    recognized = sorted(
        list(dict.fromkeys(recognized)),
        key=lambda h: (hint_priority.get(h, 99), h),
    )
    encoded = inputs[idx].get("bytes")
    if not isinstance(encoded, str):
        return {"ok": False, "reason": "pure_input_missing_bytes"}
    try:
        raw = base64.b64decode(encoded, validate=True)
    except Exception:
        return {"ok": False, "reason": "pure_input_invalid_base64"}

    mutated = None
    strategy = None
    selected_hint = None
    for hint_kind in recognized:
        candidate, candidate_strategy = mutate_pure_bytes_by_hint(raw, hint_kind)
        if candidate is None or candidate == raw:
            continue
        mutated = candidate
        strategy = candidate_strategy
        selected_hint = hint_kind
        break
    if mutated is None:
        return {"ok": False, "reason": "no_compatible_signature_hint"}

    after = base64.b64encode(mutated).decode("ascii")
    inputs[idx]["bytes"] = after
    return {
        "ok": True,
        "changes": [
            {
                "input_index": idx,
                "hints": hints,
                "selected_hint_kind": selected_hint,
                "strategy": strategy,
                "before_b64": encoded,
                "after_b64": after,
            }
        ],
    }


def mutate_state_shared_object_substitution(state_doc):
    tx = state_doc.get("transaction")
    if not isinstance(tx, dict):
        return {"ok": False, "reason": "missing_transaction"}
    inputs = tx.get("inputs")
    if not isinstance(inputs, list):
        return {"ok": False, "reason": "missing_inputs"}

    shared = []
    for idx, item in enumerate(inputs):
        if not isinstance(item, dict) or item.get("type") != "SharedObject":
            continue
        object_id = item.get("object_id")
        if not isinstance(object_id, str) or not object_id:
            continue
        shared.append(
            {
                "index": idx,
                "object_id": object_id,
                "initial_shared_version": item.get("initial_shared_version"),
                "mutable": bool(item.get("mutable")),
            }
        )

    if len(shared) < 2:
        return {"ok": False, "reason": "insufficient_shared_inputs"}

    selected = None
    for src in shared:
        for dst in shared:
            if src["index"] == dst["index"] or src["object_id"] == dst["object_id"]:
                continue
            if src["mutable"] == dst["mutable"]:
                selected = (src, dst, "same_mutability")
                break
        if selected:
            break

    if selected is None:
        for src in shared:
            for dst in shared:
                if src["index"] == dst["index"] or src["object_id"] == dst["object_id"]:
                    continue
                selected = (src, dst, "fallback_any_shared")
                break
            if selected:
                break

    if selected is None:
        return {"ok": False, "reason": "no_substitution_pair"}

    src, dst, selection_policy = selected
    src_input = inputs[src["index"]]
    src_input["object_id"] = dst["object_id"]
    if isinstance(dst.get("initial_shared_version"), int):
        src_input["initial_shared_version"] = dst["initial_shared_version"]

    return {
        "ok": True,
        "changes": [
            {
                "input_index": src["index"],
                "selection_policy": selection_policy,
                "from_object_id": src["object_id"],
                "to_object_id": dst["object_id"],
                "from_mutable": src["mutable"],
                "to_mutable": dst["mutable"],
                "from_initial_shared_version": src.get("initial_shared_version"),
                "to_initial_shared_version": dst.get("initial_shared_version"),
            }
        ],
    }


def mutate_state_object_version_skew(state_doc):
    tx = state_doc.get("transaction")
    if not isinstance(tx, dict):
        return {"ok": False, "reason": "missing_transaction"}
    inputs = tx.get("inputs")
    if not isinstance(inputs, list):
        return {"ok": False, "reason": "missing_inputs"}

    shared = []
    for idx, item in enumerate(inputs):
        if not isinstance(item, dict) or item.get("type") != "SharedObject":
            continue
        object_id = item.get("object_id")
        version = item.get("initial_shared_version")
        if not isinstance(object_id, str) or not isinstance(version, int):
            continue
        shared.append(
            {
                "index": idx,
                "object_id": object_id,
                "version": version,
            }
        )

    if not shared:
        return {"ok": False, "reason": "no_shared_input_with_version"}

    shared.sort(
        key=lambda s: (
            0 if s["version"] > 0 else 1,
            s["version"],
            s["index"],
        )
    )
    selected = shared[0]
    old_version = selected["version"]
    new_version = old_version - 1 if old_version > 0 else old_version + 1
    if new_version == old_version:
        return {"ok": False, "reason": "version_mutation_no_change"}

    inputs[selected["index"]]["initial_shared_version"] = new_version

    object_change = None
    objects = state_doc.get("objects")
    if isinstance(objects, dict):
        target = selected["object_id"].lower().strip()
        if target.startswith("0x"):
            target = target[2:]
        target_norm = target.rjust(64, "0")
        for key, value in objects.items():
            if not isinstance(key, str) or not isinstance(value, dict):
                continue
            key_norm = key.lower().strip()
            if key_norm.startswith("0x"):
                key_norm = key_norm[2:]
            key_norm = key_norm.rjust(64, "0")
            if key_norm != target_norm:
                continue
            old_obj_version = value.get("version")
            if isinstance(old_obj_version, int):
                value["version"] = new_version
                object_change = {
                    "object_key": key,
                    "object_version_before": old_obj_version,
                    "object_version_after": new_version,
                }
            break

    change = {
        "input_index": selected["index"],
        "object_id": selected["object_id"],
        "initial_shared_version_before": old_version,
        "initial_shared_version_after": new_version,
    }
    if object_change is not None:
        change.update(object_change)

    return {"ok": True, "changes": [change]}


def mutate_state_input_rewire(state_doc):
    tx = state_doc.get("transaction")
    if not isinstance(tx, dict):
        return {"ok": False, "reason": "missing_transaction"}
    inputs = tx.get("inputs")
    commands = tx.get("commands")
    if not isinstance(inputs, list):
        return {"ok": False, "reason": "missing_inputs"}
    if not isinstance(commands, list):
        return {"ok": False, "reason": "missing_commands"}

    input_kinds = {}
    for idx, item in enumerate(inputs):
        if isinstance(item, dict):
            kind = item.get("type")
            if isinstance(kind, str):
                input_kinds[idx] = kind

    if not input_kinds:
        return {"ok": False, "reason": "no_typed_inputs"}

    by_kind = {}
    for idx, kind in input_kinds.items():
        by_kind.setdefault(kind, []).append(idx)
    for kind in by_kind:
        by_kind[kind].sort()

    for cmd_idx, cmd in enumerate(commands):
        if not isinstance(cmd, dict):
            continue
        args = cmd.get("arguments")
        if not isinstance(args, list):
            continue
        for arg_idx, arg in enumerate(args):
            if not isinstance(arg, dict) or arg.get("type") != "Input":
                continue
            src_idx = arg.get("index")
            if not isinstance(src_idx, int):
                continue
            src_kind = input_kinds.get(src_idx)
            if src_kind is None:
                continue
            candidates = [i for i in by_kind.get(src_kind, []) if i != src_idx]
            if not candidates:
                continue
            dst_idx = candidates[0]
            if dst_idx == src_idx:
                continue
            arg["index"] = dst_idx
            return {
                "ok": True,
                "changes": [
                    {
                        "command_index": cmd_idx,
                        "command_type": cmd.get("type"),
                        "argument_index": arg_idx,
                        "input_kind": src_kind,
                        "input_index_before": src_idx,
                        "input_index_after": dst_idx,
                    }
                ],
            }

    return {"ok": False, "reason": "no_rewire_candidate"}


def build_mutated_state(
    digest: str,
    checkpoint: int,
    operator,
    export_info,
    signature_hints,
    run_idx: int,
):
    state_path_raw = export_info.get("state_path")
    if not state_path_raw:
        return {"ok": False, "reason": "missing_export_path"}
    state_path = Path(state_path_raw)
    if not state_path.exists():
        return {"ok": False, "reason": "export_path_not_found"}

    try:
        state_doc = json.loads(state_path.read_text(encoding="utf-8"))
    except Exception as e:
        return {"ok": False, "reason": f"state_load_failed: {e}"}

    mutator = operator.get("mutator")
    if mutator == "pure_type_aware":
        mutation = mutate_state_type_aware_pure(state_doc)
    elif mutator == "pure_signature_aware":
        mutation = mutate_state_pure_signature_aware(state_doc, signature_hints)
    elif mutator == "shared_object_substitute":
        mutation = mutate_state_shared_object_substitution(state_doc)
    elif mutator == "object_version_skew":
        mutation = mutate_state_object_version_skew(state_doc)
    elif mutator == "input_rewire":
        mutation = mutate_state_input_rewire(state_doc)
    else:
        return {"ok": False, "reason": f"unknown_mutator:{mutator}"}

    if not mutation.get("ok"):
        return {"ok": False, "reason": mutation.get("reason", "mutation_failed")}

    mutated_path = RAW_DIR / f"state_mut_{operator['name']}_{digest}_{checkpoint}_r{run_idx}.json"
    write_json(mutated_path, state_doc)
    return {
        "ok": True,
        "state_path": mutated_path,
        "meta": {
            "mutator": mutator,
            "base_state_path": str(state_path),
            "mutated_state_path": str(mutated_path),
            "changes": mutation.get("changes", []),
        },
    }


MUTATION_OPERATORS = [
    {
        "name": "baseline_repeat",
        "kind": "replay_flags",
        "flags": [
            "--fetch-strategy",
            "eager",
            "--no-prefetch",
            "--allow-fallback",
            "false",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
    {
        "name": "strict_vm",
        "kind": "replay_flags",
        "flags": [
            "--vm-only",
            "--fetch-strategy",
            "eager",
            "--no-prefetch",
            "--allow-fallback",
            "false",
            "--auto-system-objects",
            "false",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
    {
        "name": "heal_aggressive",
        "kind": "replay_flags",
        "flags": [
            "--fetch-strategy",
            "full",
            "--allow-fallback",
            "true",
            "--synthesize-missing",
            "--self-heal-dynamic-fields",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
    {
        "name": "heal_no_prefetch",
        "kind": "replay_flags",
        "flags": [
            "--fetch-strategy",
            "full",
            "--no-prefetch",
            "--allow-fallback",
            "true",
            "--synthesize-missing",
            "--self-heal-dynamic-fields",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
    {
        "name": "state_pure_type_aware",
        "kind": "state_json",
        "mutator": "pure_type_aware",
        "flags": [
            "--vm-only",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
    {
        "name": "state_pure_signature_aware",
        "kind": "state_json",
        "mutator": "pure_signature_aware",
        "flags": [
            "--vm-only",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
    {
        "name": "state_shared_object_substitute",
        "kind": "state_json",
        "mutator": "shared_object_substitute",
        "flags": [
            "--vm-only",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
    {
        "name": "state_object_version_skew",
        "kind": "state_json",
        "mutator": "object_version_skew",
        "flags": [
            "--vm-only",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
    {
        "name": "state_input_rewire",
        "kind": "state_json",
        "mutator": "input_rewire",
        "flags": [
            "--vm-only",
        ],
        "timeout": REPLAY_TIMEOUT_SECS,
    },
]


def run_operator_once(
    digest: str,
    checkpoint: int,
    operator,
    run_idx: int,
    state_exports,
    signature_hints_map,
):
    name = operator["name"]
    kind = operator.get("kind", "replay_flags")
    timeout_secs = int(operator.get("timeout", REPLAY_TIMEOUT_SECS))
    mutation_meta = None
    state_export_meta = None

    if kind == "replay_flags":
        cmd = replay_cmd_with_flags(digest, checkpoint, operator["flags"])
    elif kind == "state_json":
        export_info = state_exports.get((digest, checkpoint))
        state_export_meta = export_info
        if not export_info or not export_info.get("ok"):
            reason = (
                export_info.get("stderr_tail")
                if isinstance(export_info, dict)
                else "missing_export_info"
            )
            result = missing_operator_result(
                name, f"state_export_failed: {reason or 'unknown'}"
            )
            result["state_export"] = export_info
            result["run_idx"] = run_idx
            return result
        signature_hints = signature_hints_map.get((digest, checkpoint), {})
        mutation = build_mutated_state(
            digest,
            checkpoint,
            operator,
            export_info,
            signature_hints,
            run_idx,
        )
        if not mutation.get("ok"):
            result = missing_operator_result(
                name, f"state_mutation_unavailable: {mutation.get('reason')}"
            )
            result["state_export"] = export_info
            result["run_idx"] = run_idx
            return result
        mutation_meta = mutation.get("meta")
        cmd = replay_cmd_with_state_json(
            digest, mutation["state_path"], operator.get("flags", [])
        )
    else:
        return missing_operator_result(name, f"unknown_operator_kind:{kind}")

    run = run_cmd(cmd, timeout=timeout_secs, extra_env=replay_env)
    summary = summarize_replay_result(run, name)
    if mutation_meta is not None:
        summary["mutation_meta"] = mutation_meta
    if state_export_meta is not None:
        summary["state_export"] = state_export_meta
    summary["run_idx"] = run_idx

    raw_name = f"mut_{name}_{digest}_{checkpoint}_r{run_idx}.json"
    write_json(
        RAW_DIR / raw_name,
        {
            "operator": name,
            "run_idx": run_idx,
            "timeout_secs": timeout_secs,
            "summary": summary,
            "cmd": run["cmd"],
            "stdout": run["stdout"],
            "stderr": run["stderr"],
            "parse_error": run.get("parse_error"),
            "mutation_meta": mutation_meta,
            "state_export": state_export_meta,
        },
    )
    return summary


def run_operator_batch(tasks):
    results = {}
    batch_meta = {
        "state_exports_attempted": 0,
        "state_exports_succeeded": 0,
        "signature_hints_attempted": 0,
        "signature_hints_with_entries": 0,
        "signature_hints_inferred": 0,
        "signature_hints_module_typed": 0,
        "stability_runs": STABILITY_RUNS,
    }
    if not tasks:
        return results, batch_meta

    dedup = {}
    for _idx, digest, checkpoint, operator, run_idx in tasks:
        op_name = operator["name"]
        dedup[(digest, checkpoint, op_name, run_idx)] = operator

    operator_by_name = {}
    for (_digest, _checkpoint, op_name, _run_idx), operator in dedup.items():
        operator_by_name[op_name] = operator

    state_pairs = sorted(
        {
            (digest, checkpoint)
            for (digest, checkpoint, _op_name, _run_idx), operator in dedup.items()
            if operator.get("kind") == "state_json"
        }
    )
    batch_meta["state_exports_attempted"] = len(state_pairs)
    state_exports = export_state_batch(state_pairs) if state_pairs else {}
    batch_meta["state_exports_succeeded"] = sum(
        1 for info in state_exports.values() if info.get("ok")
    )

    needs_signature_hints = any(
        op.get("mutator") == "pure_signature_aware"
        for op in operator_by_name.values()
    )
    signature_hints_map = {}
    if needs_signature_hints:
        batch_meta["signature_hints_attempted"] = len(state_pairs)
        for pair in state_pairs:
            export_info = state_exports.get(pair, {})
            hint_result = build_signature_hints_for_pair(export_info)
            signature_hints_map[pair] = hint_result.get("hints", {})
            source_counts = hint_result.get("source_counts") or {}
            if isinstance(source_counts, dict):
                batch_meta["signature_hints_inferred"] += int(
                    source_counts.get("inferred", 0) or 0
                )
                batch_meta["signature_hints_module_typed"] += int(
                    source_counts.get("module_typed", 0) or 0
                )
        batch_meta["signature_hints_with_entries"] = sum(
            1 for hints in signature_hints_map.values() if hints
        )

    base_results = {}
    with ThreadPoolExecutor(max_workers=MUTATION_WORKERS) as pool:
        futures = {
            pool.submit(
                run_operator_once,
                digest,
                checkpoint,
                operator,
                run_idx,
                state_exports,
                signature_hints_map,
            ): (
                digest,
                checkpoint,
                op_name,
                run_idx,
            )
            for (digest, checkpoint, op_name, run_idx), operator in dedup.items()
        }
        for fut in as_completed(futures):
            digest, checkpoint, op_name, run_idx = futures[fut]
            base_key = (digest, checkpoint, op_name, run_idx)
            try:
                base_results[base_key] = fut.result()
            except Exception as e:
                base_results[base_key] = missing_operator_result(
                    op_name, f"mutation_worker_error: {e}"
                )

    for idx, digest, checkpoint, operator, run_idx in tasks:
        op_name = operator["name"]
        base_key = (digest, checkpoint, op_name, run_idx)
        results[(idx, op_name, run_idx)] = base_results.get(
            base_key,
            missing_operator_result(op_name, "mutation_missing_result"),
        )
    return results, batch_meta


major_status_re = re.compile(r"major_status:\s*([A-Z_]+)")


def error_family(summary):
    err = (summary or {}).get("local_error") or ""
    if not err:
        return None
    m = major_status_re.search(err)
    return m.group(1) if m else err[:120]


def score_operator_signal(baseline, summary, operator_name: str):
    local_error = summary.get("local_error") or ""
    if str(local_error).startswith("state_mutation_unavailable:"):
        return {"score": 0, "reasons": ["mutation_unavailable"]}
    baseline_success = baseline.get("local_success")
    baseline_timeout = bool(baseline.get("timed_out"))
    op_success = summary.get("local_success")
    op_timeout = bool(summary.get("timed_out"))
    baseline_family = error_family(baseline)
    op_family = error_family(summary)
    baseline_plane = classify_failure_plane(baseline)
    op_plane = classify_failure_plane(summary)

    score = 0
    reasons = []

    if baseline_success is not True and op_success is True:
        score += 8
        reasons.append("recovery")
    if baseline_success is True and (op_success is False or op_timeout):
        score += 6
        reasons.append("regression")
    if baseline_timeout and not op_timeout:
        if op_success is True:
            score += 4
            reasons.append("timeout_resolution")
        elif not operator_name.startswith("state_"):
            score += 2
            reasons.append("timeout_resolution_nonstate")
    if baseline_plane != op_plane and baseline_plane != "none" and op_plane != "none":
        if baseline_plane == "transport" and op_plane == "vm":
            score += 1
            reasons.append("plane_shift_transport_to_vm")
        elif baseline_plane == "vm" and op_plane == "transport":
            score += 1
            reasons.append("plane_shift_vm_to_transport")
        else:
            score += 1
            reasons.append("plane_shift")
    if baseline_family and op_family and baseline_family != op_family:
        score += 3
        reasons.append("error_family_shift")
    if op_success is True and (summary.get("synthetic_inputs") or 0) > 0:
        score += 2
        reasons.append("synthetic_success")
    if operator_name.startswith("state_"):
        score += 1
        reasons.append("state_mutation")
    if op_timeout:
        score -= 2
        reasons.append("timed_out")

    return {"score": score, "reasons": reasons}


def classify_failure_plane(summary):
    if not isinstance(summary, dict):
        return "other"
    explicit = summary.get("failure_plane")
    if isinstance(explicit, str) and explicit:
        return explicit
    return classify_failure_plane_from_error(
        summary.get("local_error"),
        bool(summary.get("timed_out")),
        summary.get("local_success"),
    )


def normalize_error_signature(local_error: str):
    if not isinstance(local_error, str):
        return ""
    s = local_error.lower()
    s = re.sub(r"0x[0-9a-f]+", "0xADDR", s)
    s = re.sub(r"\b[1-9][0-9]{2,}\b", "N", s)
    s = re.sub(r"\s+", " ", s).strip()
    return s[:240]


def finding_fingerprint(digest: str, checkpoint: int, operator_name: str, summary, signal):
    payload = {
        "digest": digest,
        "checkpoint": checkpoint,
        "operator": operator_name,
        "local_success": summary.get("local_success"),
        "timed_out": bool(summary.get("timed_out")),
        "plane": classify_failure_plane(summary),
        "error_family": error_family(summary),
        "error_signature": normalize_error_signature(summary.get("local_error") or ""),
        "signal_score": int((signal or {}).get("score", 0)),
    }
    raw = json.dumps(payload, sort_keys=True).encode("utf-8")
    return hashlib.sha1(raw).hexdigest(), payload


def replay_outcome_signature(summary):
    return (
        summary.get("local_success"),
        bool(summary.get("timed_out")),
        classify_failure_plane(summary),
        error_family(summary),
        summary.get("commands_executed"),
    )


def _collect_leaf_diffs(base_obj, mut_obj, path, out):
    if type(base_obj) != type(mut_obj):
        out.append((path, base_obj, mut_obj))
        return
    if isinstance(base_obj, dict):
        keys = sorted(set(base_obj.keys()) | set(mut_obj.keys()))
        for k in keys:
            if k not in base_obj:
                out.append((path + (("dict", k),), None, mut_obj[k]))
            elif k not in mut_obj:
                out.append((path + (("dict", k),), base_obj[k], None))
            else:
                _collect_leaf_diffs(base_obj[k], mut_obj[k], path + (("dict", k),), out)
        return
    if isinstance(base_obj, list):
        max_len = max(len(base_obj), len(mut_obj))
        for i in range(max_len):
            if i >= len(base_obj):
                out.append((path + (("list", i),), None, mut_obj[i]))
            elif i >= len(mut_obj):
                out.append((path + (("list", i),), base_obj[i], None))
            else:
                _collect_leaf_diffs(base_obj[i], mut_obj[i], path + (("list", i),), out)
        return
    if base_obj != mut_obj:
        out.append((path, base_obj, mut_obj))


def collect_leaf_diffs(base_obj, mut_obj):
    out = []
    _collect_leaf_diffs(base_obj, mut_obj, tuple(), out)
    return out


def _set_path_value(root, path, value):
    cur = root
    for idx, (kind, token) in enumerate(path):
        is_last = idx == len(path) - 1
        if kind == "dict":
            if is_last:
                cur[token] = value
            else:
                cur = cur[token]
        elif kind == "list":
            if is_last:
                if token >= len(cur):
                    cur.extend([None] * (token - len(cur) + 1))
                cur[token] = value
            else:
                cur = cur[token]


def _build_doc_from_diff_subset(base_doc, diffs, keep_indices):
    doc = json.loads(json.dumps(base_doc))
    for idx in keep_indices:
        path, _before, after = diffs[idx]
        _set_path_value(doc, path, after)
    return doc


def _minimization_preserves_behavior(candidate, original):
    if candidate.get("local_success") != original.get("local_success"):
        return False
    if bool(candidate.get("timed_out")) != bool(original.get("timed_out")):
        return False
    if candidate.get("local_success") is True:
        return True
    if error_family(candidate) != error_family(original):
        return False
    return True


def minimize_state_mutation(
    digest: str,
    checkpoint: int,
    operator,
    original_summary,
    mutation_meta,
):
    if not RUN_MINIMIZATION:
        return {"performed": False, "reason": "minimization_disabled"}
    if operator.get("kind") != "state_json":
        return {"performed": False, "reason": "not_state_operator"}
    if not isinstance(mutation_meta, dict):
        return {"performed": False, "reason": "missing_mutation_meta"}

    base_path = mutation_meta.get("base_state_path")
    mutated_path = mutation_meta.get("mutated_state_path")
    if not base_path or not mutated_path:
        return {"performed": False, "reason": "missing_state_paths"}
    try:
        base_doc = json.loads(Path(base_path).read_text(encoding="utf-8"))
        mutated_doc = json.loads(Path(mutated_path).read_text(encoding="utf-8"))
    except Exception as e:
        return {"performed": False, "reason": f"state_load_failed:{e}"}

    diffs = collect_leaf_diffs(base_doc, mutated_doc)
    if len(diffs) <= 1:
        return {
            "performed": True,
            "trials": 0,
            "diff_count_before": len(diffs),
            "diff_count_after": len(diffs),
            "minimized": False,
            "reason": "already_minimal",
        }

    active = list(range(len(diffs)))
    trials = 0
    best_summary = None
    for idx in list(active):
        if trials >= MINIMIZE_MAX_TRIALS:
            break
        candidate_keep = [i for i in active if i != idx]
        if not candidate_keep:
            continue
        trial_doc = _build_doc_from_diff_subset(base_doc, diffs, candidate_keep)
        trial_path = (
            RAW_DIR
            / f"state_min_trial_{operator['name']}_{digest}_{checkpoint}_{trials}.json"
        )
        write_json(trial_path, trial_doc)
        cmd = replay_cmd_with_state_json(digest, trial_path, operator.get("flags", []))
        run = run_cmd(cmd, timeout=int(operator.get("timeout", REPLAY_TIMEOUT_SECS)), extra_env=replay_env)
        summary = summarize_replay_result(run, operator["name"])
        trials += 1
        if _minimization_preserves_behavior(summary, original_summary):
            active = candidate_keep
            best_summary = summary

    minimized = len(active) < len(diffs)
    result = {
        "performed": True,
        "trials": trials,
        "diff_count_before": len(diffs),
        "diff_count_after": len(active),
        "minimized": minimized,
        "reason": "success" if minimized else "no_reduction",
    }
    if minimized:
        min_doc = _build_doc_from_diff_subset(base_doc, diffs, active)
        min_path = RAW_DIR / f"state_min_{operator['name']}_{digest}_{checkpoint}.json"
        write_json(min_path, min_doc)
        result["minimized_state_path"] = str(min_path)
        if best_summary is not None:
            result["summary"] = best_summary
    return result


target_attempts = []
metadata_stats = Counter()
selection_stats = Counter()
replay_candidates = []
phase_a_target_cap = (
    PHASE_A_TARGETS
    if PHASE_A_TARGETS > 0
    else (max(MAX_TARGETS, MAX_TARGETS * 3) if PHASE_MODE == "phased" else MAX_TARGETS)
)

for row in universe_rows:
    if len(replay_candidates) >= phase_a_target_cap:
        break

    pkg = row["package"]
    mod = row["module"]
    fn = row["function"]
    attempt = dict(row)

    fn_meta = None
    if METADATA_LOOKUP:
        load_meta = ensure_package_loaded(pkg)
        attempt["package_load"] = load_meta

        if not load_meta["ok"]:
            attempt["metadata_status"] = "package_load_failed"
            attempt["status"] = "package_load_failed"
            target_attempts.append(attempt)
            metadata_stats["package_load_failed"] += 1
            continue

        module_meta = get_module_meta(pkg, mod)
        attempt["module_meta_ok"] = module_meta["ok"]

        if not module_meta["ok"]:
            attempt["metadata_status"] = "module_view_failed"
            attempt["module_error"] = module_meta["error"]
            metadata_stats["module_view_failed"] += 1
        else:
            fn_meta = module_meta["functions"].get(fn)
            if fn_meta is None:
                attempt["metadata_status"] = "function_missing_in_module"
                metadata_stats["function_missing_in_module"] += 1
            else:
                attempt["metadata_status"] = "function_meta_ok"
                attempt["function_meta"] = fn_meta
                metadata_stats["function_meta_ok"] += 1

        if fn_meta is not None:
            is_entry = bool(fn_meta.get("is_entry") is True)
            visibility = (fn_meta.get("visibility") or "").lower()
            is_public = visibility == "public"
            attempt["is_entry"] = is_entry
            attempt["is_public"] = is_public
            if not is_entry and not (INCLUDE_PUBLIC and is_public):
                attempt["status"] = "skipped_non_entry"
                target_attempts.append(attempt)
                selection_stats["skipped_non_entry"] += 1
                continue
        else:
            attempt["metadata_unknown"] = True
    else:
        attempt["metadata_status"] = "disabled"
        metadata_stats["disabled"] += 1
        if INCLUDE_PUBLIC:
            attempt["metadata_note"] = (
                "--include-public has no effect when metadata lookup is disabled"
            )

    digest = row["sample_digest"]
    checkpoint = int(row["sample_checkpoint"])
    replay_candidates.append((len(target_attempts), digest, checkpoint))
    selection_stats["phase_a_candidates"] += 1
    target_attempts.append(attempt)

def missing_replay_result(mode: str, reason: str):
    return {
        "mode": mode,
        "exit_code": None,
        "elapsed_ms": 0,
        "timed_out": False,
        "local_success": None,
        "local_error": reason,
        "commands_executed": None,
        "synthetic_inputs": 0,
        "execution_path": {},
    }


phase_a_attempts = []
phase_b_candidates = []

if PHASE_MODE == "phased":
    phase_a_baseline = run_replay_batch(
        [(digest, checkpoint) for _, digest, checkpoint in replay_candidates],
        "baseline",
        PHASE_A_TIMEOUT_SECS,
        "phasea",
    )

    prioritized = []
    for idx, digest, checkpoint in replay_candidates:
        key = replay_key(digest, checkpoint, "baseline")
        baseline = phase_a_baseline.get(
            key, missing_replay_result("baseline", "phase_a_missing_result")
        )
        if baseline["timed_out"]:
            phase_a_status = "timeout"
        elif baseline["local_success"] is True:
            phase_a_status = "success"
        elif baseline["local_success"] is False:
            phase_a_status = "failure"
        else:
            phase_a_status = "unknown"

        phase_a_attempts.append(
            {
                "index": idx,
                "digest": digest,
                "checkpoint": checkpoint,
                "status": phase_a_status,
                "baseline": baseline,
            }
        )

        priority = None
        if phase_a_status == "failure":
            priority = 0
        elif phase_a_status == "unknown":
            priority = 1
        elif phase_a_status == "timeout" and PHASE_A_INCLUDE_TIMEOUTS:
            priority = 2

        if priority is not None:
            prioritized.append((priority, idx, digest, checkpoint, phase_a_status))

    prioritized.sort(key=lambda x: x[0])
    seen_idxs = set()
    for _prio, idx, digest, checkpoint, reason in prioritized:
        if idx in seen_idxs:
            continue
        phase_b_candidates.append((idx, digest, checkpoint))
        seen_idxs.add(idx)
        if len(phase_b_candidates) >= MAX_TARGETS:
            break

    if not phase_b_candidates:
        for idx, digest, checkpoint in replay_candidates[:MAX_TARGETS]:
            phase_b_candidates.append((idx, digest, checkpoint))
            seen_idxs.add(idx)
else:
    for idx, digest, checkpoint in replay_candidates[:MAX_TARGETS]:
        phase_b_candidates.append((idx, digest, checkpoint))

write_json(
    RUN_DIR / "phase_a_attempts.json",
    {
        "mode": PHASE_MODE,
        "phase_a_timeout_secs": PHASE_A_TIMEOUT_SECS,
        "phase_a_include_timeouts": PHASE_A_INCLUDE_TIMEOUTS,
        "phase_a_target_cap": phase_a_target_cap,
        "attempts": phase_a_attempts,
    },
)
write_json(
    RUN_DIR / "phase_b_selection.json",
    {
        "mode": PHASE_MODE,
        "max_targets": MAX_TARGETS,
        "phase_a_candidates": len(replay_candidates),
        "phase_b_selected": len(phase_b_candidates),
        "selected": [
            {"index": idx, "digest": digest, "checkpoint": checkpoint}
            for idx, digest, checkpoint in phase_b_candidates
        ],
    },
)

selection_stats["phase_b_selected"] = len(phase_b_candidates)
selection_stats["replayed"] = len(phase_b_candidates)
replay_attempts = len(phase_b_candidates)

phase_b_baseline = run_replay_batch(
    [(digest, checkpoint) for _, digest, checkpoint in phase_b_candidates],
    "baseline",
    REPLAY_TIMEOUT_SECS,
    "phaseb",
)

heal_candidates = []
for idx, digest, checkpoint in phase_b_candidates:
    attempt = target_attempts[idx]
    baseline_key = replay_key(digest, checkpoint, "baseline")
    baseline = phase_b_baseline.get(
        baseline_key, missing_replay_result("baseline", "phase_b_missing_result")
    )
    attempt["baseline"] = baseline
    run_heal = False
    if HEAL_MODE == "always":
        run_heal = True
    elif HEAL_MODE == "on-failure":
        # Timeout cases are typically transport-bound; skip heal retry in practical mode
        # to avoid doubling network pressure on already slow candidates.
        run_heal = (baseline["local_success"] is False) and (not baseline["timed_out"])
    if run_heal:
        heal_candidates.append((idx, digest, checkpoint))
    else:
        attempt["heal_skipped"] = True

heal_attempts = len(heal_candidates)
heal_results = run_replay_batch(
    [(digest, checkpoint) for _, digest, checkpoint in heal_candidates],
    "heal",
    REPLAY_TIMEOUT_SECS,
    "phaseb",
)

for idx, digest, checkpoint in phase_b_candidates:
    attempt = target_attempts[idx]
    baseline = attempt["baseline"]
    heal = None
    heal_key = replay_key(digest, checkpoint, "heal")
    if heal_key in heal_results:
        heal = heal_results[heal_key]
        attempt["heal"] = heal
    if heal is None:
        if baseline["timed_out"]:
            replay_status = "baseline_timeout"
        elif baseline["local_success"] is True:
            replay_status = "baseline_success"
        elif baseline["local_success"] is False:
            replay_status = "baseline_failure"
        else:
            replay_status = "baseline_unknown"
    elif baseline["timed_out"] or heal["timed_out"]:
        replay_status = "timeout"
    elif baseline["local_success"] is False and heal["local_success"] is True:
        replay_status = "fail_to_heal"
    elif baseline["local_success"] is True and heal["local_success"] is True:
        replay_status = "stable_success"
    elif baseline["local_success"] is False and heal["local_success"] is False:
        replay_status = "persistent_failure"
    elif baseline["local_success"] is True and heal["local_success"] is False:
        replay_status = "regressed_on_heal"
    else:
        replay_status = "mixed_or_unknown"

    attempt["replay_status"] = replay_status
    if attempt.get("metadata_unknown"):
        attempt["status"] = f"{replay_status}_meta_unknown"
    else:
        attempt["status"] = replay_status

final_attempts = [target_attempts[idx] for idx, _, _ in phase_b_candidates]

mutation_runs = 0
mutation_task_instances = 0
mutation_candidates = []
mutation_results = []
oracle_targets = []
invariant_violations = []
minimization_results = []
findings_index = {}
mutation_batch_meta = {
    "state_exports_attempted": 0,
    "state_exports_succeeded": 0,
}
operator_signal_counts = Counter()
operator_signal_max = {}
plane_shift_counts = Counter()
unstable_operator_targets = 0

if RUN_MUTATIONS and final_attempts and MUTATION_BUDGET > 0:
    prioritized = []
    for i, attempt in enumerate(final_attempts):
        status = str(attempt.get("status") or "")
        priority = 4
        if "failure" in status or status in ("fail_to_heal", "persistent_failure"):
            priority = 0
        elif "timeout" in status:
            priority = 1
        elif "unknown" in status:
            priority = 2
        elif "success" in status:
            priority = 3
        prioritized.append((priority, i))
    prioritized.sort(key=lambda x: x[0])

    for _prio, i in prioritized[:MUTATION_BUDGET]:
        attempt = final_attempts[i]
        mutation_candidates.append(
            {
                "index": i,
                "digest": attempt["sample_digest"],
                "checkpoint": int(attempt["sample_checkpoint"]),
                "baseline_status": attempt.get("status"),
            }
        )

    mutation_tasks = []
    for c in mutation_candidates:
        for op in MUTATION_OPERATORS:
            for run_idx in range(STABILITY_RUNS):
                mutation_tasks.append(
                    (c["index"], c["digest"], c["checkpoint"], op, run_idx)
                )

    mutation_task_instances = len(mutation_tasks)
    mutation_runs = len(
        {
            (digest, checkpoint, operator["name"], run_idx)
            for _idx, digest, checkpoint, operator, run_idx in mutation_tasks
        }
    )
    op_results, mutation_batch_meta = run_operator_batch(mutation_tasks)

    for c in mutation_candidates:
        idx = c["index"]
        attempt = final_attempts[idx]
        baseline = attempt.get("baseline") or {}
        baseline_success = baseline.get("local_success")
        baseline_timed_out = bool(baseline.get("timed_out"))
        baseline_family = error_family(baseline)
        baseline_plane = classify_failure_plane(baseline)

        per_attempt = {}
        per_attempt_runs = {}
        per_attempt_signal = {}
        unstable_operators = []

        for op in MUTATION_OPERATORS:
            op_name = op["name"]
            run_summaries = []
            run_signals = []
            for run_idx in range(STABILITY_RUNS):
                key = (idx, op_name, run_idx)
                summary = op_results.get(
                    key, missing_replay_result(op_name, "mutation_missing_result")
                )
                run_summaries.append(summary)
                signal = score_operator_signal(baseline, summary, op_name)
                run_signals.append(signal)
                fingerprint, payload = finding_fingerprint(
                    c["digest"],
                    c["checkpoint"],
                    op_name,
                    summary,
                    signal,
                )
                signal_score = int(signal.get("score", 0))
                if signal_score >= 3:
                    entry = findings_index.setdefault(
                        fingerprint,
                        {
                            "fingerprint": fingerprint,
                            "count": 0,
                            "operators": {},
                            "signal_max": 0,
                            "sample": payload,
                            "examples": [],
                        },
                    )
                    entry["count"] += 1
                    entry["signal_max"] = max(entry["signal_max"], signal_score)
                    entry["operators"][op_name] = entry["operators"].get(op_name, 0) + 1
                    if len(entry["examples"]) < 5:
                        entry["examples"].append(
                            {
                                "digest": c["digest"],
                                "checkpoint": c["checkpoint"],
                                "index": idx,
                                "run_idx": run_idx,
                            }
                        )
                mutation_results.append(
                    {
                        "index": idx,
                        "digest": c["digest"],
                        "checkpoint": c["checkpoint"],
                        "operator": op_name,
                        "run_idx": run_idx,
                        "summary": summary,
                        "signal": signal,
                        "fingerprint": fingerprint,
                    }
                )

            primary = run_summaries[0] if run_summaries else missing_replay_result(
                op_name, "mutation_missing_primary"
            )
            per_attempt[op_name] = primary
            per_attempt_runs[op_name] = run_summaries
            primary_signal = run_signals[0] if run_signals else {"score": 0, "reasons": []}
            per_attempt_signal[op_name] = primary_signal

            unique_outcomes = {replay_outcome_signature(s) for s in run_summaries}
            if len(unique_outcomes) > 1:
                unstable_operators.append(op_name)
                invariant_violations.append(
                    {
                        "invariant": "inv_operator_stability_band",
                        "index": idx,
                        "digest": c["digest"],
                        "checkpoint": c["checkpoint"],
                        "operator": op_name,
                        "stability_runs": STABILITY_RUNS,
                        "unique_outcomes": len(unique_outcomes),
                    }
                )

            failed_families = {
                error_family(s)
                for s in run_summaries
                if s.get("local_success") is False
                and not bool(s.get("timed_out"))
                and error_family(s)
            }
            if len(failed_families) > 1:
                invariant_violations.append(
                    {
                        "invariant": "inv_error_family_stability_band",
                        "index": idx,
                        "digest": c["digest"],
                        "checkpoint": c["checkpoint"],
                        "operator": op_name,
                        "families": sorted(failed_families),
                    }
                )

            if (
                RUN_MINIMIZATION
                and op.get("kind") == "state_json"
                and int(primary_signal.get("score", 0)) >= 3
                and isinstance(primary.get("mutation_meta"), dict)
            ):
                min_result = minimize_state_mutation(
                    c["digest"],
                    c["checkpoint"],
                    op,
                    primary,
                    primary.get("mutation_meta"),
                )
                if min_result.get("performed"):
                    minimization_results.append(
                        {
                            "index": idx,
                            "digest": c["digest"],
                            "checkpoint": c["checkpoint"],
                            "operator": op_name,
                            "result": min_result,
                        }
                    )
                    primary["minimization"] = min_result

        attempt["mutations"] = per_attempt
        attempt["mutation_runs"] = per_attempt_runs
        attempt["mutation_signals"] = per_attempt_signal
        attempt["unstable_operators"] = unstable_operators
        if unstable_operators:
            unstable_operator_targets += 1

        recovery_ops = []
        regression_ops = []
        timeout_resolution_ops = []
        error_family_shift_ops = []
        synthetic_success_ops = []
        counterfactual_success_ops = []
        operator_signals = []
        transport_to_vm_ops = []
        vm_to_transport_ops = []

        for op_name, summary in per_attempt.items():
            op_success = summary.get("local_success")
            op_timeout = bool(summary.get("timed_out"))
            op_family = error_family(summary)
            op_plane = classify_failure_plane(summary)
            if baseline_plane != "none" and op_plane != "none" and baseline_plane != op_plane:
                plane_key = f"{baseline_plane}->{op_plane}"
                plane_shift_counts[plane_key] += 1
                if plane_key == "transport->vm":
                    transport_to_vm_ops.append(op_name)
                elif plane_key == "vm->transport":
                    vm_to_transport_ops.append(op_name)

            signal = per_attempt_signal.get(op_name, {"score": 0, "reasons": []})
            signal_score = int(signal.get("score", 0))
            if signal_score >= 3:
                operator_signal_counts[op_name] += 1
                operator_signal_max[op_name] = max(
                    signal_score,
                    int(operator_signal_max.get(op_name, signal_score)),
                )
                operator_signals.append(
                    {
                        "operator": op_name,
                        "score": signal_score,
                        "reasons": signal.get("reasons", []),
                    }
                )

            if op_success is True:
                cmds = summary.get("commands_executed")
                gas_used = summary.get("gas_used")
                if not isinstance(cmds, int) or cmds <= 0:
                    invariant_violations.append(
                        {
                            "invariant": "inv_success_has_command_progress",
                            "index": idx,
                            "digest": c["digest"],
                            "checkpoint": c["checkpoint"],
                            "operator": op_name,
                            "commands_executed": cmds,
                        }
                    )
                if not isinstance(gas_used, int) or gas_used <= 0:
                    invariant_violations.append(
                        {
                            "invariant": "inv_success_has_positive_gas",
                            "index": idx,
                            "digest": c["digest"],
                            "checkpoint": c["checkpoint"],
                            "operator": op_name,
                            "gas_used": gas_used,
                        }
                    )
                if summary.get("effects_success") is False:
                    invariant_violations.append(
                        {
                            "invariant": "inv_effects_success_consistency",
                            "index": idx,
                            "digest": c["digest"],
                            "checkpoint": c["checkpoint"],
                            "operator": op_name,
                        }
                    )

            if op_timeout and op_plane != "transport":
                invariant_violations.append(
                    {
                        "invariant": "inv_timeout_plane_transport",
                        "index": idx,
                        "digest": c["digest"],
                        "checkpoint": c["checkpoint"],
                        "operator": op_name,
                        "failure_plane": op_plane,
                    }
                )

            if baseline_success is not True and op_success is True:
                recovery_ops.append(op_name)
            if baseline_success is True and (op_success is False or op_timeout):
                regression_ops.append(op_name)
            if baseline_timed_out and not op_timeout:
                timeout_resolution_ops.append(op_name)
            if baseline_family and op_family and op_family != baseline_family:
                error_family_shift_ops.append(op_name)
            if op_success is True and (summary.get("synthetic_inputs") or 0) > 0:
                synthetic_success_ops.append(op_name)
            if op_name.startswith("state_") and baseline_success is not True and op_success is True:
                counterfactual_success_ops.append(op_name)
            if op_name.startswith("state_") and op_timeout:
                if str(summary.get("local_error") or "").startswith(
                    "state_mutation_unavailable:"
                ):
                    continue
                invariant_violations.append(
                    {
                        "invariant": "inv_state_mutation_no_timeout",
                        "index": idx,
                        "digest": c["digest"],
                        "checkpoint": c["checkpoint"],
                        "operator": op_name,
                        "baseline_status": attempt.get("status"),
                    }
                )

        operator_signals.sort(key=lambda s: (-s["score"], s["operator"]))
        baseline_repeat = per_attempt.get("baseline_repeat")
        flaky_baseline = False
        if baseline_repeat is not None:
            repeat_success = baseline_repeat.get("local_success")
            repeat_family = error_family(baseline_repeat)
            if repeat_success != baseline_success or repeat_family != baseline_family:
                flaky_baseline = True
                invariant_violations.append(
                    {
                        "invariant": "inv_baseline_repeat_stable",
                        "index": idx,
                        "digest": c["digest"],
                        "checkpoint": c["checkpoint"],
                        "baseline_success": baseline_success,
                        "repeat_success": repeat_success,
                        "baseline_error_family": baseline_family,
                        "repeat_error_family": repeat_family,
                    }
                )

        heal_aggressive = per_attempt.get("heal_aggressive")
        if baseline_success is True and heal_aggressive is not None:
            if heal_aggressive.get("local_success") is False or heal_aggressive.get("timed_out"):
                invariant_violations.append(
                    {
                        "invariant": "inv_heal_non_regression",
                        "index": idx,
                        "digest": c["digest"],
                        "checkpoint": c["checkpoint"],
                        "baseline_success": baseline_success,
                        "heal_aggressive_success": heal_aggressive.get("local_success"),
                        "heal_aggressive_timed_out": bool(heal_aggressive.get("timed_out")),
                    }
                )

        if baseline_success is False and heal_aggressive is not None and heal_aggressive.get("local_success") is True:
            base_cmds = baseline.get("commands_executed")
            heal_cmds = heal_aggressive.get("commands_executed")
            if isinstance(base_cmds, int) and isinstance(heal_cmds, int) and heal_cmds < base_cmds:
                invariant_violations.append(
                    {
                        "invariant": "inv_recovery_exec_progress",
                        "index": idx,
                        "digest": c["digest"],
                        "checkpoint": c["checkpoint"],
                        "baseline_commands_executed": base_cmds,
                        "heal_commands_executed": heal_cmds,
                    }
                )

        oracle_targets.append(
            {
                "index": idx,
                "digest": c["digest"],
                "checkpoint": c["checkpoint"],
                "baseline_status": attempt.get("status"),
                "recovery_operators": recovery_ops,
                "regression_operators": regression_ops,
                "timeout_resolution_operators": timeout_resolution_ops,
                "error_family_shift_operators": error_family_shift_ops,
                "synthetic_success_operators": synthetic_success_ops,
                "counterfactual_success_operators": counterfactual_success_ops,
                "transport_to_vm_operators": transport_to_vm_ops,
                "vm_to_transport_operators": vm_to_transport_ops,
                "unstable_operators": unstable_operators,
                "top_operator_signals": operator_signals[:3],
                "flaky_baseline": flaky_baseline,
            }
        )

oracle_summary = {
    "enabled": RUN_MUTATIONS,
    "mutation_budget": MUTATION_BUDGET,
    "mutation_workers": MUTATION_WORKERS,
    "stability_runs": STABILITY_RUNS,
    "typed_mutators": TYPED_MUTATORS,
    "auto_minimize": RUN_MINIMIZATION,
    "minimize_max_trials": MINIMIZE_MAX_TRIALS,
    "mutation_operators": [op["name"] for op in MUTATION_OPERATORS],
    "mutation_candidates": len(mutation_candidates),
    "mutation_task_instances": mutation_task_instances,
    "mutation_runs": mutation_runs,
    "state_exports_attempted": mutation_batch_meta["state_exports_attempted"],
    "state_exports_succeeded": mutation_batch_meta["state_exports_succeeded"],
    "signature_hints_attempted": mutation_batch_meta.get("signature_hints_attempted", 0),
    "signature_hints_with_entries": mutation_batch_meta.get("signature_hints_with_entries", 0),
    "signature_hints_inferred": mutation_batch_meta.get("signature_hints_inferred", 0),
    "signature_hints_module_typed": mutation_batch_meta.get("signature_hints_module_typed", 0),
    "unstable_operator_targets": unstable_operator_targets,
    "targets_with_recovery": sum(1 for t in oracle_targets if t["recovery_operators"]),
    "targets_with_regression": sum(1 for t in oracle_targets if t["regression_operators"]),
    "targets_with_counterfactual_success": sum(
        1 for t in oracle_targets if t["counterfactual_success_operators"]
    ),
    "targets_with_timeout_resolution": sum(
        1 for t in oracle_targets if t["timeout_resolution_operators"]
    ),
    "flaky_baseline_targets": sum(1 for t in oracle_targets if t["flaky_baseline"]),
    "plane_shift_counts": {
        k: v
        for k, v in sorted(
            plane_shift_counts.items(),
            key=lambda kv: (-kv[1], kv[0]),
        )
    },
    "operator_signal_counts": {
        k: v
        for k, v in sorted(
            operator_signal_counts.items(),
            key=lambda kv: (-kv[1], kv[0]),
        )
    },
    "operator_signal_max": {
        k: int(v)
        for k, v in sorted(
            operator_signal_max.items(),
            key=lambda kv: (-kv[1], kv[0]),
        )
    },
    "minimization_attempts": len(minimization_results),
    "minimization_successes": sum(
        1 for m in minimization_results if (m.get("result") or {}).get("minimized")
    ),
    "findings_fingerprints": len(findings_index),
    "findings_instances": sum(v.get("count", 0) for v in findings_index.values()),
    "invariant_violations": len(invariant_violations),
}

write_json(
    RUN_DIR / "mutation_results.json",
    {
        "summary": oracle_summary,
        "candidates": mutation_candidates,
        "results": mutation_results,
    },
)
write_json(
    RUN_DIR / "oracle_report.json",
    {"summary": oracle_summary, "targets": oracle_targets},
)
write_json(RUN_DIR / "invariant_violations.json", invariant_violations)
write_json(
    RUN_DIR / "minimization_results.json",
    minimization_results,
)
write_json(
    RUN_DIR / "findings_index.json",
    sorted(
        findings_index.values(),
        key=lambda e: (-int(e.get("signal_max", 0)), -int(e.get("count", 0))),
    ),
)

write_json(RUN_DIR / "attempts.json", final_attempts)

# Coverage summary
stats = Counter(a.get("status", "unknown") for a in final_attempts)
entry_count = sum(
    1
    for a in final_attempts
    if "baseline" in a and (a.get("function_meta") or {}).get("is_entry") is True
)
public_count = sum(
    1
    for a in final_attempts
    if "baseline" in a and (a.get("function_meta") or {}).get("visibility") == "public"
)
metadata_unknown_replayed = sum(
    1
    for a in final_attempts
    if "baseline" in a and a.get("metadata_unknown") is True
)
metadata_status_final = Counter(a.get("metadata_status", "unknown") for a in final_attempts)

coverage = {
    "checkpoint_ingest": checkpoint_ingest,
    "phase_mode": PHASE_MODE,
    "phase_a_timeout_secs": PHASE_A_TIMEOUT_SECS,
    "phase_a_target_cap": phase_a_target_cap,
    "phase_a_include_timeouts": PHASE_A_INCLUDE_TIMEOUTS,
    "phase_a_candidates": len(replay_candidates),
    "phase_b_candidates": len(phase_b_candidates),
    "targets_scanned": len(universe_rows),
    "targets_requested_for_replay": MAX_TARGETS,
    "attempt_records": len(final_attempts),
    "replay_jobs": REPLAY_JOBS,
    "mutation_workers": MUTATION_WORKERS,
    "stability_runs": STABILITY_RUNS,
    "typed_mutators": TYPED_MUTATORS,
    "auto_minimize": RUN_MINIMIZATION,
    "minimize_max_trials": MINIMIZE_MAX_TRIALS,
    "replay_attempts": replay_attempts,
    "heal_attempts": heal_attempts,
    "heal_mode": HEAL_MODE,
    "metadata_lookup": METADATA_LOOKUP,
    "entry_targets_replayed": entry_count,
    "public_targets_replayed": public_count,
    "metadata_unknown_replayed": metadata_unknown_replayed,
    "status_counts": dict(stats),
    "metadata_status_counts": dict(metadata_status_final),
    "metadata_status_counts_phase_a": dict(metadata_stats),
    "selection_counts": dict(selection_stats),
    "include_public": INCLUDE_PUBLIC,
    "oracle_summary": oracle_summary,
}
write_json(RUN_DIR / "coverage.json", coverage)

# Failure clusters
cluster = Counter()
for a in final_attempts:
    err = ""
    if isinstance(a.get("heal"), dict):
        err = a["heal"].get("local_error") or ""
    if not err and isinstance(a.get("baseline"), dict):
        err = a["baseline"].get("local_error") or ""
    if not err:
        continue
    m = major_status_re.search(err)
    key = m.group(1) if m else err[:140]
    cluster[key] += 1

write_json(
    RUN_DIR / "failure_clusters.json",
    [{"cluster": k, "count": v} for k, v in cluster.most_common()],
)

interesting = [
    a
    for a in final_attempts
    if str(a.get("status") or "").startswith("fail_to_heal")
]
write_json(RUN_DIR / "interesting_successes.json", interesting)

report = {
    "status": "ok",
    "coverage": coverage,
    "top_interesting_count": len(interesting),
    "run_dir": str(RUN_DIR),
}
write_json(RUN_DIR / "report.json", report)

lines = []
lines.append("# Entry Function Practical Fuzzer\n")
lines.append("Replay-backed practical fuzzer over real recent Walrus function targets.\n")
lines.append(f"- Checkpoint window: `{start_cp}..{latest_cp}`")
lines.append(f"- Transactions inspected: `{transactions_seen}`")
lines.append(f"- Targets scanned: `{len(universe_rows)}`")
lines.append(f"- Phase mode: `{PHASE_MODE}`")
lines.append(f"- Phase A candidates: `{len(replay_candidates)}`")
lines.append(f"- Phase B selected: `{len(phase_b_candidates)}`")
lines.append(f"- Replay targets requested: `{MAX_TARGETS}`")
lines.append(f"- Replay jobs: `{REPLAY_JOBS}`")
lines.append(f"- Mutation workers: `{MUTATION_WORKERS}`")
lines.append(f"- Stability runs: `{STABILITY_RUNS}`")
lines.append(f"- Mutation candidates: `{len(mutation_candidates)}`")
lines.append(f"- Mutation task instances: `{mutation_task_instances}`")
lines.append(f"- Replay attempts executed: `{replay_attempts}`")
lines.append(f"- Heal attempts executed: `{heal_attempts}` (mode: `{HEAL_MODE}`)")
lines.append(f"- Metadata lookup enabled: `{METADATA_LOOKUP}`")
lines.append(f"- Checkpoint ingest enabled: `{INGEST_CHECKPOINTS}`")
lines.append(f"- Include public non-entry: `{INCLUDE_PUBLIC}`")
lines.append("\n## Status Counts\n")
for k, v in stats.items():
    lines.append(f"- {k}: `{v}`")

if interesting:
    lines.append("\n## Fail -> Heal Samples\n")
    for item in interesting[:10]:
        lines.append(
            f"- `{item['package']}::{item['module']}::{item['function']}` via `{item['sample_digest']}` @ `{item['sample_checkpoint']}`"
        )

if oracle_summary.get("enabled"):
    lines.append("\n## Oracle Summary\n")
    lines.append(f"- State exports attempted: `{oracle_summary['state_exports_attempted']}`")
    lines.append(f"- State exports succeeded: `{oracle_summary['state_exports_succeeded']}`")
    lines.append(f"- Signature hints with entries: `{oracle_summary['signature_hints_with_entries']}`")
    lines.append(f"- Signature hints inferred: `{oracle_summary['signature_hints_inferred']}`")
    lines.append(f"- Signature hints from module metadata: `{oracle_summary['signature_hints_module_typed']}`")
    lines.append(f"- Targets with recovery: `{oracle_summary['targets_with_recovery']}`")
    lines.append(f"- Targets with regression: `{oracle_summary['targets_with_regression']}`")
    lines.append(
        f"- Targets with counterfactual success: `{oracle_summary['targets_with_counterfactual_success']}`"
    )
    lines.append(
        f"- Targets with timeout resolution: `{oracle_summary['targets_with_timeout_resolution']}`"
    )
    lines.append(
        f"- Operators with positive signal: `{len(oracle_summary.get('operator_signal_counts', {}))}`"
    )
    lines.append(
        f"- Minimization successes: `{oracle_summary['minimization_successes']}/{oracle_summary['minimization_attempts']}`"
    )
    lines.append(f"- Finding fingerprints: `{oracle_summary['findings_fingerprints']}`")
    lines.append(f"- Flaky baseline targets: `{oracle_summary['flaky_baseline_targets']}`")
    lines.append(f"- Invariant violations: `{oracle_summary['invariant_violations']}`")

lines.append("\n## Files\n")
lines.append("- `checkpoint_ingest.json`")
lines.append("- `function_universe.json`")
lines.append("- `phase_a_attempts.json`")
lines.append("- `phase_b_selection.json`")
lines.append("- `attempts.json`")
lines.append("- `mutation_results.json`")
lines.append("- `oracle_report.json`")
lines.append("- `invariant_violations.json`")
lines.append("- `minimization_results.json`")
lines.append("- `findings_index.json`")
lines.append("- `coverage.json`")
lines.append("- `failure_clusters.json`")
lines.append("- `interesting_successes.json`")
lines.append("- `report.json`")
lines.append("- `raw/*.json` (cached replay outputs)")

(RUN_DIR / "README.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

print(f"Entry Function Practical Fuzzer complete: {RUN_DIR}")
print(f"Phase mode: {PHASE_MODE} (phase-a={len(replay_candidates)}, phase-b={len(phase_b_candidates)})")
print(f"Replay attempts executed: {replay_attempts}")
print(f"Heal attempts executed: {heal_attempts} (mode={HEAL_MODE})")
print(
    "Mutation stage: "
    f"enabled={RUN_MUTATIONS} candidates={len(mutation_candidates)} "
    f"task_instances={mutation_task_instances} runs={mutation_runs} workers={MUTATION_WORKERS} "
    f"state_exports={oracle_summary['state_exports_succeeded']}/{oracle_summary['state_exports_attempted']} "
    f"signal_ops={len(oracle_summary.get('operator_signal_counts', {}))} "
    f"stability_runs={STABILITY_RUNS} "
    f"minimized={oracle_summary['minimization_successes']}/{oracle_summary['minimization_attempts']}"
)
print(f"Invariant violations: {len(invariant_violations)}")
print(f"Fail->heal count: {len(interesting)}")
PY

echo ""
echo "Artifacts:"
echo "  $RUN_DIR/README.md"
echo "  $RUN_DIR/checkpoint_ingest.json"
echo "  $RUN_DIR/function_universe.json"
echo "  $RUN_DIR/phase_a_attempts.json"
echo "  $RUN_DIR/phase_b_selection.json"
echo "  $RUN_DIR/attempts.json"
echo "  $RUN_DIR/mutation_results.json"
echo "  $RUN_DIR/oracle_report.json"
echo "  $RUN_DIR/invariant_violations.json"
echo "  $RUN_DIR/minimization_results.json"
echo "  $RUN_DIR/findings_index.json"
echo "  $RUN_DIR/coverage.json"
echo "  $RUN_DIR/failure_clusters.json"
echo "  $RUN_DIR/interesting_successes.json"
echo "  $RUN_DIR/report.json"
