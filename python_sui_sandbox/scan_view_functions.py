#!/usr/bin/env python3
"""Scan the ~1000 most-used Sui mainnet packages for view functions.

View functions are public, read-only functions that return values without
mutating state. Detection rule:
  visibility == "public" AND len(returns) > 0 AND no &mut params

Packages are collected from:
  https://github.com/MystenLabs/sui-packages/tree/main/packages/mainnet_most_used

Usage:
  python scan_view_functions.py [--workers 4] [--delay 0.05] [--output-dir ./scan_results] [--resume] [--refresh] [--retry-errors]
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor, ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

# ---------------------------------------------------------------------------
# GitHub helpers
# ---------------------------------------------------------------------------

GITHUB_API = "https://api.github.com"
REPO = "MystenLabs/sui-packages"
BRANCH = "main"
MOST_USED_PATH = "packages/mainnet_most_used"
MAINNET_PATH = "packages/mainnet"

def _gh_api(endpoint: str) -> dict | list:
    """Call the GitHub REST API via the `gh` CLI (handles auth automatically).

    Falls back to unauthenticated urllib if `gh` is not available.
    """
    try:
        result = subprocess.run(
            ["gh", "api", endpoint],
            capture_output=True, text=True, timeout=30,
        )
        if result.returncode == 0:
            return json.loads(result.stdout)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Fallback: unauthenticated request
    url = f"{GITHUB_API}/{endpoint}" if not endpoint.startswith("http") else endpoint
    headers = {"Accept": "application/vnd.github.v3+json"}
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if token:
        headers["Authorization"] = f"token {token}"
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


def _gh_get_raw(path: str) -> bytes:
    """GET raw file content from the repo's default branch.

    Uses `gh api` for authentication, falls back to raw.githubusercontent.com.
    """
    api_path = f"repos/{REPO}/contents/{path}?ref={BRANCH}"
    try:
        result = subprocess.run(
            ["gh", "api", api_path, "-H", "Accept: application/vnd.github.raw+json"],
            capture_output=True, timeout=30,
        )
        if result.returncode == 0:
            return result.stdout
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Fallback
    url = f"https://raw.githubusercontent.com/{REPO}/{BRANCH}/{path}"
    req = urllib.request.Request(url)
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read()


def _list_directory_entries(dir_path: str) -> list[str]:
    """List entry names in a repo directory via the GitHub Contents API."""
    data = _gh_api(f"repos/{REPO}/contents/{dir_path}?ref={BRANCH}")
    if isinstance(data, list):
        return [e["name"] for e in data]
    raise RuntimeError(f"Expected list from contents API for {dir_path}, got {type(data)}")


def _collect_all_package_ids() -> list[str]:
    """Walk the two-level hex-prefix directory structure and return full package IDs.

    The mainnet_most_used/ directory is sharded by hex prefix:
      mainnet_most_used/0x00/<rest_of_address> -> symlink to ../../mainnet/0x00/<rest>
      mainnet_most_used/0x05/<rest_of_address> -> symlink to ../../mainnet/0x05/<rest>

    Full package ID = prefix + entry_name (e.g., "0x05" + "f51d...b1" = "0x05f51d...b1").
    """
    prefix_dirs = _list_directory_entries(MOST_USED_PATH)
    # Filter to hex prefix directories (0x00..0xff)
    prefix_dirs = [d for d in prefix_dirs if d.startswith("0x") and len(d) <= 4]

    all_ids = []
    for prefix in prefix_dirs:
        subdir_path = f"{MOST_USED_PATH}/{prefix}"
        try:
            entries = _list_directory_entries(subdir_path)
        except Exception:
            continue
        for entry in entries:
            full_id = prefix + entry
            all_ids.append(full_id)

    return all_ids


def _fetch_metadata(package_id: str) -> dict | None:
    """Fetch metadata.json for a package given its full ID.

    The repo is sharded: packages/mainnet/<prefix>/<rest>/metadata.json
    where prefix = first 4 chars (e.g., "0x05") and rest = remaining chars.
    """
    prefix = package_id[:4]  # e.g. "0x05"
    rest = package_id[4:]    # remaining hex chars

    # Try mainnet path (actual data location)
    path = f"{MAINNET_PATH}/{prefix}/{rest}/metadata.json"
    try:
        raw = _gh_get_raw(path)
        return json.loads(raw)
    except Exception:
        pass

    # Fallback: try mainnet_most_used path (GitHub resolves symlinks)
    path = f"{MOST_USED_PATH}/{prefix}/{rest}/metadata.json"
    try:
        raw = _gh_get_raw(path)
        return json.loads(raw)
    except Exception:
        return None


# ---------------------------------------------------------------------------
# Package manifest: collect & deduplicate
# ---------------------------------------------------------------------------

def collect_package_manifest(output_dir: Path, refresh: bool = False) -> list[dict]:
    """Build or load the deduplicated package manifest.

    Returns a list of dicts with: package_id, original_package_id, version,
    checkpoint, all_versions.
    """
    manifest_path = output_dir / "package_manifest.json"
    if manifest_path.exists() and not refresh:
        print(f"Loading cached manifest from {manifest_path}")
        with open(manifest_path) as f:
            return json.load(f)

    print("Collecting package entries from GitHub...")
    entries = _collect_all_package_ids()
    total_entries = len(entries)
    print(f"  Found {total_entries} package IDs in mainnet_most_used/")

    # Fetch metadata in parallel (I/O-bound, threads are fine)
    all_versions: dict[str, list[dict]] = defaultdict(list)
    fetch_errors = []

    print(f"  Fetching metadata for {total_entries} packages (8 threads)...")
    metadata_results: dict[str, dict | None] = {}
    with ThreadPoolExecutor(max_workers=8) as pool:
        future_to_entry = {pool.submit(_fetch_metadata, e): e for e in entries}
        done = 0
        for future in as_completed(future_to_entry):
            entry = future_to_entry[future]
            done += 1
            if done % 100 == 0:
                print(f"  Fetching metadata [{done}/{total_entries}]...")
            try:
                metadata_results[entry] = future.result()
            except Exception:
                metadata_results[entry] = None

    for entry in entries:
        meta = metadata_results.get(entry)
        if meta is None:
            fetch_errors.append(entry)
            continue

        pkg_id = meta.get("id", entry)
        original_id = meta.get("originalPackageId", pkg_id)
        version = meta.get("version", 1)
        checkpoint = meta.get("checkpoint")

        # Normalize: if version is a string, parse it
        if isinstance(version, str):
            try:
                version = int(version)
            except ValueError:
                version = 1

        all_versions[original_id].append({
            "package_id": pkg_id,
            "original_package_id": original_id,
            "version": version,
            "checkpoint": checkpoint,
        })

    # Deduplicate: keep highest version per original package
    manifest = []
    duplicates_skipped = 0
    for original_id, versions in all_versions.items():
        versions.sort(key=lambda v: v["version"], reverse=True)
        best = versions[0].copy()
        best["all_versions"] = [
            f"{v['package_id']}(v{v['version']})" for v in versions
        ]
        manifest.append(best)
        duplicates_skipped += len(versions) - 1

    manifest.sort(key=lambda m: m["package_id"])

    print(f"  {total_entries} entries -> {len(manifest)} unique packages "
          f"({duplicates_skipped} older versions skipped)")
    if fetch_errors:
        print(f"  {len(fetch_errors)} entries failed metadata fetch: "
              f"{fetch_errors[:5]}{'...' if len(fetch_errors) > 5 else ''}")

    # Save manifest
    output_dir.mkdir(parents=True, exist_ok=True)
    with open(manifest_path, "w") as f:
        json.dump(manifest, f, indent=2)
    print(f"  Saved manifest to {manifest_path}")

    return manifest


# ---------------------------------------------------------------------------
# View function detection
# ---------------------------------------------------------------------------

def _has_mut_ref(param: dict) -> bool:
    """Check if a parameter type contains a &mut reference anywhere."""
    if not isinstance(param, dict):
        return False
    kind = param.get("kind")
    if kind == "ref" and param.get("mutable", False):
        return True
    # Recurse into nested types
    if kind == "ref":
        return _has_mut_ref(param.get("to", {}))
    if kind == "vector":
        return _has_mut_ref(param.get("type", {}))
    if kind == "datatype":
        return any(_has_mut_ref(a) for a in param.get("type_args", []))
    return False


def _is_primitive_type(param: dict) -> bool:
    """Check if a type is a primitive (not a reference to a struct)."""
    if not isinstance(param, dict):
        return False
    kind = param.get("kind", "")
    if kind in ("bool", "u8", "u16", "u32", "u64", "u128", "u256", "address", "signer"):
        return True
    if kind == "vector":
        return _is_primitive_type(param.get("type", {}))
    if kind == "type_param":
        return True  # treat generics as primitive for categorization
    return False


def _is_immutable_ref(param: dict) -> bool:
    """Check if a param is an immutable reference."""
    return (isinstance(param, dict)
            and param.get("kind") == "ref"
            and not param.get("mutable", False))


def _categorize_view_function(params: list[dict], returns: list[dict]) -> str:
    """Categorize a view function based on its signature.

    Categories:
      getter   - Single &T param (immutable ref), returns 1 value
      predicate - Returns exactly 1 bool
      compute  - All params are primitives, returns value(s)
      query    - Everything else
    """
    # Predicate: returns exactly 1 bool
    if (len(returns) == 1
            and isinstance(returns[0], dict)
            and returns[0].get("kind") == "bool"):
        return "predicate"

    # Getter: single immutable-ref param, returns 1 value
    if (len(params) == 1
            and _is_immutable_ref(params[0])
            and len(returns) == 1):
        return "getter"

    # Compute: all params are primitives (no references at all)
    if all(_is_primitive_type(p) for p in params):
        return "compute"

    return "query"


def extract_view_functions(package_id: str, interface: dict) -> list[dict]:
    """Extract view functions from a package interface JSON.

    A view function is: public + has returns + no &mut params.
    """
    view_fns = []
    modules = interface.get("modules", {})

    for mod_name, mod_data in modules.items():
        if not isinstance(mod_data, dict):
            continue
        functions = mod_data.get("functions", {})
        for fn_name, fn_data in functions.items():
            if not isinstance(fn_data, dict):
                continue

            # Must be public
            visibility = fn_data.get("visibility", "")
            if visibility != "public":
                continue

            # Must have return values
            returns = fn_data.get("returns", [])
            if not returns:
                continue

            # Must not have any &mut params
            params = fn_data.get("params", [])
            if any(_has_mut_ref(p) for p in params):
                continue

            category = _categorize_view_function(params, returns)
            target = f"{package_id}::{mod_name}::{fn_name}"

            view_fns.append({
                "module": mod_name,
                "function": fn_name,
                "target": target,
                "category": category,
                "visibility": visibility,
                "is_entry": fn_data.get("is_entry", False),
                "type_params": fn_data.get("type_params", []),
                "params": params,
                "returns": returns,
            })

    return view_fns


# ---------------------------------------------------------------------------
# Single-package scan (used by both sequential and parallel paths)
# ---------------------------------------------------------------------------

def scan_single_package(
    package_id: str,
    original_package_id: str,
    version: int,
    rpc_url: str = "https://fullnode.mainnet.sui.io:443",
) -> dict:
    """Scan one package and return its results dict.

    This function is designed to run in a subprocess via ProcessPoolExecutor.
    It imports sui_move_extractor locally so it works across process boundaries.
    """
    import sui_move_extractor

    t0 = time.monotonic()
    try:
        interface = sui_move_extractor.extract_interface(
            package_id=package_id,
            rpc_url=rpc_url,
        )
        view_fns = extract_view_functions(package_id, interface)

        # Count modules and total functions
        modules = interface.get("modules", {})
        module_count = len(modules)
        total_functions = sum(
            len(m.get("functions", {}))
            for m in modules.values()
            if isinstance(m, dict)
        )

        elapsed = time.monotonic() - t0
        return {
            "ok": True,
            "package_id": package_id,
            "original_package_id": original_package_id,
            "version": version,
            "module_count": module_count,
            "total_functions": total_functions,
            "view_function_count": len(view_fns),
            "view_functions": view_fns,
            "elapsed_seconds": round(elapsed, 3),
            "error": None,
        }
    except Exception as e:
        elapsed = time.monotonic() - t0
        return {
            "ok": False,
            "package_id": package_id,
            "original_package_id": original_package_id,
            "version": version,
            "module_count": 0,
            "total_functions": 0,
            "view_function_count": 0,
            "view_functions": [],
            "elapsed_seconds": round(elapsed, 3),
            "error": str(e),
        }


# ---------------------------------------------------------------------------
# Resume support
# ---------------------------------------------------------------------------

def _load_progress(progress_path: Path) -> dict[str, dict]:
    """Load previously completed results from the progress file."""
    completed = {}
    if not progress_path.exists():
        return completed
    with open(progress_path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
                pid = entry.get("package_id")
                if pid:
                    completed[pid] = entry
            except json.JSONDecodeError:
                continue
    return completed


# ---------------------------------------------------------------------------
# Main scan loop
# ---------------------------------------------------------------------------

def run_scan(
    manifest: list[dict],
    output_dir: Path,
    workers: int = 1,
    delay: float = 0.05,
    resume: bool = False,
    retry_errors: bool = False,
    rpc_url: str = "https://fullnode.mainnet.sui.io:443",
) -> dict:
    """Run the scan across all packages in the manifest."""
    progress_path = output_dir / "scan_progress.jsonl"
    errors_path = output_dir / "scan_errors.json"
    output_path = output_dir / "view_functions.json"
    flat_path = output_dir / "view_functions_flat.jsonl"

    # Load prior progress for resume
    completed = {}
    if resume:
        completed = _load_progress(progress_path)
        print(f"Resume: {len(completed)} packages already processed")

    # Determine which packages to scan
    to_scan = []
    for pkg in manifest:
        pid = pkg["package_id"]
        if pid in completed:
            prior = completed[pid]
            if retry_errors and not prior.get("ok", True):
                to_scan.append(pkg)  # Retry failed packages
            # else skip
        else:
            to_scan.append(pkg)

    print(f"Scanning {len(to_scan)} packages ({len(manifest) - len(to_scan)} skipped)...")
    if not to_scan and not completed:
        print("Nothing to scan.")
        return {}

    # Open progress file for appending
    progress_file = open(progress_path, "a")
    results = list(completed.values())
    scan_start = time.monotonic()

    try:
        if workers <= 1:
            # Sequential scan
            for i, pkg in enumerate(to_scan):
                result = scan_single_package(
                    pkg["package_id"],
                    pkg["original_package_id"],
                    pkg["version"],
                    rpc_url,
                )
                results.append(result)

                # Write progress
                progress_file.write(json.dumps(result) + "\n")
                progress_file.flush()

                # Print progress
                cat_counts = defaultdict(int)
                for vf in result["view_functions"]:
                    cat_counts[vf["category"]] += 1
                cat_str = " ".join(f"{k}={v}" for k, v in sorted(cat_counts.items()))
                status = "OK" if result["ok"] else "ERR"
                total_done = len(results)
                total_all = len(manifest)
                pkg_short = pkg["package_id"][:10] + "..." + pkg["package_id"][-4:]
                print(
                    f"  [{total_done}/{total_all}] {pkg_short}  "
                    f"{result['elapsed_seconds']:.2f}s  "
                    f"{result['view_function_count']} view fns  "
                    f"({cat_str})  [{status}]"
                )

                if delay > 0:
                    time.sleep(delay)
        else:
            # Parallel scan with ProcessPoolExecutor
            futures = {}
            with ProcessPoolExecutor(max_workers=workers) as executor:
                for pkg in to_scan:
                    future = executor.submit(
                        scan_single_package,
                        pkg["package_id"],
                        pkg["original_package_id"],
                        pkg["version"],
                        rpc_url,
                    )
                    futures[future] = pkg

                for future in as_completed(futures):
                    pkg = futures[future]
                    try:
                        result = future.result()
                    except Exception as e:
                        result = {
                            "ok": False,
                            "package_id": pkg["package_id"],
                            "original_package_id": pkg["original_package_id"],
                            "version": pkg["version"],
                            "module_count": 0,
                            "total_functions": 0,
                            "view_function_count": 0,
                            "view_functions": [],
                            "elapsed_seconds": 0,
                            "error": str(e),
                        }

                    results.append(result)

                    # Write progress
                    progress_file.write(json.dumps(result) + "\n")
                    progress_file.flush()

                    # Print progress
                    cat_counts = defaultdict(int)
                    for vf in result["view_functions"]:
                        cat_counts[vf["category"]] += 1
                    cat_str = " ".join(f"{k}={v}" for k, v in sorted(cat_counts.items()))
                    status = "OK" if result["ok"] else "ERR"
                    total_done = len(results)
                    total_all = len(manifest)
                    pkg_short = pkg["package_id"][:10] + "..." + pkg["package_id"][-4:]
                    print(
                        f"  [{total_done}/{total_all}] {pkg_short}  "
                        f"{result['elapsed_seconds']:.2f}s  "
                        f"{result['view_function_count']} view fns  "
                        f"({cat_str})  [{status}]"
                    )
    finally:
        progress_file.close()

    scan_elapsed = time.monotonic() - scan_start

    # Separate successes and errors
    successes = [r for r in results if r["ok"]]
    errors = [r for r in results if not r["ok"]]

    # Compute category summary
    category_counts = defaultdict(int)
    total_view_fns = 0
    for r in successes:
        for vf in r["view_functions"]:
            category_counts[vf["category"]] += 1
            total_view_fns += 1

    # Build final output
    output = {
        "metadata": {
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "scan_elapsed_seconds": round(scan_elapsed, 1),
            "total_entries": len(manifest) + sum(
                len(p.get("all_versions", [])) - 1 for p in manifest
            ),
            "unique_packages": len(manifest),
            "duplicates_skipped": sum(
                max(0, len(p.get("all_versions", [])) - 1) for p in manifest
            ),
            "packages_scanned": len(results),
            "packages_succeeded": len(successes),
            "packages_failed": len(errors),
            "total_view_functions": total_view_fns,
        },
        "summary_by_category": dict(sorted(category_counts.items())),
        "packages": [],
    }

    # Build per-package entries (successes only, sorted by package_id)
    for r in sorted(successes, key=lambda r: r["package_id"]):
        output["packages"].append({
            "package_id": r["package_id"],
            "original_package_id": r["original_package_id"],
            "version": r["version"],
            "module_count": r["module_count"],
            "total_functions": r["total_functions"],
            "view_functions": r["view_functions"],
        })

    # Write outputs
    output_dir.mkdir(parents=True, exist_ok=True)

    with open(output_path, "w") as f:
        json.dump(output, f, indent=2)
    print(f"\nWrote {output_path}")

    # Flat JSONL: one line per view function
    with open(flat_path, "w") as f:
        for pkg_data in output["packages"]:
            for vf in pkg_data["view_functions"]:
                line = {
                    "package_id": pkg_data["package_id"],
                    "original_package_id": pkg_data["original_package_id"],
                    "module": vf["module"],
                    "function": vf["function"],
                    "category": vf["category"],
                    "target": vf["target"],
                }
                f.write(json.dumps(line) + "\n")
    print(f"Wrote {flat_path}")

    # Errors
    if errors:
        with open(errors_path, "w") as f:
            json.dump(
                [{"package_id": e["package_id"], "error": e["error"]} for e in errors],
                f,
                indent=2,
            )
        print(f"Wrote {errors_path} ({len(errors)} failures)")

    # Print summary
    print(f"\n{'='*60}")
    print(f"Scan complete in {scan_elapsed:.1f}s")
    print(f"  Packages: {len(successes)} succeeded, {len(errors)} failed")
    print(f"  Total view functions: {total_view_fns}")
    print(f"  Categories: {dict(sorted(category_counts.items()))}")
    print(f"{'='*60}")

    return output


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Scan Sui mainnet most-used packages for view functions."
    )
    parser.add_argument(
        "--workers", type=int, default=1,
        help="Number of parallel workers (default: 1, sequential)",
    )
    parser.add_argument(
        "--delay", type=float, default=0.05,
        help="Delay between sequential requests in seconds (default: 0.05)",
    )
    parser.add_argument(
        "--output-dir", type=str, default="./scan_results",
        help="Output directory (default: ./scan_results)",
    )
    parser.add_argument(
        "--resume", action="store_true",
        help="Skip packages already in scan_progress.jsonl",
    )
    parser.add_argument(
        "--refresh", action="store_true",
        help="Force re-fetch of package manifest from GitHub",
    )
    parser.add_argument(
        "--retry-errors", action="store_true",
        help="Re-scan packages that previously failed",
    )
    parser.add_argument(
        "--rpc-url", type=str, default="https://fullnode.mainnet.sui.io:443",
        help="Sui RPC URL for GraphQL endpoint resolution",
    )
    args = parser.parse_args()

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    # Step 1: Collect & deduplicate packages
    manifest = collect_package_manifest(output_dir, refresh=args.refresh)
    if not manifest:
        print("No packages found. Exiting.")
        sys.exit(1)

    # Step 2 & 3: Scan and output
    run_scan(
        manifest=manifest,
        output_dir=output_dir,
        workers=args.workers,
        delay=args.delay,
        resume=args.resume,
        retry_errors=args.retry_errors,
        rpc_url=args.rpc_url,
    )


if __name__ == "__main__":
    main()
