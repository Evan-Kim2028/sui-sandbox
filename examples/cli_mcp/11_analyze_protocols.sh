#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/_common.sh"

init_context
print_header "Protocol analysis (CLI/MCP analog)"

PACKAGES=(
  "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb"
  "0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf"
  "0x000000000000000000000000000000000000000000000000000000000000dee9"
  "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1"
  "0xa0eba10b173538c8fecca1dff298e488402cc9ff374f8a12ca7758eebe830b66"
  "0x3492c874c1e3b3e2984e8c41b589e642d4d0a5d6459e5a9cfc2d52fd7c89c267"
)

idx=0
for pkg in "${PACKAGES[@]}"; do
  idx=$((idx+1))
  run_tool load_from_mainnet "{\"id\":\"$pkg\",\"kind\":\"package\"}" "${OUT_DIR}/protocol_pkg_${idx}.json"
  echo "Loaded package $pkg"
  sleep 0.2
done

# Lightweight analog to the Rust analyzer: search for 'version' symbols
run_tool search '{"pattern":"version"}' "${OUT_DIR}/protocol_search_version.json"

print_header "Protocol analysis analog complete"
