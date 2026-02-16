#!/usr/bin/env python3
"""Replay a context with local state (native path).

Example output (run on 2026-02-16):
{'digest': 'synthetic_make_move_vec_demo', 'success': True, 'gas_used': None}
"""

import os
import sui_sandbox

# Execute replay using env overrides when provided.
result = sui_sandbox.context_run(
    os.getenv("PACKAGE_ID", "0x2"),
    os.getenv("DIGEST"),
    checkpoint=int(os.getenv("CHECKPOINT")) if os.getenv("CHECKPOINT") else None,
    state_file=os.getenv("STATE_FILE", "examples/data/state_json_synthetic_ptb_demo.json"),
    analyze_only=os.getenv("ANALYZE_ONLY", "true").lower() in {"1", "true", "yes", "on"},
)

# Print key fields for quick pass/fail checks.
print({"digest": result.get("digest"), "success": result.get("local_success"), "gas_used": result.get("gas_used")})
