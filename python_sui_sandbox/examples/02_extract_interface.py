#!/usr/bin/env python3
"""Example 2: extract package interface summary via Python bindings."""

from __future__ import annotations

import argparse

import sui_sandbox


def main() -> None:
    parser = argparse.ArgumentParser(description="Extract Move package interface")
    parser.add_argument(
        "--package-id",
        default="0x2",
        help="Package ID to fetch via GraphQL (default: 0x2)",
    )
    parser.add_argument(
        "--bytecode-dir",
        default=None,
        help="Optional local bytecode_modules directory (alternative to --package-id)",
    )
    parser.add_argument(
        "--module-limit",
        type=int,
        default=10,
        help="How many modules to print in summary (default: 10)",
    )
    args = parser.parse_args()

    if args.bytecode_dir:
        interface = sui_sandbox.extract_interface(bytecode_dir=args.bytecode_dir)
        source = f"bytecode_dir={args.bytecode_dir}"
    else:
        interface = sui_sandbox.extract_interface(package_id=args.package_id)
        source = f"package_id={args.package_id}"

    modules = interface.get("modules", {})
    module_names = sorted(modules.keys())

    fn_count = sum(len(mod_data.get("functions", {})) for mod_data in modules.values())
    struct_count = sum(len(mod_data.get("structs", {})) for mod_data in modules.values())

    print("=== Package Interface Summary ===")
    print(f"Source:              {source}")
    print(f"Modules:             {len(module_names)}")
    print(f"Structs:             {struct_count}")
    print(f"Functions:           {fn_count}")

    if module_names:
        print(f"\nTop {min(args.module_limit, len(module_names))} module names:")
        for name in module_names[: args.module_limit]:
            mod_data = modules.get(name, {})
            print(
                f"- {name} "
                f"(functions={len(mod_data.get('functions', {}))}, "
                f"structs={len(mod_data.get('structs', {}))})"
            )


if __name__ == "__main__":
    main()
