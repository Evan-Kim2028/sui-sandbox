#!/usr/bin/env python3
"""Call view functions for a Sui wallet using Snowflake data + local Move VM.

End-to-end PoC that:
  1. Queries Snowflake for wallet's owned objects (and their OBJECT_JSON)
  2. Loads view function signatures from scan results
  3. Matches objects to view functions
  4. Converts OBJECT_JSON → BCS via sui_move_extractor.json_to_bcs()
  5. Executes view functions via sui_move_extractor.call_view_function()
  6. Outputs structured results

Usage:
  python call_view_functions.py \
    --wallet 0x1e19b697bb7a332d8e23651cb3fb8247e536eb0846b28602c801e991cb4dbad0 \
    --scan-results ./scan_results/view_functions.json \
    --output-dir ./view_results
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import sys
import time
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

import sui_move_extractor

# ---------------------------------------------------------------------------
# Snowflake connection
# ---------------------------------------------------------------------------

def get_snowflake_connection():
    """Create a Snowflake connection using env vars or SSO."""
    import snowflake.connector

    account = os.environ.get("SNOWFLAKE_ACCOUNT", "xna45413")
    user = os.environ.get("SNOWFLAKE_USER")
    warehouse = os.environ.get("SNOWFLAKE_WAREHOUSE", "DATA_TEAM_WH")
    database = os.environ.get("SNOWFLAKE_DATABASE", "PIPELINE_V2_GROOT_DB")
    schema = os.environ.get("SNOWFLAKE_SCHEMA", "PIPELINE_V2_GROOT_SCHEMA")

    connect_kwargs = dict(
        account=account,
        warehouse=warehouse,
        database=database,
        schema=schema,
    )

    password = os.environ.get("SNOWFLAKE_PASSWORD")
    if user and password:
        connect_kwargs["user"] = user
        connect_kwargs["password"] = password
    elif user:
        connect_kwargs["user"] = user
        connect_kwargs["authenticator"] = "externalbrowser"
    else:
        connect_kwargs["authenticator"] = "externalbrowser"

    return snowflake.connector.connect(**connect_kwargs)


# ---------------------------------------------------------------------------
# Phase A: Snowflake data collection
# ---------------------------------------------------------------------------

WALLET_OBJECTS_SQL = """
SELECT
    ol.object_id,
    ol.object_type AS type,
    ol.version,
    ol.timestamp_ms,
    op2.object_json
FROM PIPELINE_V2_GROOT_DB.PIPELINE_V2_GROOT_SCHEMA.OBJECT_LATEST ol
JOIN PIPELINE_V2_GROOT_DB.PIPELINE_V2_GROOT_SCHEMA.OBJECT_PARQUET2 op2
    ON ol.object_id = op2.object_id
    AND ol.version = op2.object_version
    AND ol.timestamp_ms = op2.timestamp_ms
WHERE ol.owner_address = %(wallet)s
    AND ol.object_status != 'deleted'
LIMIT 500
"""

SHARED_OBJECT_SQL = """
SELECT
    op2.object_id,
    op2.type,
    op2.object_json,
    op2.object_version AS version,
    op2.timestamp_ms
FROM PIPELINE_V2_GROOT_DB.PIPELINE_V2_GROOT_SCHEMA.OBJECT_PARQUET2 op2
WHERE op2.object_id = %(object_id)s
ORDER BY op2.timestamp_ms DESC
LIMIT 1
"""

CHILD_OBJECTS_SQL = """
SELECT
    op2.object_id,
    op2.type,
    op2.object_json,
    op2.object_version AS version
FROM PIPELINE_V2_GROOT_DB.PIPELINE_V2_GROOT_SCHEMA.OBJECT_PARQUET2 op2
JOIN PIPELINE_V2_GROOT_DB.PIPELINE_V2_GROOT_SCHEMA.OBJECT_LATEST ol
    ON op2.object_id = ol.object_id
    AND op2.object_version = ol.version
    AND op2.timestamp_ms = ol.timestamp_ms
WHERE ol.owner_address = %(parent_id)s
    AND ol.owner_type = 'ObjectOwner'
    AND ol.object_status != 'deleted'
LIMIT 200
"""


def fetch_wallet_objects(conn, wallet_address: str) -> list[dict]:
    """Fetch all objects owned by a wallet from Snowflake."""
    print(f"  Querying Snowflake for wallet objects...")
    cur = conn.cursor()
    cur.execute(WALLET_OBJECTS_SQL, {"wallet": wallet_address})
    columns = [desc[0].lower() for desc in cur.description]
    rows = []
    for row in cur:
        obj = dict(zip(columns, row))
        # Parse OBJECT_JSON if it's a string
        if isinstance(obj.get("object_json"), str):
            try:
                obj["object_json"] = json.loads(obj["object_json"])
            except (json.JSONDecodeError, TypeError):
                pass
        rows.append(obj)
    cur.close()
    print(f"  Found {len(rows)} objects")
    return rows


def fetch_shared_object(conn, object_id: str) -> dict | None:
    """Fetch a shared object by ID from Snowflake."""
    cur = conn.cursor()
    cur.execute(SHARED_OBJECT_SQL, {"object_id": object_id})
    columns = [desc[0].lower() for desc in cur.description]
    row = cur.fetchone()
    cur.close()
    if row is None:
        return None
    obj = dict(zip(columns, row))
    if isinstance(obj.get("object_json"), str):
        try:
            obj["object_json"] = json.loads(obj["object_json"])
        except (json.JSONDecodeError, TypeError):
            pass
    return obj


def fetch_child_objects(conn, parent_id: str) -> list[dict]:
    """Fetch dynamic field children of a parent object."""
    cur = conn.cursor()
    cur.execute(CHILD_OBJECTS_SQL, {"parent_id": parent_id})
    columns = [desc[0].lower() for desc in cur.description]
    rows = []
    for row in cur:
        obj = dict(zip(columns, row))
        if isinstance(obj.get("object_json"), str):
            try:
                obj["object_json"] = json.loads(obj["object_json"])
            except (json.JSONDecodeError, TypeError):
                pass
        rows.append(obj)
    cur.close()
    return rows


def extract_cap_references(obj: dict) -> list[str]:
    """Extract object ID references from a capability object's JSON.

    Looks for hex strings matching 0x[a-f0-9]{64} in the JSON values.
    """
    obj_type = obj.get("type", "")
    type_lower = obj_type.lower()

    # Only examine cap/key-like objects
    cap_indicators = ["cap", "key", "owner", "ticket", "receipt"]
    if not any(ind in type_lower for ind in cap_indicators):
        return []

    refs = set()
    obj_json = obj.get("object_json")
    if not isinstance(obj_json, dict):
        return []

    # Recursively find hex addresses in JSON
    def walk(v):
        if isinstance(v, str):
            if re.match(r"^0x[a-f0-9]{64}$", v):
                # Don't include the object's own ID
                own_id = obj.get("object_id", "")
                if v != own_id:
                    refs.add(v)
        elif isinstance(v, dict):
            for val in v.values():
                walk(val)
        elif isinstance(v, list):
            for item in v:
                walk(item)

    walk(obj_json)
    return list(refs)


# ---------------------------------------------------------------------------
# Type parsing utilities
# ---------------------------------------------------------------------------

def parse_type_base(type_str: str) -> tuple[str, str, str, list[str]]:
    """Parse a Sui type string into (package, module, name, type_args).

    Example: "0x2::coin::Coin<0x2::sui::SUI>"
         -> ("0x2", "coin", "Coin", ["0x2::sui::SUI"])
    """
    # Strip type params
    base = type_str
    type_args = []
    if "<" in type_str:
        idx = type_str.index("<")
        base = type_str[:idx]
        inner = type_str[idx + 1 : -1]  # Remove < and >
        type_args = split_type_args(inner)

    parts = base.split("::")
    if len(parts) >= 3:
        return parts[0], parts[1], parts[2], type_args
    return "", "", base, type_args


def split_type_args(s: str) -> list[str]:
    """Split type arguments respecting nested angle brackets."""
    result = []
    depth = 0
    current = []
    for ch in s:
        if ch == "<":
            depth += 1
            current.append(ch)
        elif ch == ">":
            depth -= 1
            current.append(ch)
        elif ch == "," and depth == 0:
            result.append("".join(current).strip())
            current = []
        else:
            current.append(ch)
    if current:
        result.append("".join(current).strip())
    return [r for r in result if r]


def normalize_address(addr: str) -> str:
    """Normalize a Sui address to full 66-char hex."""
    addr = addr.strip()
    if addr.startswith("0x"):
        hex_part = addr[2:]
    else:
        hex_part = addr
    return "0x" + hex_part.zfill(64)


def extract_ref_type(param: dict) -> dict | None:
    """Extract the inner type from a &T or &mut T parameter."""
    if not isinstance(param, dict):
        return None
    if param.get("kind") == "ref":
        return param.get("to")
    return None


def param_type_to_string(param: dict) -> str:
    """Convert a param type dict to a string representation."""
    if not isinstance(param, dict):
        return str(param)
    kind = param.get("kind", "")
    if kind in ("bool", "u8", "u16", "u32", "u64", "u128", "u256", "address"):
        return kind
    if kind == "ref":
        inner = param_type_to_string(param.get("to", {}))
        if param.get("mutable"):
            return f"&mut {inner}"
        return f"&{inner}"
    if kind == "vector":
        inner = param_type_to_string(param.get("type", {}))
        return f"vector<{inner}>"
    if kind == "datatype":
        pkg = param.get("package", "")
        mod_name = param.get("module", "")
        struct_name = param.get("name", "")
        type_args = param.get("type_args", [])
        base = f"{pkg}::{mod_name}::{struct_name}"
        if type_args:
            args = ", ".join(param_type_to_string(a) for a in type_args)
            return f"{base}<{args}>"
        return base
    if kind == "type_param":
        return f"T{param.get('index', '?')}"
    return kind or "?"


def match_type_args(
    fn_param_type: dict,
    concrete_type_str: str,
    type_param_map: dict[int, str],
) -> bool:
    """Try to match a function parameter type against a concrete object type.

    Fills type_param_map with resolved type parameters.
    Returns True if match is possible.
    """
    if not isinstance(fn_param_type, dict):
        return False

    kind = fn_param_type.get("kind", "")

    # Type parameter — always matches, record the mapping
    if kind == "type_param":
        idx = fn_param_type.get("index", 0)
        type_param_map[idx] = concrete_type_str
        return True

    # Datatype — must match package::module::name and recurse into type_args
    if kind == "datatype":
        pkg = normalize_address(fn_param_type.get("package", ""))
        mod_name = fn_param_type.get("module", "")
        struct_name = fn_param_type.get("name", "")

        c_pkg, c_mod, c_name, c_type_args = parse_type_base(concrete_type_str)
        c_pkg = normalize_address(c_pkg)

        if pkg != c_pkg or mod_name != c_mod or struct_name != c_name:
            return False

        fn_type_args = fn_param_type.get("type_args", [])
        if len(fn_type_args) != len(c_type_args):
            return False

        for fn_ta, c_ta in zip(fn_type_args, c_type_args):
            if not match_type_args(fn_ta, c_ta, type_param_map):
                return False
        return True

    # Primitive types — just check they match
    if kind in ("bool", "u8", "u16", "u32", "u64", "u128", "u256", "address"):
        return kind == concrete_type_str

    return False


# ---------------------------------------------------------------------------
# Phase B: Package bytecode collection
# ---------------------------------------------------------------------------

def collect_package_bytecodes(
    package_ids: set[str],
    cache_dir: Path,
) -> dict[str, list[bytes]]:
    """Fetch package bytecodes via GraphQL, with disk cache."""
    result = {}
    to_fetch = set()

    for pkg_id in package_ids:
        norm = normalize_address(pkg_id)
        # Skip framework packages — bundled in the resolver
        if norm in (
            normalize_address("0x1"),
            normalize_address("0x2"),
            normalize_address("0x3"),
        ):
            continue

        cached = cache_dir / norm
        if cached.exists():
            # Load from cache
            modules = []
            for mod_file in sorted(cached.glob("*.mv")):
                modules.append(mod_file.read_bytes())
            if modules:
                result[norm] = modules
                continue

        to_fetch.add(norm)

    if to_fetch:
        print(f"  Fetching {len(to_fetch)} packages via GraphQL (with deps)...")
        for pkg_id in to_fetch:
            try:
                pkg_data = sui_move_extractor.fetch_package_bytecodes(
                    package_id=pkg_id, resolve_deps=True
                )
                packages = pkg_data.get("packages", {})
                for fetched_id, b64_modules in packages.items():
                    norm_id = normalize_address(fetched_id)
                    if norm_id in result:
                        continue
                    modules = []
                    for b64 in b64_modules:
                        modules.append(base64.b64decode(b64))
                    result[norm_id] = modules

                    # Cache to disk
                    pkg_cache = cache_dir / norm_id
                    pkg_cache.mkdir(parents=True, exist_ok=True)
                    for i, mod_bytes in enumerate(modules):
                        (pkg_cache / f"module_{i}.mv").write_bytes(mod_bytes)

            except Exception as e:
                print(f"    Warning: failed to fetch {pkg_id}: {e}")

    return result


# ---------------------------------------------------------------------------
# Phase C: View function matching
# ---------------------------------------------------------------------------

def match_objects_to_view_functions(
    objects: list[dict],
    view_functions: list[dict],
) -> list[dict]:
    """Match wallet objects to compatible view functions.

    Returns a list of {view_fn, objects, type_args} dicts ready for execution.
    """
    matches = []

    # Index view functions by their first param's base type for fast lookup
    # We only match "getter" category (single &T param) for the PoC
    for vf in view_functions:
        params = vf.get("params", [])
        if not params:
            continue

        # Get the first param — must be an immutable reference
        first_param = params[0]
        inner_type = extract_ref_type(first_param)
        if inner_type is None:
            # Not a reference — skip for now (would need pure args)
            continue

        # For each wallet object, try to match against this view function's first param
        for obj in objects:
            obj_type = obj.get("type", "")
            if not obj_type:
                continue

            type_param_map = {}
            if match_type_args(inner_type, obj_type, type_param_map):
                # Build type_args from the map
                fn_type_params = vf.get("type_params", [])
                resolved_type_args = []
                all_resolved = True
                for i in range(len(fn_type_params)):
                    if i in type_param_map:
                        resolved_type_args.append(type_param_map[i])
                    else:
                        all_resolved = False
                        break

                if not all_resolved and fn_type_params:
                    continue

                # Only match single-param view functions for PoC
                if len(params) == 1:
                    matches.append({
                        "package_id": vf.get("package_id", ""),
                        "module": vf["module"],
                        "function": vf["function"],
                        "target": vf["target"],
                        "category": vf.get("category", ""),
                        "type_args": resolved_type_args,
                        "object": obj,
                        "extra_params": [],
                    })

    return matches


# ---------------------------------------------------------------------------
# Phase D: Execution
# ---------------------------------------------------------------------------

def execute_view_function_call(
    match: dict,
    package_bytecodes: dict[str, list[bytes]],
) -> dict:
    """Execute a single view function call and return the result."""
    t0 = time.monotonic()
    obj = match["object"]
    pkg_id = match["package_id"]
    module = match["module"]
    function = match["function"]
    type_args = match["type_args"]
    target = match["target"]

    obj_id = obj["object_id"]
    obj_type = obj["type"]
    obj_json = obj.get("object_json")

    result = {
        "target": target,
        "object_id": obj_id,
        "object_type": obj_type,
        "type_args": type_args,
        "success": False,
        "error": None,
        "return_values": [],
        "elapsed_ms": 0,
    }

    try:
        # Step 1: Collect all module bytecodes needed for BCS conversion
        all_bytecodes = []
        for pkg_modules in package_bytecodes.values():
            all_bytecodes.extend(pkg_modules)

        if not all_bytecodes:
            result["error"] = "No package bytecodes available"
            result["elapsed_ms"] = round((time.monotonic() - t0) * 1000, 1)
            return result

        # Step 2: Convert OBJECT_JSON → BCS
        if obj_json is None:
            result["error"] = "No OBJECT_JSON available"
            result["elapsed_ms"] = round((time.monotonic() - t0) * 1000, 1)
            return result

        obj_json_str = json.dumps(obj_json) if isinstance(obj_json, dict) else str(obj_json)
        bcs_bytes = sui_move_extractor.json_to_bcs(
            type_str=obj_type,
            object_json=obj_json_str,
            package_bytecodes=all_bytecodes,
        )

        # Step 3: Build package_bytecodes dict for call_view_function
        pkg_bytes_dict = {}
        for pid, modules in package_bytecodes.items():
            pkg_bytes_dict[pid] = modules

        # Step 4: Call the view function
        call_result = sui_move_extractor.call_view_function(
            package_id=normalize_address(pkg_id),
            module=module,
            function=function,
            type_args=type_args,
            object_inputs=[{
                "object_id": obj_id,
                "bcs_bytes": bytes(bcs_bytes),
                "type_tag": obj_type,
                "is_shared": False,
            }],
            pure_inputs=[],
            package_bytecodes=pkg_bytes_dict,
            fetch_deps=True,
        )

        result["success"] = call_result.get("success", False)
        result["error"] = call_result.get("error")
        result["return_values"] = call_result.get("return_values", [])
        result["gas_used"] = call_result.get("gas_used", 0)

    except Exception as e:
        result["error"] = str(e)

    result["elapsed_ms"] = round((time.monotonic() - t0) * 1000, 1)
    return result


# ---------------------------------------------------------------------------
# Phase E: Output
# ---------------------------------------------------------------------------

def write_results(
    results: list[dict],
    output_dir: Path,
    wallet: str,
):
    """Write results to output files."""
    output_dir.mkdir(parents=True, exist_ok=True)

    # Full results
    results_path = output_dir / "results.json"
    with open(results_path, "w") as f:
        json.dump(results, f, indent=2, default=str)

    # Flat JSONL
    flat_path = output_dir / "results_flat.jsonl"
    with open(flat_path, "w") as f:
        for r in results:
            f.write(json.dumps(r, default=str) + "\n")

    # Errors only
    errors = [r for r in results if not r.get("success")]
    errors_path = output_dir / "errors.json"
    with open(errors_path, "w") as f:
        json.dump(errors, f, indent=2, default=str)

    # Summary
    succeeded = sum(1 for r in results if r.get("success"))
    failed = sum(1 for r in results if not r.get("success"))
    error_categories = defaultdict(int)
    for r in results:
        if not r.get("success") and r.get("error"):
            # Categorize by first line of error
            err_line = str(r["error"]).split("\n")[0][:100]
            error_categories[err_line] += 1

    summary = {
        "wallet": wallet,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "total_calls": len(results),
        "succeeded": succeeded,
        "failed": failed,
        "success_rate": f"{succeeded / max(len(results), 1) * 100:.1f}%",
        "top_errors": dict(sorted(error_categories.items(), key=lambda x: -x[1])[:10]),
    }
    summary_path = output_dir / "summary.json"
    with open(summary_path, "w") as f:
        json.dump(summary, f, indent=2)

    print(f"\n  Results written to {output_dir}/")
    print(f"    results.json      - {len(results)} call results")
    print(f"    results_flat.jsonl - flat format")
    print(f"    errors.json       - {failed} errors")
    print(f"    summary.json      - stats")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Call view functions for a Sui wallet")
    parser.add_argument(
        "--wallet",
        required=True,
        help="Sui wallet address (0x...)",
    )
    parser.add_argument(
        "--scan-results",
        type=Path,
        default=Path("./scan_results/view_functions.json"),
        help="Path to view_functions.json from scan phase",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("./view_results"),
        help="Output directory for results",
    )
    parser.add_argument(
        "--objects-cache",
        type=Path,
        default=None,
        help="Path to pre-fetched objects JSON (skip Snowflake)",
    )
    parser.add_argument(
        "--max-calls",
        type=int,
        default=50,
        help="Maximum number of view function calls to execute",
    )
    parser.add_argument(
        "--package-cache-dir",
        type=Path,
        default=Path.home() / ".sui-sandbox" / "cache" / "packages",
        help="Directory for caching package bytecodes",
    )
    args = parser.parse_args()

    wallet = normalize_address(args.wallet)
    print(f"View Function Execution PoC")
    print(f"  Wallet: {wallet}")
    print()

    # -----------------------------------------------------------------------
    # Phase A: Get wallet objects
    # -----------------------------------------------------------------------
    print("Phase A: Collecting wallet objects from Snowflake...")
    if args.objects_cache and args.objects_cache.exists():
        print(f"  Loading from cache: {args.objects_cache}")
        with open(args.objects_cache) as f:
            objects = json.load(f)
        print(f"  Loaded {len(objects)} objects from cache")
    else:
        conn = get_snowflake_connection()
        objects = fetch_wallet_objects(conn, wallet)

        # Follow cap indirection
        all_cap_refs = set()
        for obj in objects:
            refs = extract_cap_references(obj)
            all_cap_refs.update(refs)

        if all_cap_refs:
            print(f"  Found {len(all_cap_refs)} cap references, fetching shared objects...")
            shared_objects = []
            for ref_id in all_cap_refs:
                shared = fetch_shared_object(conn, ref_id)
                if shared:
                    shared_objects.append(shared)
            objects.extend(shared_objects)
            print(f"  Added {len(shared_objects)} shared objects")

        conn.close()

        # Cache objects for re-runs
        cache_path = args.output_dir / "objects.json"
        args.output_dir.mkdir(parents=True, exist_ok=True)
        with open(cache_path, "w") as f:
            json.dump(objects, f, indent=2, default=str)
        print(f"  Cached {len(objects)} objects to {cache_path}")

    print(f"  Total objects: {len(objects)}")

    # Show type distribution
    type_counts = defaultdict(int)
    for obj in objects:
        t = obj.get("type", "unknown")
        pkg, mod, name, _ = parse_type_base(t)
        type_counts[f"{pkg}::{mod}::{name}"] += 1
    print(f"  Unique object types: {len(type_counts)}")
    for t, c in sorted(type_counts.items(), key=lambda x: -x[1])[:10]:
        print(f"    {c:4d}x {t}")

    # -----------------------------------------------------------------------
    # Phase B: Load scan results (view function signatures)
    # -----------------------------------------------------------------------
    print(f"\nPhase B: Loading view function signatures...")
    if not args.scan_results.exists():
        print(f"  ERROR: {args.scan_results} not found. Run scan_view_functions.py first.")
        sys.exit(1)

    with open(args.scan_results) as f:
        scan_data = json.load(f)

    # The scan results file is a list of per-package results
    all_view_functions = []
    for pkg_result in scan_data:
        if not isinstance(pkg_result, dict):
            continue
        pkg_id = pkg_result.get("package_id", "")
        for vf in pkg_result.get("view_functions", []):
            vf["package_id"] = pkg_id
            all_view_functions.append(vf)

    print(f"  Loaded {len(all_view_functions)} view functions from {len(scan_data)} packages")

    # -----------------------------------------------------------------------
    # Phase C: Match objects to view functions
    # -----------------------------------------------------------------------
    print(f"\nPhase C: Matching objects to view functions...")
    matches = match_objects_to_view_functions(objects, all_view_functions)
    print(f"  Found {len(matches)} potential matches")

    if not matches:
        print("  No matches found. This could mean:")
        print("    - Wallet objects don't match any scanned view function signatures")
        print("    - Scan results don't cover the wallet's DeFi protocols")
        # Still write empty results
        write_results([], args.output_dir, wallet)
        return

    # Show match distribution
    target_counts = defaultdict(int)
    for m in matches:
        target_counts[m["target"]] += 1
    for t, c in sorted(target_counts.items(), key=lambda x: -x[1])[:10]:
        print(f"    {c:4d}x {t}")

    # Limit matches
    if len(matches) > args.max_calls:
        print(f"  Limiting to {args.max_calls} calls (use --max-calls to increase)")
        matches = matches[: args.max_calls]

    # -----------------------------------------------------------------------
    # Phase B2: Collect package bytecodes
    # -----------------------------------------------------------------------
    print(f"\nPhase B2: Collecting package bytecodes...")
    needed_packages = set()
    for m in matches:
        needed_packages.add(normalize_address(m["package_id"]))
        # Also need packages from type args
        for ta in m.get("type_args", []):
            pkg, _, _, _ = parse_type_base(ta)
            if pkg:
                needed_packages.add(normalize_address(pkg))
        # And from object types
        obj_type = m["object"].get("type", "")
        pkg, _, _, _ = parse_type_base(obj_type)
        if pkg:
            needed_packages.add(normalize_address(pkg))

    args.package_cache_dir.mkdir(parents=True, exist_ok=True)
    package_bytecodes = collect_package_bytecodes(needed_packages, args.package_cache_dir)
    print(f"  Collected bytecodes for {len(package_bytecodes)} packages")

    # -----------------------------------------------------------------------
    # Phase D: Execute view functions
    # -----------------------------------------------------------------------
    print(f"\nPhase D: Executing {len(matches)} view function calls...")
    results = []
    for i, match in enumerate(matches):
        target = match["target"]
        obj_id = match["object"]["object_id"]
        short_obj = obj_id[:10] + "..." if len(obj_id) > 10 else obj_id

        result = execute_view_function_call(match, package_bytecodes)
        results.append(result)

        status = "OK" if result["success"] else "FAIL"
        elapsed = result["elapsed_ms"]
        rv = result.get("return_values", [])
        rv_summary = f"[{len(rv)} values]" if rv else "[]"

        print(f"  [{i + 1}/{len(matches)}] {target} ({short_obj}) "
              f"{elapsed:.0f}ms {status} {rv_summary}")

        if not result["success"] and result.get("error"):
            err = str(result["error"])[:120]
            print(f"           {err}")

    # -----------------------------------------------------------------------
    # Phase E: Output
    # -----------------------------------------------------------------------
    print(f"\nPhase E: Writing results...")
    write_results(results, args.output_dir, wallet)

    # Print summary
    succeeded = sum(1 for r in results if r.get("success"))
    failed = sum(1 for r in results if not r.get("success"))
    print(f"\n  Summary: {succeeded} succeeded, {failed} failed out of {len(results)} calls")


if __name__ == "__main__":
    main()
