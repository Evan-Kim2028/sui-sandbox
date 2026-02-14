#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

usage() {
  cat <<'EOF'
Usage: scripts/workflow_auto_bootstrap.sh --package-id <ID> [options]

Generate and validate a draft adapter workflow from a package id.

Options:
  --package-id <ID>         Package id to scaffold from (required)
  --template <NAME>         Optional template override: generic|cetus|suilend|scallop
  --digest <DIGEST>         Optional seed digest for replay/analyze steps
  --checkpoint <CP>         Optional checkpoint paired with --digest
  --output <PATH>           Output workflow spec path
  --format <FMT>            json (default) or yaml
  --best-effort             Continue even if closure validation fails
  --force                   Overwrite output file
  --bin <PATH>              CLI binary (default: target/debug/sui-sandbox, fallback: sui-sandbox in PATH)
  -h, --help                Show help

Examples:
  scripts/workflow_auto_bootstrap.sh --package-id 0x2 --force
  scripts/workflow_auto_bootstrap.sh --package-id 0x... --digest <DIGEST> --checkpoint <CP> --best-effort --force
EOF
}

PACKAGE_ID=""
TEMPLATE=""
DIGEST=""
CHECKPOINT=""
OUTPUT=""
FORMAT="json"
BEST_EFFORT=0
FORCE=0
CLI_BIN="${SUI_SANDBOX_BIN:-$ROOT/target/debug/sui-sandbox}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --package-id)
      PACKAGE_ID="${2:-}"
      shift 2
      ;;
    --template)
      TEMPLATE="${2:-}"
      shift 2
      ;;
    --digest)
      DIGEST="${2:-}"
      shift 2
      ;;
    --checkpoint)
      CHECKPOINT="${2:-}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:-}"
      shift 2
      ;;
    --format)
      FORMAT="${2:-}"
      shift 2
      ;;
    --best-effort)
      BEST_EFFORT=1
      shift
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --bin)
      CLI_BIN="${2:-}"
      shift 2
      ;;
    -h|--help)
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

if [[ -z "$PACKAGE_ID" ]]; then
  echo "Error: --package-id is required" >&2
  usage
  exit 2
fi

if [[ -n "$DIGEST" && -z "$CHECKPOINT" ]]; then
  echo "Error: --checkpoint is required when --digest is provided" >&2
  exit 2
fi
if [[ -n "$CHECKPOINT" && -z "$DIGEST" ]]; then
  echo "Error: --digest is required when --checkpoint is provided" >&2
  exit 2
fi

if [[ "$FORMAT" != "json" && "$FORMAT" != "yaml" ]]; then
  echo "Error: --format must be json or yaml" >&2
  exit 2
fi

if [[ ! -x "$CLI_BIN" ]]; then
  if command -v sui-sandbox >/dev/null 2>&1; then
    CLI_BIN="$(command -v sui-sandbox)"
  else
    echo "[workflow-auto-bootstrap] building CLI binary at $ROOT/target/debug/sui-sandbox"
    cargo build --bin sui-sandbox >/dev/null
  fi
fi

if [[ -z "$OUTPUT" ]]; then
  short_pkg="${PACKAGE_ID#0x}"
  short_pkg="${short_pkg:0:12}"
  OUTPUT="$ROOT/examples/out/workflow_auto/workflow.auto.${short_pkg}.${FORMAT}"
fi
mkdir -p "$(dirname "$OUTPUT")"

auto_cmd=("$CLI_BIN" workflow auto --package-id "$PACKAGE_ID" --format "$FORMAT" --output "$OUTPUT")
if [[ -n "$TEMPLATE" ]]; then
  auto_cmd+=(--template "$TEMPLATE")
fi
if [[ -n "$DIGEST" ]]; then
  auto_cmd+=(--digest "$DIGEST" --checkpoint "$CHECKPOINT")
fi
if [[ "$BEST_EFFORT" == "1" ]]; then
  auto_cmd+=(--best-effort)
fi
if [[ "$FORCE" == "1" ]]; then
  auto_cmd+=(--force)
fi

echo "[workflow-auto-bootstrap] generating draft workflow"
"${auto_cmd[@]}"

echo "[workflow-auto-bootstrap] validating workflow: $OUTPUT"
"$CLI_BIN" workflow validate --spec "$OUTPUT"

echo "[workflow-auto-bootstrap] dry-run plan"
"$CLI_BIN" workflow run --spec "$OUTPUT" --dry-run

echo "[workflow-auto-bootstrap] PASS"
echo "Generated spec: $OUTPUT"
