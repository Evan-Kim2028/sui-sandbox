#!/usr/bin/env python3
"""Extract a Move interface from compiled bytecode.

Example output (run on 2026-02-16):
{'source': 'tests/fixture/build/fixture', 'modules': 1}
"""

import os
import sui_sandbox

# Use test fixture bytecode by default, but allow override.
bytecode_dir = os.getenv("BYTECODE_DIR", "tests/fixture/build/fixture")

# Parse module/function/struct metadata from local bytecode.
interface = sui_sandbox.extract_interface(bytecode_dir=bytecode_dir)

# Print a compact summary for quick validation.
print({
    "source": bytecode_dir,
    "modules": len(interface.get("modules", {})),
})
