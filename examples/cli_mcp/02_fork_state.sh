#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/_common.sh"

init_context
print_header "Fork State via MCP tool"

DEEPBOOK_PACKAGE="0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809"
DEEPBOOK_REGISTRY="0xaf16199a2dff736e9f07a845f23c5da6df6f756eddb631aed9d24a93efc4549d"

# Load DeepBook package + registry object
run_tool load_from_mainnet "{\"id\":\"$DEEPBOOK_PACKAGE\",\"kind\":\"package\"}" "${OUT_DIR}/deepbook_package.json"
run_tool load_from_mainnet "{\"id\":\"$DEEPBOOK_REGISTRY\",\"kind\":\"object\"}" "${OUT_DIR}/deepbook_registry.json"

# Create a tiny local package
PROJECT_OUT="${OUT_DIR}/fork_project.json"
run_tool create_move_project '{"name":"fork_demo","persist":false}' "$PROJECT_OUT"
PROJECT_ID=$(python3 -c "import json; data=json.load(open('$PROJECT_OUT')); print(data['result']['project_id'])")

# Build + deploy
run_tool build_project "{\"project_id\":\"$PROJECT_ID\"}" "${OUT_DIR}/fork_build.json"
run_tool deploy_project "{\"project_id\":\"$PROJECT_ID\"}" "${OUT_DIR}/fork_deploy.json"
PACKAGE_ID=$(python3 -c "import json; data=json.load(open('${OUT_DIR}/fork_deploy.json')); print(data['result']['package_id'])")

# Verify DeepBook package is present in the load output
python3 - "${OUT_DIR}/deepbook_package.json" <<'PY' >/dev/null
import json
import sys

data = json.load(open(sys.argv[1]))
packages = data.get("result", {}).get("packages", [])
assert any(p.get("package_id", "").lower() == "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809" for p in packages), packages
PY

# Call our custom package function
run_tool call_function "{\"package\":\"$PACKAGE_ID\",\"module\":\"fork_demo\",\"function\":\"add\",\"args\":[1,2]}" "${OUT_DIR}/fork_call.json"

print_header "Fork state complete"
