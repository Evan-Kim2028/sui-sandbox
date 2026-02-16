#!/usr/bin/env python3
"""Extract a Move interface from an on-chain package (DeepBook margin).

Example output (run on 2026-02-16):
{'source': 'package_id', 'package_id': '0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b', 'bytecode_dir': None, 'modules': 12, 'structs': 59, 'functions': 285, 'module_samples': ['margin_constants', 'margin_manager', 'margin_pool', 'margin_registry', 'margin_state', 'oracle', 'pool_proxy', 'position_manager', 'protocol_config', 'protocol_fees']}
"""

import os
import sui_sandbox

# DeepBook margin package on mainnet (override with PACKAGE_ID).
DEFAULT_PACKAGE = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b"
package_id = os.getenv("PACKAGE_ID", DEFAULT_PACKAGE)
rpc_url = os.getenv("RPC_URL", "https://fullnode.mainnet.sui.io:443")
bytecode_dir = os.getenv("BYTECODE_DIR")

# Resolve interface from local bytecode (offline) or on-chain package (GraphQL).
if bytecode_dir:
    interface = sui_sandbox.extract_interface(bytecode_dir=bytecode_dir)
    source = "bytecode_dir"
else:
    interface = sui_sandbox.extract_interface(package_id=package_id, rpc_url=rpc_url)
    source = "package_id"

modules = interface.get("modules", {})
module_names = sorted(modules.keys())
struct_count = sum(len(m.get("structs", {})) for m in modules.values())
function_count = sum(len(m.get("functions", {})) for m in modules.values())

summary = {
    "source": source,
    "package_id": package_id,
    "bytecode_dir": bytecode_dir,
    "modules": len(module_names),
    "structs": struct_count,
    "functions": function_count,
    "module_samples": module_names[:10],
}
print(summary)
# Example output:
# {'source': 'package_id', 'package_id': '0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b', 'bytecode_dir': None, 'modules': 12, 'structs': 59, 'functions': 285, 'module_samples': ['margin_constants', 'margin_manager', 'margin_pool', 'margin_registry', 'margin_state', 'oracle', 'pool_proxy', 'position_manager', 'protocol_config', 'protocol_fees']}
