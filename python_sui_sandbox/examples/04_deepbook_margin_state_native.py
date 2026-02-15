#!/usr/bin/env python3
"""Example 4: DeepBook margin manager_state via native binding."""

from __future__ import annotations

import os

import sui_sandbox

DEFAULT_VERSIONS_FILE = (
    "examples/advanced/deepbook_margin_state/data/deepbook_versions_240733000.json"
)


out = sui_sandbox.deepbook_margin_state(
    versions_file=os.getenv("VERSIONS_FILE", DEFAULT_VERSIONS_FILE),
    grpc_endpoint=os.getenv("SUI_GRPC_ENDPOINT"),
    grpc_api_key=os.getenv("SUI_GRPC_API_KEY"),
)
print("success:", out.get("success"), "gas_used:", out.get("gas_used"))
print("decoded:", out.get("decoded_margin_state"))
if out.get("error"):
    print("error:", out["error"])
if out.get("hint"):
    print("hint:", out["hint"])
