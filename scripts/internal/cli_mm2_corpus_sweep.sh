#!/bin/bash
# ==============================================================================
# Corpus MM2 Sweep (CLI)
# ==============================================================================
# Runs:
#   sui-sandbox --json analyze package --bytecode-dir <PACKAGE_DIR> --mm2
# for package directories in a local corpus. Produces a TSV report and a
# concise pass/fail summary to validate MM2 stability over a corpus sample.
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -z "$PROJECT_ROOT" ]]; then
  PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
fi
SUI_SANDBOX="${SUI_SANDBOX_BIN:-${PROJECT_ROOT}/target/release/sui-sandbox}"
DEFAULT_CORPUS="${PROJECT_ROOT}/../sui-packages/packages/mainnet_most_used"
LIMIT="${2:-0}" # 0 means all packages in corpus
OUT_DIR="${OUT_DIR:-/tmp/sui-sandbox-mm2-sweep}"
PROGRESS_EVERY="${PROGRESS_EVERY:-25}"

print_usage() {
  cat <<EOF
Usage: $0 [CORPUS_DIR] [LIMIT]

Arguments:
  CORPUS_DIR  Path to corpus root (default: ${DEFAULT_CORPUS})
  LIMIT       Number of packages to test (0 = all, default: ${LIMIT})

Environment overrides:
  SUI_SANDBOX_BIN  Path to sui-sandbox binary (default: ${SUI_SANDBOX})
  SUI_CORPUS_DIR   Corpus directory if arg is omitted
  OUT_DIR          Output directory (default: ${OUT_DIR})
  PROGRESS_EVERY   Print progress every N packages (default: ${PROGRESS_EVERY})

Examples:
  $0 /path/to/sui-packages/packages/mainnet_most_used 100
  $0 /path/to/sui-packages/packages/mainnet_most_used 1000
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  print_usage
  exit 0
fi

CORPUS_DIR="${1:-${SUI_CORPUS_DIR:-${DEFAULT_CORPUS}}}"

if [[ ! -d "${CORPUS_DIR}" ]]; then
  echo "error: corpus directory not found: ${CORPUS_DIR}" >&2
  print_usage
  exit 1
fi

if ! [[ "${LIMIT}" =~ ^[0-9]+$ ]]; then
  echo "error: LIMIT must be a non-negative integer, got '${LIMIT}'" >&2
  exit 1
fi

mkdir -p "${OUT_DIR}"

ensure_binary() {
  if [[ ! -x "${SUI_SANDBOX}" ]]; then
    echo "Building sui-sandbox (release)..."
    cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
  fi
  if ! "${SUI_SANDBOX}" analyze package --help >/dev/null 2>&1; then
    echo "Rebuilding sui-sandbox (release) to ensure analyze package is available..."
    cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
  fi
}

ensure_binary

TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
REPORT_FILE="${OUT_DIR}/mm2_sweep_${TIMESTAMP}.tsv"

mapfile -t PACKAGE_DIRS < <(
  find "${CORPUS_DIR}" -mindepth 2 -maxdepth 2 \( -type d -o -type l \) | sort
)

if [[ "${LIMIT}" -gt 0 && "${#PACKAGE_DIRS[@]}" -gt "${LIMIT}" ]]; then
  PACKAGE_DIRS=("${PACKAGE_DIRS[@]:0:${LIMIT}}")
fi

TOTAL="${#PACKAGE_DIRS[@]}"
if [[ "${TOTAL}" -eq 0 ]]; then
  echo "error: no package directories found under ${CORPUS_DIR}" >&2
  exit 1
fi

echo -e "index\tstatus\tpackage_dir\tmm2_error" > "${REPORT_FILE}"

ok_count=0
fail_count=0
panic_count=0

start_epoch="$(date +%s)"

echo "Running MM2 sweep: total=${TOTAL} corpus=${CORPUS_DIR}"

for ((i = 0; i < TOTAL; i++)); do
  pkg_dir="${PACKAGE_DIRS[$i]}"
  idx=$((i + 1))

  set +e
  output="$("${SUI_SANDBOX}" --json analyze package --bytecode-dir "${pkg_dir}" --mm2 2>&1)"
  status=$?
  set -e

  mm2_status="failed"
  mm2_error=""

  if [[ "${status}" -eq 0 ]]; then
    readarray -t parsed < <(python - "${output}" <<'PY'
import json
import sys

text = sys.argv[1]
try:
    payload = json.loads(text)
except Exception as exc:
    print("0")
    print(f"json_parse_error: {exc}")
    raise SystemExit(0)

ok = payload.get("mm2_model_ok") is True
err = payload.get("mm2_error") or ""
print("1" if ok else "0")
print(str(err).replace("\n", " ").strip())
PY
)
    parsed_ok="${parsed[0]:-0}"
    mm2_error="${parsed[1]:-}"

    if [[ "${parsed_ok}" == "1" ]]; then
      mm2_status="ok"
      ok_count=$((ok_count + 1))
    else
      fail_count=$((fail_count + 1))
    fi
  else
    fail_count=$((fail_count + 1))
    mm2_error="$(printf "%s" "${output}" | tr '\n' ' ' | tr '\t' ' ' | sed 's/  */ /g')"
  fi

  if printf "%s %s" "${output}" "${mm2_error}" | grep -qi "panic"; then
    panic_count=$((panic_count + 1))
  fi

  mm2_error="$(printf "%s" "${mm2_error}" | tr '\n' ' ' | tr '\t' ' ' | sed 's/  */ /g')"
  echo -e "${idx}\t${mm2_status}\t${pkg_dir}\t${mm2_error}" >> "${REPORT_FILE}"

  if [[ "${idx}" -eq "${TOTAL}" ]] || (( idx % PROGRESS_EVERY == 0 )); then
    echo "  progress: ${idx}/${TOTAL} ok=${ok_count} fail=${fail_count} panic=${panic_count}"
  fi
done

end_epoch="$(date +%s)"
elapsed="$((end_epoch - start_epoch))"

echo ""
echo "MM2 Sweep Summary"
echo "  total:  ${TOTAL}"
echo "  ok:     ${ok_count}"
echo "  failed: ${fail_count}"
echo "  panic:  ${panic_count}"
echo "  secs:   ${elapsed}"
echo "  report: ${REPORT_FILE}"
