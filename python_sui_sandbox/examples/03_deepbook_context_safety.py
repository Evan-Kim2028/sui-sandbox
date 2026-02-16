#!/usr/bin/env python3
"""Prepare a DeepBook context artifact (safety/orchestration layer).

Example output (run on 2026-02-16):
{'package_id': '0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b',
 'packages_fetched': 5,
 'package_sample': ['0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b', '0x8d97f1cd6ac663735be08d1d2b6d02a159e711586461306ce60a2b7a6a565a9e', '0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270'],
 'with_deps': True,
 'context_path': 'examples/out/contexts/context.deepbook_margin.json'}
"""

import os
import sui_sandbox

# DeepBook margin package on mainnet.
MARGIN_PKG = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b"
OUTPUT_PATH = os.getenv(
    "CONTEXT_PATH",
    "examples/out/contexts/context.deepbook_margin.json",
)

# Safety step: prefetch and pin package closure before replay execution.
ctx = sui_sandbox.context_prepare(
    MARGIN_PKG,
    resolve_deps=True,
    output_path=OUTPUT_PATH,
)

packages_value = ctx.get("packages_fetched", ctx.get("count"))
if isinstance(packages_value, list):
    package_count = len(packages_value)
    package_sample = packages_value[:3]
else:
    package_count = int(packages_value or 0)
    package_sample = []

summary = {
    "package_id": ctx.get("package_id"),
    "packages_fetched": package_count,
    "package_sample": package_sample,
    "with_deps": ctx.get("with_deps", ctx.get("resolve_deps")),
    "context_path": OUTPUT_PATH,
}
print(summary)
# Example output:
# {'package_id': '0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b', 'packages_fetched': 5, 'package_sample': ['0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b', '0x8d97f1cd6ac663735be08d1d2b6d02a159e711586461306ce60a2b7a6a565a9e', '0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270'], 'with_deps': True, 'context_path': 'examples/out/contexts/context.deepbook_margin.json'}
