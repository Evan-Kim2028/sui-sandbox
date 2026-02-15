#!/usr/bin/env python3
import os
import sui_sandbox

bytecode_dir = os.getenv("BYTECODE_DIR", "tests/fixture/build/fixture")
interface = sui_sandbox.extract_interface(bytecode_dir=bytecode_dir)
print({
    "source": bytecode_dir,
    "modules": len(interface.get("modules", {})),
})
