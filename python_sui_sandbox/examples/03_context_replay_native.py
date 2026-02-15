#!/usr/bin/env python3
import os
import sui_sandbox

result = sui_sandbox.context_run(
    os.getenv("PACKAGE_ID", "0x2"),
    os.getenv("DIGEST"),
    checkpoint=int(os.getenv("CHECKPOINT")) if os.getenv("CHECKPOINT") else None,
    state_file=os.getenv("STATE_FILE", "examples/data/state_json_synthetic_ptb_demo.json"),
    analyze_only=os.getenv("ANALYZE_ONLY", "true").lower() in {"1", "true", "yes", "on"},
)
print({"digest": result.get("digest"), "success": result.get("local_success"), "gas_used": result.get("gas_used")})
