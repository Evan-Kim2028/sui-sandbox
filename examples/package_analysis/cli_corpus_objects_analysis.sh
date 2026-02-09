#!/bin/bash
# ==============================================================================
# Corpus Object Analysis (CLI)
# ==============================================================================
# Runs:
#   sui-sandbox --json analyze objects --corpus-dir <DIR>
# against a local package corpus (for example mainnet_most_used), stores raw JSON,
# and prints a compact summary including Sam-style baseline comparison fields.
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
SUI_SANDBOX="${SUI_SANDBOX_BIN:-${PROJECT_ROOT}/target/release/sui-sandbox}"
DEFAULT_CORPUS="${PROJECT_ROOT}/../sui-packages/packages/mainnet_most_used"
TOP="${TOP:-20}"
LIST_TYPES="${LIST_TYPES:-0}"
OUT_DIR="${OUT_DIR:-/tmp/sui-sandbox-corpus-analysis}"
PROFILE="${PROFILE:-hybrid}"
PROFILE_FILE="${PROFILE_FILE:-}"

print_usage() {
  cat <<EOF
Usage: $0 [CORPUS_DIR]

Environment overrides:
  SUI_SANDBOX_BIN  Path to sui-sandbox binary (default: ${SUI_SANDBOX})
  SUI_CORPUS_DIR   Corpus directory if arg is omitted
  TOP              --top value for analyze objects (default: ${TOP})
  LIST_TYPES       Set to 1 to include --list-types (default: ${LIST_TYPES})
  OUT_DIR          Output directory (default: ${OUT_DIR})
  PROFILE          Analyze objects profile name (default: ${PROFILE})
  PROFILE_FILE     Explicit profile YAML file (overrides PROFILE)

Example:
  $0 /path/to/sui-packages/packages/mainnet_most_used
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

mkdir -p "${OUT_DIR}"

ensure_binary() {
  if [[ ! -x "${SUI_SANDBOX}" ]]; then
    echo "Building sui-sandbox (release)..."
    cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
  fi
  if ! "${SUI_SANDBOX}" analyze objects --help >/dev/null 2>&1; then
    echo "Rebuilding sui-sandbox (release) to ensure analyze objects is available..."
    cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
  fi
  if ! "${SUI_SANDBOX}" analyze objects --help 2>/dev/null | grep -q -- "--profile"; then
    echo "Rebuilding sui-sandbox (release) to ensure analyze objects profile flags are available..."
    cargo build --release --bin sui-sandbox --manifest-path="${PROJECT_ROOT}/Cargo.toml"
  fi
}

ensure_binary

TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
OBJECTS_JSON="${OUT_DIR}/objects_${TIMESTAMP}.json"

CMD=(
  "${SUI_SANDBOX}"
  --json
  analyze
  objects
  --corpus-dir "${CORPUS_DIR}"
  --top "${TOP}"
)

if [[ -n "${PROFILE_FILE}" ]]; then
  CMD+=(--profile-file "${PROFILE_FILE}")
elif [[ -n "${PROFILE}" ]]; then
  CMD+=(--profile "${PROFILE}")
fi

if [[ "${LIST_TYPES}" == "1" ]]; then
  CMD+=(--list-types)
fi

echo "Running object analysis..."
"${CMD[@]}" > "${OBJECTS_JSON}"

python - "${OBJECTS_JSON}" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text())

print("")
print("Object Analysis Summary")
print(f"  corpus: {data.get('corpus_dir')}")
profile = data.get("profile", {})
if profile:
    print(
        "  profile: "
        f"name={profile.get('name')} "
        f"source={profile.get('source')} "
        f"semantic={profile.get('semantic_mode')} "
        f"dynamic_mode={profile.get('dynamic', {}).get('mode')} "
        f"lookback={profile.get('dynamic', {}).get('lookback')}"
    )
print(
    "  scan:   "
    f"packages={data.get('packages_scanned')} "
    f"failed={data.get('packages_failed')} "
    f"modules={data.get('modules_scanned')}"
)
print(
    "  objects:"
    f" discovered={data.get('object_types_discovered')} "
    f"unique={data.get('object_types_unique')}"
)

own = data.get("ownership", {})
own_u = data.get("ownership_unique", {})
party_transfer_eligible = data.get("party_transfer_eligible", data.get("party_capable", {}))
party_transfer_observed_in_bytecode = data.get(
    "party_transfer_observed_in_bytecode",
    data.get("party_observed", {}),
)
print(
    "  ownership (occurrence): "
    f"owned={own.get('owned', 0)} "
    f"shared={own.get('shared', 0)} "
    f"immutable={own.get('immutable', 0)} "
    f"party={own.get('party', 0)} "
    f"receive={own.get('receive', 0)}"
)
print(
    "  ownership (unique):     "
    f"owned={own_u.get('owned', 0)} "
    f"shared={own_u.get('shared', 0)} "
    f"immutable={own_u.get('immutable', 0)} "
    f"party={own_u.get('party', 0)} "
    f"receive={own_u.get('receive', 0)}"
)
print(
    "  party split: "
    f"eligible(types/occurrences)="
    f"{party_transfer_eligible.get('types', 0)}/{party_transfer_eligible.get('occurrences', 0)} "
    f"observed_in_bytecode(types/occurrences)="
    f"{party_transfer_observed_in_bytecode.get('types', 0)}/{party_transfer_observed_in_bytecode.get('occurrences', 0)}"
)
if party_transfer_eligible.get("types", 0) > 0:
    gap = (
        party_transfer_eligible.get("types", 0)
        - party_transfer_observed_in_bytecode.get("types", 0)
    )
    print(
        "  party gap (eligible-observed_in_bytecode, types): "
        f"{gap}  "
        "(large gap => many types are party-transfer eligible but are not observed via party-transfer calls in package bytecode)"
    )
print(
    "  traits: "
    f"singleton_types={data.get('singleton_types', 0)} "
    f"singleton_occurrences={data.get('singleton_occurrences', 0)} "
    f"dynamic_field_types={data.get('dynamic_field_types', 0)} "
    f"dynamic_field_occurrences={data.get('dynamic_field_occurrences', 0)}"
)

sam_baseline = {
    "object_types_discovered": 3879,
    "ownership_owned": 1116,
    "ownership_shared": 1530,
    "ownership_immutable": 6,
    "ownership_party": 1,
    "ownership_receive": 11,
    "singleton_types": 1330,
    "dynamic_field_types": 497,
}

print("")
print("Sam Baseline Delta (current - baseline)")
print(
    "  object_types_discovered: "
    f"{data.get('object_types_discovered', 0) - sam_baseline['object_types_discovered']:+d}"
)
print(
    "  ownership owned/shared/immutable/party/receive (occurrence, observed): "
    f"{own.get('owned', 0) - sam_baseline['ownership_owned']:+d}/"
    f"{own.get('shared', 0) - sam_baseline['ownership_shared']:+d}/"
    f"{own.get('immutable', 0) - sam_baseline['ownership_immutable']:+d}/"
    f"{own.get('party', 0) - sam_baseline['ownership_party']:+d}/"
    f"{own.get('receive', 0) - sam_baseline['ownership_receive']:+d}"
)
print(
    "  singleton_types: "
    f"{data.get('singleton_types', 0) - sam_baseline['singleton_types']:+d}"
)
print(
    "  dynamic_field_types: "
    f"{data.get('dynamic_field_types', 0) - sam_baseline['dynamic_field_types']:+d}"
)
PY

echo ""
echo "Raw JSON written to: ${OBJECTS_JSON}"
