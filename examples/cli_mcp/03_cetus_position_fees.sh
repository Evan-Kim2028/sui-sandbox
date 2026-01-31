#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/_common.sh"

init_context
print_header "Cetus Position Fees (CLI/MCP analog)"

CETUS_CLMM_PACKAGE_ID="0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb"
CETUS_INTEGRATE_PACKAGE_ID="0x996c4d9480708fb8b92aa7acf819fb0497b5ec8e65ba06601cae2fb6db3312c3"
CETUS_GLOBAL_CONFIG_ID="0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f"

run_tool load_from_mainnet "{\"id\":\"$CETUS_CLMM_PACKAGE_ID\",\"kind\":\"package\"}" "${OUT_DIR}/cetus_clmm_package.json"
run_tool load_from_mainnet "{\"id\":\"$CETUS_INTEGRATE_PACKAGE_ID\",\"kind\":\"package\"}" "${OUT_DIR}/cetus_integrate_package.json"
run_tool load_from_mainnet "{\"id\":\"$CETUS_GLOBAL_CONFIG_ID\",\"kind\":\"object\"}" "${OUT_DIR}/cetus_global_config.json"

# Optional: load pool/position if provided
if [ -n "${CETUS_POOL_ID:-}" ]; then
  run_tool read_object "{\"object_id\":\"${CETUS_POOL_ID}\",\"fetch\":true}" "${OUT_DIR}/cetus_pool.json"
fi
if [ -n "${CETUS_POSITION_ID:-}" ]; then
  run_tool read_object "{\"object_id\":\"${CETUS_POSITION_ID}\",\"fetch\":true}" "${OUT_DIR}/cetus_position.json"
fi

# Quick interface inspection of a core module
run_tool get_interface "{\"package\":\"$CETUS_CLMM_PACKAGE_ID\",\"module\":\"position\"}" "${OUT_DIR}/cetus_position_interface.json"

print_header "Cetus position fees analog complete"
