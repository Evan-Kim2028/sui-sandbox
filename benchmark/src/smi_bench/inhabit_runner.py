"""
Phase II benchmark runner (type inhabitation).

High-level responsibilities:
- Derive targets (key structs) from bytecode-derived interface JSON (via the Rust extractor).
- Produce PTB plans using one of:
  - baseline-search (deterministic heuristics over entry functions),
  - template-search (baseline skeleton + LLM fills args), or
  - real-openai-compatible (LLM plans directly with progressive exposure).
- Simulate the PTB via Rust helper `smi_tx_sim` (dry-run / dev-inspect / build-only).
- Score by comparing created object types vs target key types using *base-type* matching.

Maintainability notes:
- The control flow is intentionally “tiered”: parse/schema → build → execute → score.
- For “official” evals, prefer `--simulation-mode dry-run --require-dry-run` to avoid weaker fallbacks.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

import httpx
from rich.console import Console
from rich.progress import track

from smi_bench.agents.real_agent import RealAgent, load_real_agent_config
from smi_bench.dataset import collect_packages, sample_packages
from smi_bench.env import load_dotenv
from smi_bench.inhabit.dryrun import DryRunFailure, classify_dry_run_response
from smi_bench.inhabit.executable_subset import (
    analyze_package,
    summarize_interface,
)
from smi_bench.inhabit.normalize import normalize_ptb_spec
from smi_bench.inhabit.score import InhabitationScore, normalize_type_string, score_inhabitation
from smi_bench.inhabit.validator import validate_ptb_causality_detailed
from smi_bench.logging import JsonlLogger, default_run_id
from smi_bench.runner import _extract_key_types_from_interface_json
from smi_bench.rust import default_rust_binary, emit_bytecode_json, validate_rust_binary
from smi_bench.schema import Phase2ResultKeys, validate_phase2_run_json
from smi_bench.utils import (
    compute_json_checksum,
    ensure_temp_dir,
    get_smi_temp_dir,
    run_json_helper,
    safe_json_loads,
    validate_binary,
)

console = Console()


def _pid_is_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        # Signal 0 does not kill the process; it checks existence/permission.
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        # Exists but we might not be allowed to signal it.
        return True
    else:
        return True


def _check_run_guards(*, parent_pid: int | None, run_deadline: float | None) -> None:
    if parent_pid is not None and parent_pid > 0 and not _pid_is_alive(parent_pid):
        raise RuntimeError(
            f"Parent process exited (pid={parent_pid})\n"
            f"  The benchmark run will stop to prevent orphaned execution.\n"
            f"  This is expected if the parent process was terminated."
        )
    if run_deadline is not None and time.monotonic() >= run_deadline:
        raise TimeoutError(
            f"Maximum run time exceeded\n"
            f"  Deadline: {run_deadline:.1f}s\n"
            f"  Current: {time.monotonic():.1f}s\n"
            f"  The benchmark run has been stopped to respect the time limit."
        )


def _run_rust_emit_bytecode_json(bytecode_package_dir: Path, rust_bin: Path) -> dict[str, Any]:
    """
    Backward-compatible shim for older code/tests.

    Historically, Phase II used a local helper named `_run_rust_emit_bytecode_json(...)`.
    We now centralize the logic in `smi_bench.rust.emit_bytecode_json`, but keep this
    wrapper to avoid drift in call sites and to preserve stable patch targets in tests.

    Args:
        bytecode_package_dir: Path to bytecode package directory.
        rust_bin: Path to Rust extractor binary.

    Returns:
        Parsed interface JSON dict.
    """
    return emit_bytecode_json(package_dir=bytecode_package_dir, rust_bin=rust_bin)


def _fetch_inventory(rpc_url: str, sender: str) -> dict[str, list[str]]:
    """
    Fetch owned objects for sender and group by type.

    Used to resolve placeholder arguments in PTB plans. Fetches all owned objects
    via paginated RPC calls and groups them by normalized type string.

    Args:
        rpc_url: Sui RPC endpoint URL.
        sender: Sender address (must start with "0x").

    Returns:
        Dict mapping normalized type strings to lists of object IDs.
        Example: {"0x2::coin::Coin<0x2::sui::SUI>": ["0xID1", "0xID2"]}

    Note:
        Returns empty dict on any error (network, RPC, parsing) to keep function
        side-effect-free and simplify testing.
    """
    if sender == "0x0" or not sender.startswith("0x"):
        return {}

    try:
        # Simple paginated fetch of all owned objects
        objects = []
        cursor = None
        while True:
            payload = {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "suix_getOwnedObjects",
                "params": [sender, {"filter": None, "options": {"showType": True}}, cursor, 50],
            }
            # Using synchronous httpx.post? We imported httpx.
            # But httpx is usually async or used via client.
            # We can use `httpx.post` (sync).
            # But wait, `inhabit_runner.py` uses `subprocess` mostly.
            # `httpx` is imported.
            resp = httpx.post(rpc_url, json=payload, timeout=30)
            if resp.status_code != 200:
                break
            res = resp.json()
            if "error" in res:
                break
            data = res.get("result", {})
            for item in data.get("data", []):
                objects.append(item)
            if not data.get("hasNextPage"):
                break
            cursor = data.get("nextCursor")
            # Limit inventory fetch to avoid huge wallets slowing down benchmark
            if len(objects) > 200:
                break

        inventory = {}
        for obj in objects:
            t = obj.get("data", {}).get("type")
            oid = obj.get("data", {}).get("objectId")
            if t and oid:
                t_norm = normalize_type_string(t)
                if t_norm not in inventory:
                    inventory[t_norm] = []
                inventory[t_norm].append(oid)
        return inventory
    except (httpx.RequestError, httpx.HTTPStatusError, httpx.TimeoutException, ValueError, KeyError):
        # Fail gracefully on inventory fetch (network issues, RPC errors, or malformed response).
        # Note: keep this function side-effect-free (no logger dependency) to simplify testing.
        return {}


def _resolve_placeholders(ptb_spec: dict[str, Any], inventory: dict[str, list[str]]) -> bool:
    """
    Replace placeholder arguments in PTB spec with actual object IDs from inventory.

    Resolves {"$smi_placeholder": "Type"} arguments by finding matching objects
    in the inventory and replacing with {"imm_or_owned_object": "ID"}.

    Args:
        ptb_spec: PTB specification dict (modified in-place).
        inventory: Dict mapping normalized type strings to object ID lists.

    Returns:
        True if all placeholders were resolved, False otherwise.
    """
    resolved_all = True

    for call in ptb_spec.get("calls", []):
        if not isinstance(call, dict):
            continue
        args = call.get("args", [])
        for i, arg in enumerate(args):
            if not isinstance(arg, dict):
                continue

            ph = arg.get("$smi_placeholder")
            if ph:
                # Resolve it
                ph_norm = normalize_type_string(ph)
                candidates = inventory.get(ph_norm)
                if candidates:
                    # Pick the first one (deterministic)
                    args[i] = {"imm_or_owned_object": candidates[0]}
                else:
                    # If we don't have inventory (common in build-only scans with sender=0x0),
                    # replace placeholders with a deterministic dummy object id.
                    # This keeps the transaction builder from failing on unsupported arg kinds.
                    args[i] = {"imm_or_owned_object": "0x0"}
                    resolved_all = False

    return resolved_all


def _parse_gas_budget_ladder(s: str) -> list[int]:
    """
    Parse gas budget ladder string into list of integers.

    Format: comma-separated integers, e.g., "1000000,5000000,10000000"

    Args:
        s: Comma-separated gas budget values.

    Returns:
        List of gas budget integers in ascending order.

    Raises:
        ValueError: If string format is invalid.
    """
    """
    Parse a comma-separated list of integer gas budgets.

    Example: "20000000,50000000"
    """
    s = s.strip()
    if not s:
        return []
    out: list[int] = []
    for part in s.split(","):
        part = part.strip()
        if not part:
            continue
        try:
            n = int(part)
        except Exception as e:
            raise ValueError(f"invalid gas budget: {part!r}") from e
        if n <= 0:
            continue
        out.append(n)
    # stable unique
    seen: set[int] = set()
    uniq: list[int] = []
    for n in out:
        if n in seen:
            continue
        seen.add(n)
        uniq.append(n)
    return uniq


def _gas_budgets_to_try(*, base: int, ladder: list[int]) -> list[int]:
    out: list[int] = [base]
    for n in ladder:
        if n not in out:
            out.append(n)
    return out


def _is_retryable_gas_error(err: str | None) -> bool:
    if not err:
        return False
    return err == "InsufficientGas"


def _resolve_sender_and_gas_coin(
    *,
    sender: str | None,
    gas_coin: str | None,
    env_overrides: dict[str, str],
) -> tuple[str, str | None]:
    """
    Resolve sender address and gas coin from args or environment.

    Args:
        sender: Sender address from args (None = use env or default).
        gas_coin: Gas coin ID from args (None = use env or None).
        env_overrides: Environment variable overrides dict.

    Returns:
        Tuple of (resolved_sender, resolved_gas_coin).
    """
    resolved_sender = sender or env_overrides.get("SMI_SENDER") or "0x0"
    resolved_gas_coin = gas_coin or env_overrides.get("SMI_GAS_COIN")
    return resolved_sender, resolved_gas_coin


_INT_ARG_KEYS: tuple[str, ...] = (
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "u256",
)


def _rewrite_ptb_addresses_in_place(ptb_spec: dict[str, Any], *, sender: str) -> bool:
    """
    Rewrite address arguments in PTB spec to use sender address.

    Modifies PTB spec in-place, replacing address arguments with sender address.
    Used to create address-rewritten variants of PTB plans.

    Args:
        ptb_spec: PTB specification dict (modified in-place).
        sender: Sender address to use for rewriting.

    Returns:
        True if any addresses were changed, False otherwise.
    """
    """
    Heuristic rewrite: replace any `address` / `vector_address` args with `sender`.

    Useful when a plan uses placeholder addresses but otherwise has a valid call target/shape.
    """
    changed = False
    for call in ptb_spec.get("calls", []):
        if not isinstance(call, dict):
            continue
        args = call.get("args", [])
        if not isinstance(args, list):
            continue
        for arg in args:
            if not isinstance(arg, dict) or len(arg) != 1:
                continue
            k = next(iter(arg.keys()))
            if k == "address" and isinstance(arg.get(k), str):
                if arg[k] != sender:
                    arg[k] = sender
                    changed = True
            elif k == "vector_address" and isinstance(arg.get(k), list):
                v = arg[k]
                if v != [sender] * len(v):
                    arg[k] = [sender] * len(v)
                    changed = True
    return changed


def _rewrite_ptb_ints_in_place(ptb_spec: dict[str, Any], *, value: int) -> bool:
    """
    Rewrite integer arguments in PTB spec to a specific value.

    Modifies PTB spec in-place, replacing integer arguments with the specified value.
    Used to create integer-rewritten variants of PTB plans.

    Args:
        ptb_spec: PTB specification dict (modified in-place).
        value: Integer value to use for rewriting.

    Returns:
        True if any integers were changed, False otherwise.
    """
    """
    Heuristic rewrite: replace any integer-typed pure args with `value`.

    Useful for retrying common MoveAbort conditions caused by invalid literals.
    """
    changed = False
    for call in ptb_spec.get("calls", []):
        if not isinstance(call, dict):
            continue
        args = call.get("args", [])
        if not isinstance(args, list):
            continue
        for arg in args:
            if not isinstance(arg, dict) or len(arg) != 1:
                continue
            k = next(iter(arg.keys()))
            if k in _INT_ARG_KEYS:
                cur = arg.get(k)
                if isinstance(cur, int) and cur != value:
                    arg[k] = value
                    changed = True
    return changed


def _ptb_variants(base_spec: dict[str, Any], *, sender: str, max_variants: int) -> list[tuple[str, dict[str, Any]]]:
    """
    Generate deterministic, bounded PTB variants for local adaptation.

    Creates variants by modifying addresses and integer arguments in the base spec.
    This allows the benchmark to try multiple approaches within a fixed budget
    without making LLM calls.

    Args:
        base_spec: Base PTB specification dict.
        sender: Sender address for address rewriting.
        max_variants: Maximum number of variants to return.

    Returns:
        List of (variant_name, spec_dict) tuples. Variants are deterministic
        and bounded to keep corpus runs fast and diff-stable.
    """
    if max_variants <= 0:
        return []

    variants: list[tuple[str, dict[str, Any]]] = []
    seen: set[str] = set()

    def _add(name: str, spec: dict[str, Any]) -> None:
        key = json.dumps(spec, sort_keys=True, separators=(",", ":"))
        if key in seen:
            return
        seen.add(key)
        variants.append((name, spec))

    _add("base", copy.deepcopy(base_spec))

    if sender and sender != "0x0":
        v = copy.deepcopy(base_spec)
        if _rewrite_ptb_addresses_in_place(v, sender=sender):
            _add("addr_sender", v)

    for n in (0, 2, 10, 100):
        v = copy.deepcopy(base_spec)
        if _rewrite_ptb_ints_in_place(v, value=n):
            _add(f"ints_{n}", v)

    return variants[:max_variants]


def _summarize_inventory(inventory: dict[str, list[str]]) -> str:
    """
    Format inventory dict as a human-readable string for LLM prompts.

    Args:
        inventory: Dict mapping type strings to object ID lists.

    Returns:
        Formatted string describing owned objects grouped by type.
    """
    if not inventory:
        return "Your inventory is empty or could not be fetched."
    lines = ["You own the following objects (grouped by type):"]
    for t, ids in sorted(inventory.items()):
        lines.append(f"- {t}: {len(ids)} objects available")
    return "\n".join(lines)


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def _default_dev_inspect_binary() -> Path:
    exe = "smi_tx_sim.exe" if os.name == "nt" else "smi_tx_sim"
    local = _repo_root() / "target" / "release" / exe
    if local.exists():
        return local
    return Path("/usr/local/bin") / exe


def _load_plan_file(path: Path) -> dict[str, dict]:
    data = json.loads(path.read_text())
    if not isinstance(data, dict):
        raise SystemExit(f"--plan-file must be a JSON object mapping package_id -> PTB spec: {path}")
    out: dict[str, dict] = {}
    for k, v in data.items():
        if isinstance(k, str) and isinstance(v, dict):
            out[k] = v
    return out


def _load_ids_file_ordered(path: Path) -> list[str]:
    out: list[str] = []
    for line in path.read_text().splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        out.append(s)
    return out


def _run_tx_sim_via_helper(
    *,
    dev_inspect_bin: Path,
    rpc_url: str,
    sender: str,
    mode: str,
    gas_budget: int | None,
    gas_coin: str | None,
    bytecode_package_dir: Path | None,
    ptb_spec: dict[str, Any],
    timeout_s: float,
) -> tuple[dict[str, Any] | None, set[str], set[str], str]:
    """
    Run transaction simulation via Rust helper with proper temp file cleanup.

    Writes PTB spec to a temporary file, invokes the Rust helper binary, and
    parses the response. Always cleans up the temp file, even on error.

    Args:
        dev_inspect_bin: Path to smi_tx_sim binary.
        rpc_url: Sui RPC endpoint URL.
        sender: Sender address.
        mode: Simulation mode ("dry-run", "dev-inspect", "build-only").
        gas_budget: Gas budget for transaction (optional).
        gas_coin: Gas coin object ID (optional).
        bytecode_package_dir: Path to bytecode package directory (optional).
        ptb_spec: PTB specification dict.
        timeout_s: Timeout in seconds for subprocess call.

    Returns:
        Tuple of (tx_output_dict, created_types_set, static_types_set, mode_used).
        tx_output_dict may be None if simulation failed.

    Raises:
        TimeoutError: If subprocess times out.
        RuntimeError: If subprocess fails or returns invalid JSON.
    """
    # Write a temporary PTB spec file (small, single package).
    tmp_dir = get_smi_temp_dir()
    tmp_path = tmp_dir / f"ptb_spec_{int(time.time() * 1000)}.json"
    try:
        tmp_path.write_text(json.dumps(ptb_spec, indent=2, sort_keys=True) + "\n")
    except Exception as e:
        raise RuntimeError(
            f"Failed to write temp PTB spec: {tmp_path}\n"
            f"  Error: {e}\n"
            f"  Check disk space and write permissions for: {tmp_path.parent}"
        ) from e

    try:
        cmd = [
            str(dev_inspect_bin),
            "--rpc-url",
            rpc_url,
            "--sender",
            sender,
            "--mode",
            mode,
            "--ptb-spec",
            str(tmp_path),
        ]
        if gas_budget is not None:
            cmd += ["--gas-budget", str(gas_budget)]
        if gas_coin is not None:
            cmd += ["--gas-coin", gas_coin]
        if bytecode_package_dir is not None:
            cmd += ["--bytecode-package-dir", str(bytecode_package_dir)]

        try:
            data = run_json_helper(
                cmd,
                timeout_s=timeout_s,
                context=f"transaction simulation output ({mode})",
            )
        except TimeoutError as e:
            raise TimeoutError(
                f"Transaction simulation timed out after {timeout_s}s\n"
                f"  Mode: {mode}\n"
                f"  RPC: {rpc_url}\n"
                f"  This may indicate network issues or a very complex transaction."
            ) from e
        except RuntimeError as e:
            # Re-wrap RuntimeError to maintain existing error message format expected by callers/tests
            # or just let it bubble up. The original code raised RuntimeError.
            # `run_json_helper` raises RuntimeError with detail.
            # We can just let it bubble or wrap it if we want specific phrasing.
            # The original code added "Check RPC connectivity..." hints.
            raise RuntimeError(f"Transaction simulation failed: {e}") from e

        mode_used = data.get("modeUsed") if isinstance(data.get("modeUsed"), str) else "unknown"
        created_types = data.get("createdObjectTypes")
        static_types = data.get("staticCreatedObjectTypes")
        dry_run = data.get("dryRun")
        dev_inspect = data.get("devInspect")

        created_set = (
            {t for t in created_types if isinstance(t, str) and t} if isinstance(created_types, list) else set()
        )
        static_set = {t for t in static_types if isinstance(t, str) and t} if isinstance(static_types, list) else set()

        # Prefer dry-run if present, otherwise dev-inspect (best-effort).
        tx_out = None
        if isinstance(dry_run, dict):
            tx_out = dry_run
        elif isinstance(dev_inspect, dict):
            tx_out = dev_inspect

        return tx_out, created_set, static_set, mode_used
    finally:
        # Always clean up temp file
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except Exception:
            # Best-effort cleanup; log but don't fail
            pass


def _persist_ptb_spec_for_debug(
    *,
    logger: JsonlLogger | None,
    package_id: str,
    plan_attempt: int,
    plan_variant: str | None,
    sim_attempt: int,
    ptb_spec: dict[str, Any],
) -> None:
    """
    Persist PTB spec to debug directory for inspection.

    Writes PTB spec to a file in the debug directory for post-mortem analysis.
    Only writes if logger is provided (debug mode enabled).

    Args:
        logger: Logger instance (if None, function does nothing).
        package_id: Package ID being processed.
        plan_attempt: Plan attempt number.
        plan_variant: Plan variant name (e.g., "base", "addr_sender").
        sim_attempt: Simulation attempt number.
        ptb_spec: PTB specification dict to persist.
    """
    if logger is None:
        return
    out_dir = logger.paths.root / "ptb_specs"
    out_dir.mkdir(parents=True, exist_ok=True)
    variant = plan_variant or "base"
    out_path = out_dir / f"{package_id}_plan{plan_attempt}_{variant}_sim{sim_attempt}.json"
    out_path.write_text(json.dumps(ptb_spec, indent=2, sort_keys=True) + "\n")


def _run_tx_sim_with_fallback(
    *,
    sim_bin: Path,
    rpc_url: str,
    sender: str,
    gas_budget: int | None,
    gas_coin: str | None,
    bytecode_package_dir: Path | None,
    ptb_spec: dict[str, Any],
    timeout_s: float,
    require_dry_run: bool,
) -> tuple[dict[str, Any] | None, set[str], set[str], str, bool, bool, str | None]:
    """
    Run transaction simulation with fallback from dry-run to dev-inspect.

    Tries dry-run first, then falls back to dev-inspect if dry-run fails and
    require_dry_run is False. This provides graceful degradation for packages
    that can't be dry-run but can be dev-inspected.

    Args:
        sim_bin: Path to smi_tx_sim binary.
        rpc_url: Sui RPC endpoint URL.
        sender: Sender address.
        gas_budget: Gas budget for transaction (optional).
        gas_coin: Gas coin object ID (optional).
        bytecode_package_dir: Path to bytecode package directory (optional).
        ptb_spec: PTB specification dict.
        timeout_s: Timeout in seconds for subprocess call.
        require_dry_run: If True, don't fall back to dev-inspect on dry-run failure.

    Returns:
        Tuple of (tx_output_dict, created_types_set, static_types_set, mode_used,
        dry_run_ok, fell_back_to_dev_inspect, dry_run_error).
    """
    """
    Attempt dry-run first to get transaction-ground-truth created types.
    If dry-run fails and require_dry_run is false, fall back to dev-inspect (static types only).
    """
    try:
        tx_out, created, static_created, mode_used = _run_tx_sim_via_helper(
            dev_inspect_bin=sim_bin,
            rpc_url=rpc_url,
            sender=sender,
            mode="dry-run",
            gas_budget=gas_budget,
            gas_coin=gas_coin,
            bytecode_package_dir=bytecode_package_dir,
            ptb_spec=ptb_spec,
            timeout_s=timeout_s,
        )
        # If dry-run succeeded, created types should already be present.
        return tx_out, created, static_created, mode_used, False, True, None
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, RuntimeError, ValueError, TimeoutError) as e:
        if require_dry_run:
            raise
        dry_run_err = str(e)

    # Dev-inspect fallback: we still call the helper in dev-inspect mode to keep outputs consistent,
    # but scoring relies on static types because dev-inspect does not include object type strings.
    tmp_dir = get_smi_temp_dir()
    tmp_path = tmp_dir / f"ptb_spec_{int(time.time() * 1000)}.json"
    try:
        tmp_path.write_text(json.dumps(ptb_spec, indent=2, sort_keys=True) + "\n")
    except Exception as e:
        raise RuntimeError(f"failed to write temp PTB spec for dev-inspect fallback: {tmp_path}") from e
    try:
        cmd = [
            str(sim_bin),
            "--rpc-url",
            rpc_url,
            "--sender",
            sender,
            "--mode",
            "dev-inspect",
            "--ptb-spec",
            str(tmp_path),
        ]
        if gas_budget is not None:
            cmd += ["--gas-budget", str(gas_budget)]
        if gas_coin is not None:
            cmd += ["--gas-coin", gas_coin]
        if bytecode_package_dir is not None:
            cmd += ["--bytecode-package-dir", str(bytecode_package_dir)]

        data = run_json_helper(
            cmd,
            timeout_s=timeout_s,
            context="tx sim helper output (dev-inspect fallback)",
        )

        mode_used = data.get("modeUsed") if isinstance(data.get("modeUsed"), str) else "unknown"
        created_types = data.get("createdObjectTypes")
        static_types = data.get("staticCreatedObjectTypes")
        created_set = (
            {t for t in created_types if isinstance(t, str) and t} if isinstance(created_types, list) else set()
        )
        static_set = {t for t in static_types if isinstance(t, str) and t} if isinstance(static_types, list) else set()
        return None, created_set, static_set, mode_used, True, False, dry_run_err
    finally:
        # Always clean up temp file
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except (OSError, PermissionError):
            # Best-effort cleanup
            pass


def _build_real_agent_prompt(
    *,
    package_id: str,
    target_key_types: set[str],
    interface_summary: str,
    inventory_summary: str,
    max_planning_calls: int,
) -> str:
    """
    Build prompt for LLM to generate PTB spec JSON.

    The prompt instructs the model to create a Programmable Transaction Block plan
    that will inhabit (create) as many target key types as possible.

    Args:
        package_id: Package ID being targeted.
        target_key_types: Set of key type strings to create.
        interface_summary: Formatted summary of available functions in package.
        inventory_summary: Formatted summary of owned objects.
        max_planning_calls: Maximum number of planning calls allowed.

    Returns:
        Complete prompt string for the LLM.
    """
    instructions = (
        "You are an expert Move developer crafting a Programmable Transaction Block (PTB) plan.\n"
        "Your goal is to inhabit (create) as many of the following target types as possible:\n"
        f"{sorted(target_key_types)}\n\n"
        "### Available Functions in Package\n"
        f"{interface_summary}\n\n"
        "### Your Inventory\n"
        f"{inventory_summary}\n\n"
        "### Standard System Objects\n"
        "- 0x6::clock::Clock (Shared)\n"
        "- 0x8::random::Random (Shared)\n"
        "- 0x403::deny_list::DenyList (Shared)\n\n"
        "### PTB Schema Rules\n"
        "Return ONLY valid JSON matching this schema:\n"
        '{"calls":[{"target":"0xADDR::module::function","type_args":["<TypeTag>",...],'
        '"args":[{"u64":1},{"vector_u8_utf8":"hi"},{"imm_or_owned_object":"0xID"},...] }]}\n'
        "- Use 'imm_or_owned_object' for objects you own.\n"
        '- Use \'shared_object\' for shared objects: {"shared_object": {"id": "0x...", "mutable": true}}\n'
        "- NEVER use arg kinds named 'object' or 'object_id' (unsupported).\n"
        "  If you have an object id, use 'imm_or_owned_object' instead.\n"
        "- Do not include tx_context arguments (implicit).\n"
        "### Progressive Exposure (IMPORTANT)\n"
        "You may request more interface details if needed by returning ONLY this JSON object:\n"
        '{"need_more":["0xADDR::module::function",...],"reason":"..."}\n'
        f"You have at most {max_planning_calls} planning calls total for this package; prefer succeeding in 1.\n"
        "If you do not need more details, return ONLY a PTB JSON plan matching the schema.\n"
        "CRITICAL: Do NOT ask natural-language questions. Output either a PTB plan OR a need_more JSON object.\n"
    )
    payload = {"package_id": package_id, "target_key_types": sorted(target_key_types)}
    return instructions + json.dumps(payload, indent=2, sort_keys=True)


def _build_real_agent_retry_prompt(
    *,
    package_id: str,
    target_key_types: set[str],
    last_failure: dict[str, Any],
    interface_summary: str | None = None,
    max_planning_calls: int | None = None,
) -> str:
    """
    Build failure-aware retry prompt for LLM adaptation.

    Creates a prompt that includes information about the last failure, allowing
    the agent to adapt its approach within the per-package timeout.

    Args:
        package_id: Package ID being targeted.
        target_key_types: Set of key type strings to create.
        last_failure: Dict with failure details (harness_error, dry_run_effects_error, etc.).
        interface_summary: Optional formatted summary of available functions.
        max_planning_calls: Optional maximum planning calls remaining.

    Returns:
        Retry prompt string with failure context.
    """
    error_detail = ""
    if "harness_error" in last_failure:
        harness_err = last_failure["harness_error"]
        error_detail = f"The harness failed to parse your JSON or build the transaction: {harness_err}\n"
    elif "dry_run_effects_error" in last_failure:
        effects_err = last_failure["dry_run_effects_error"]
        error_detail = f"The transaction was built but failed on-chain simulation: {effects_err}\n"

    extra_iface = ""
    if interface_summary:
        extra_iface = "\n### Available Functions (Focused)\n" + interface_summary + "\n"

    budget_line = ""
    if max_planning_calls is not None:
        budget_line = f"You have at most {max_planning_calls} planning calls total; prefer succeeding now.\n"

    instructions = (
        "Your previous PTB plan failed.\n"
        f"{error_detail}"
        "\nRevise the PTB plan to avoid the failure.\n"
        f"{extra_iface}"
        f"{budget_line}"
        "Return ONLY valid JSON matching this schema:\n"
        '{"calls":[{"target":"0xADDR::module::function","type_args":["<TypeTag>",...],'
        '"args":[{"u64":1},{"vector_u8_utf8":"hi"},...] }]}\n'
        "Do NOT use arg kinds named 'object' or 'object_id' (unsupported).\n"
        "Do not include tx_context arguments (they are implicit).\n"
        "CRITICAL: Output a valid JSON plan attempt. Do NOT ask for clarification.\n"
    )
    payload = {
        "package_id": package_id,
        "target_key_types": sorted(target_key_types),
        "last_failure": last_failure,
    }
    return instructions + json.dumps(payload, indent=2, sort_keys=True)


def _build_template_agent_prompt(
    *,
    package_id: str,
    target_key_types: set[str],
    calls: list[dict],
) -> str:
    """
    Prompt the agent to fill in values for a pre-discovered call sequence.
    """
    instructions = (
        "You are an expert Move developer. I have found a sequence of function calls "
        "that might create the following target objects: "
        f"{sorted(target_key_types)}\n\n"
        "Your task is to provide VALID DATA VALUES for the 'args' in the JSON below.\n"
        "Rules:\n"
        "1. Return ONLY the 'calls' JSON array.\n"
        "2. Do NOT change the 'target' or 'type_args'.\n"
        "3. Replace any placeholder values with realistic data (e.g., sensible u64 amounts, valid string content).\n"
        "4. If an argument is a result reference (e.g., {'result': 0}), keep it AS IS.\n"
        "5. If an argument is a placeholder (e.g., {'$smi_placeholder': '...'}), try to find a system object "
        "or keep it if you cannot (the runner will try to resolve it).\n"
    )
    payload = {"package_id": package_id, "skeleton_calls": calls}
    return instructions + json.dumps(payload, indent=2, sort_keys=True)


@dataclass
class InhabitPackageResult:
    package_id: str
    score: InhabitationScore
    error: str | None = None
    elapsed_seconds: float | None = None
    timed_out: bool | None = None
    created_object_types_list: list[str] | None = None
    simulation_mode: str | None = None
    fell_back_to_dev_inspect: bool | None = None
    ptb_parse_ok: bool | None = None
    tx_build_ok: bool | None = None
    dry_run_ok: bool | None = None
    dry_run_exec_ok: bool | None = None
    dry_run_status: str | None = None
    dry_run_effects_error: str | None = None
    dry_run_abort_code: int | None = None
    dry_run_abort_location: str | None = None
    dev_inspect_ok: bool | None = None
    dry_run_error: str | None = None
    plan_attempts: int | None = None
    sim_attempts: int | None = None
    gas_budget_used: int | None = None
    plan_variant: str | None = None
    schema_violation_count: int | None = None
    schema_violation_attempts_until_first_valid: int | None = None
    semantic_failure_count: int | None = None
    semantic_failure_attempts_until_first_success: int | None = None
    # New planning intelligence fields
    formatting_corrections: list[str] | None = None
    formatting_corrections_histogram: dict[str, int] | None = None
    causality_valid: bool | None = None
    causality_score: float | None = None
    causality_errors: list[str] | None = None


@dataclass
class InhabitRunResult:
    """
    Phase II benchmark run result (schema contract).

    Schema versioning:
    - Increment `schema_version` when adding/removing/renaming fields or changing semantics.
    - Backward compatibility: older readers should handle missing optional fields gracefully.
    - Determinism: `packages` list order should be stable (sorted by package_id) for reproducible diffs.

    Field contracts:
    - `schema_version`: integer, must match expected version for strict validation.
    - `aggregate`: dict with keys like `"avg_hit_rate"`, `"packages_total"`,
      `"packages_with_hits"`, `"errors"`, `"timeouts"`.
    - `packages`: list of dicts, each with `"package_id"`, `"score"` (InhabitationScore dict), plus simulation metadata.
    - Simulation metadata fields (`dry_run_ok`, `fell_back_to_dev_inspect`, etc.) indicate evidence quality for scoring.
    """

    schema_version: int
    started_at_unix_seconds: int
    finished_at_unix_seconds: int
    corpus_root_name: str
    samples: int
    seed: int
    agent: str
    rpc_url: str
    sender: str
    gas_budget: int
    gas_coin: str | None
    aggregate: dict[str, Any]
    packages: list[dict[str, Any]]


def _write_checkpoint(out_path: Path, run_result: InhabitRunResult) -> None:
    """
    Write checkpoint atomically with checksum validation.

    Uses a temporary file (.tmp suffix) and atomic replace to ensure checkpoint
    integrity. Validates schema before writing and adds checksum for corruption detection.
    Always cleans up .tmp file on failure.

    Args:
        out_path: Path to checkpoint file.
        run_result: Run result to serialize.

    Raises:
        ValueError: If schema validation fails.
        OSError: If file write fails.
    """
    tmp = out_path.with_suffix(out_path.suffix + ".tmp")
    try:
        data = asdict(run_result)
        # Validate schema before writing
        validate_phase2_run_json(data)
        # Add checksum for corruption detection
        checksum = compute_json_checksum(data)
        data["_checksum"] = checksum
        json_str = json.dumps(data, indent=2, sort_keys=True) + "\n"
        tmp.write_text(json_str)
        tmp.replace(out_path)
    except Exception:
        # Clean up .tmp file on failure to prevent accumulation
        if tmp.exists():
            try:
                tmp.unlink()
            except Exception:
                pass  # Best-effort cleanup
        raise


def _load_checkpoint(out_path: Path) -> InhabitRunResult:
    """
    Load checkpoint with checksum validation.

    Reads checkpoint file, validates checksum if present, and deserializes to InhabitRunResult.
    Provides detailed error messages for common failure modes.

    Args:
        out_path: Path to checkpoint file.

    Returns:
        Deserialized InhabitRunResult.

    Raises:
        FileNotFoundError: If checkpoint file doesn't exist.
        RuntimeError: If file read fails, JSON parse fails, checksum mismatch, or invalid shape.
    """
    try:
        text = out_path.read_text()
    except FileNotFoundError as exc:
        raise FileNotFoundError(
            f"Checkpoint file not found: {out_path}\n"
            f"  Did you mean to run without --resume?\n"
            f"  Or check that the file path is correct."
        ) from exc
    except (OSError, PermissionError) as exc:
        raise RuntimeError(
            f"Failed to read checkpoint file: {out_path}\n  Error: {exc}\n  Check file permissions and disk space."
        ) from exc

    try:
        data = safe_json_loads(text, context=f"checkpoint file {out_path}")
    except ValueError as e:
        raise RuntimeError(
            f"Checkpoint JSON parse error: {out_path}\n"
            f"  {e}\n"
            f"  The checkpoint file may be corrupted. Consider removing it and restarting."
        ) from e

    # Validate checksum if present
    stored_checksum = data.pop("_checksum", None)
    if stored_checksum:
        computed_checksum = compute_json_checksum(data)
        if stored_checksum != computed_checksum:
            raise RuntimeError(
                f"Checkpoint checksum mismatch: {out_path}\n"
                f"  Stored checksum: {stored_checksum}\n"
                f"  Computed checksum: {computed_checksum}\n"
                f"  This indicates file corruption.\n"
                f"  Fix: Remove the checkpoint file and restart without --resume."
            )

    if not isinstance(data, dict):
        raise RuntimeError(
            f"Invalid checkpoint shape: {out_path}\n"
            f"  Expected top-level dict, got {type(data).__name__}\n"
            f"  The checkpoint file may be corrupted or from an incompatible version."
        )
    if not isinstance(data.get("packages"), list):
        raise RuntimeError(
            f"Invalid checkpoint shape: {out_path}\n"
            f"  Expected 'packages' to be list, got {type(data.get('packages')).__name__}\n"
            f"  The checkpoint file may be corrupted or from an incompatible version."
        )
    return InhabitRunResult(
        schema_version=int(data["schema_version"]),
        started_at_unix_seconds=int(data["started_at_unix_seconds"]),
        finished_at_unix_seconds=int(data["finished_at_unix_seconds"]),
        corpus_root_name=str(data["corpus_root_name"]),
        samples=int(data["samples"]),
        seed=int(data["seed"]),
        agent=str(data["agent"]),
        rpc_url=str(data["rpc_url"]),
        sender=str(data["sender"]),
        gas_budget=(int(data["gas_budget"]) if "gas_budget" in data else 10_000_000),
        gas_coin=(str(data["gas_coin"]) if isinstance(data.get("gas_coin"), str) else None),
        aggregate=data.get("aggregate") if isinstance(data.get("aggregate"), dict) else {},
        packages=data["packages"],
    )


def _resume_results_from_checkpoint(
    cp: InhabitRunResult,
    *,
    logger: JsonlLogger | None = None,
) -> tuple[list[InhabitPackageResult], set[str], int, int]:
    """
    Resume package results from a checkpoint.

    Extracts completed packages from checkpoint and reconstructs InhabitPackageResult objects.
    Gracefully skips malformed package rows and logs skip events.

    Args:
        cp: Checkpoint InhabitRunResult to resume from.
        logger: Optional logger for skip events.

    Returns:
        Tuple of (results_list, seen_package_ids_set, error_count, started_timestamp).
    """
    results: list[InhabitPackageResult] = []
    seen: set[str] = set()
    errors = cp.aggregate.get("errors") if isinstance(cp.aggregate, dict) else 0
    try:
        error_count = int(errors) if errors is not None else 0
    except (ValueError, TypeError):
        # Invalid error count format; default to 0
        error_count = 0

    for row in cp.packages:
        if not isinstance(row, dict):
            continue
        pkg_id = row.get("package_id")
        if not isinstance(pkg_id, str) or not pkg_id:
            continue
        score_d = row.get("score")
        if not isinstance(score_d, dict):
            continue
        try:
            score = InhabitationScore(**score_d)
        except (TypeError, ValueError) as e:
            # Log but continue - malformed score shouldn't break resume
            if logger is not None and pkg_id:
                logger.event("checkpoint_resume_skip", package_id=pkg_id, reason=f"invalid score: {e}")
            continue
        sim_mode = row.get("simulation_mode")
        fell_back = row.get("fell_back_to_dev_inspect")
        results.append(
            InhabitPackageResult(
                package_id=pkg_id,
                score=score,
                error=row.get("error"),
                simulation_mode=sim_mode if isinstance(sim_mode, str) else None,
                fell_back_to_dev_inspect=bool(fell_back) if fell_back is not None else None,
            )
        )
        seen.add(pkg_id)
    return results, seen, error_count, cp.started_at_unix_seconds


def _to_package_dict(r: InhabitPackageResult) -> dict[str, Any]:
    k = Phase2ResultKeys
    return {
        k.PACKAGE_ID: r.package_id,
        k.SCORE: asdict(r.score),
        k.ERROR: r.error,
        k.ELAPSED_SECONDS: r.elapsed_seconds,
        k.TIMED_OUT: r.timed_out,
        k.CREATED_OBJECT_TYPES_LIST: r.created_object_types_list,
        k.SIMULATION_MODE: r.simulation_mode,
        k.FELL_BACK_TO_DEV_INSPECT: r.fell_back_to_dev_inspect,
        k.PTB_PARSE_OK: r.ptb_parse_ok,
        k.TX_BUILD_OK: r.tx_build_ok,
        k.DRY_RUN_OK: r.dry_run_ok,
        k.DRY_RUN_EXEC_OK: r.dry_run_exec_ok,
        k.DRY_RUN_STATUS: r.dry_run_status,
        k.DRY_RUN_EFFECTS_ERROR: r.dry_run_effects_error,
        k.DRY_RUN_ABORT_CODE: r.dry_run_abort_code,
        k.DRY_RUN_ABORT_LOCATION: r.dry_run_abort_location,
        k.DEV_INSPECT_OK: r.dev_inspect_ok,
        k.DRY_RUN_ERROR: r.dry_run_error,
        k.PLAN_ATTEMPTS: r.plan_attempts,
        k.SIM_ATTEMPTS: r.sim_attempts,
        k.GAS_BUDGET_USED: r.gas_budget_used,
        k.PLAN_VARIANT: r.plan_variant,
        k.SCHEMA_VIOLATION_COUNT: r.schema_violation_count,
        k.SCHEMA_VIOLATION_ATTEMPTS_UNTIL_FIRST_VALID: r.schema_violation_attempts_until_first_valid,
        k.SEMANTIC_FAILURE_COUNT: r.semantic_failure_count,
        k.SEMANTIC_FAILURE_ATTEMPTS_UNTIL_FIRST_SUCCESS: r.semantic_failure_attempts_until_first_success,
        k.FORMATTING_CORRECTIONS: r.formatting_corrections,
        k.FORMATTING_CORRECTIONS_HISTOGRAM: r.formatting_corrections_histogram,
        k.CAUSALITY_VALID: r.causality_valid,
        k.CAUSALITY_SCORE: r.causality_score,
        k.CAUSALITY_ERRORS: r.causality_errors,
    }


def run(
    *,
    corpus_root: Path,
    samples: int,
    seed: int,
    package_ids_file: Path | None,
    agent_name: str,
    rust_bin: Path,
    dev_inspect_bin: Path,
    rpc_url: str,
    sender: str | None,
    gas_budget: int,
    gas_coin: str | None,
    gas_budget_ladder: str,
    max_planning_calls: int,
    max_plan_attempts: int,
    baseline_max_candidates: int,
    max_heuristic_variants: int,
    plan_file: Path | None,
    env_file: Path | None,
    out_path: Path | None,
    resume: bool,
    continue_on_error: bool,
    max_errors: int,
    per_package_timeout_seconds: float,
    include_created_types: bool,
    require_dry_run: bool,
    simulation_mode: str,
    log_dir: Path | None,
    run_id: str | None,
    parent_pid: int | None = None,
    max_run_seconds: float | None = None,
    checkpoint_every: int = 10,
) -> InhabitRunResult:
    # Validate binaries exist and are executable
    try:
        rust_bin = validate_rust_binary(rust_bin)
    except (FileNotFoundError, PermissionError) as e:
        raise SystemExit(str(e)) from e
    try:
        dev_inspect_bin = validate_binary(dev_inspect_bin, binary_name="dev-inspect helper binary")
    except (FileNotFoundError, PermissionError) as e:
        raise SystemExit(str(e)) from e

    env_overrides = load_dotenv(env_file) if env_file is not None else {}
    sender, gas_coin = _resolve_sender_and_gas_coin(sender=sender, gas_coin=gas_coin, env_overrides=env_overrides)
    plan_by_id: dict[str, dict[str, Any]] = {}
    if plan_file is not None:
        plan_by_id = _load_plan_file(plan_file)

    logger: JsonlLogger | None = None
    if log_dir is not None:
        rid = run_id or default_run_id(prefix="phase2")
        logger = JsonlLogger(base_dir=log_dir, run_id=rid, use_stdout=True)

    started = int(time.time())
    run_deadline = (time.monotonic() + float(max_run_seconds)) if max_run_seconds is not None else None
    if logger is not None:
        logger.write_run_metadata(
            {
                "schema_version": 1,
                "benchmark": "phase2_inhabitation",
                "started_at_unix_seconds": started,
                "agent": agent_name,
                "seed": seed,
                "rpc_url": rpc_url,
                "sender": sender,
                "gas_budget": gas_budget,
                "gas_budget_ladder": gas_budget_ladder,
                "gas_coin": gas_coin,
                "simulation_mode": simulation_mode,
                "max_plan_attempts": max_plan_attempts,
                "max_heuristic_variants": max_heuristic_variants,
                "parent_pid": parent_pid,
                "max_run_seconds": max_run_seconds,
                "argv": list(map(str, sys.argv)),
            }
        )
        logger.event(
            "run_started",
            started_at_unix_seconds=started,
            agent=agent_name,
            seed=seed,
            simulation_mode=simulation_mode,
        )

    packages = collect_packages(corpus_root)
    if package_ids_file is not None:
        ids = _load_ids_file_ordered(package_ids_file)
        by_id = {p.package_id: p for p in packages}
        picked = [by_id[i] for i in ids if i in by_id]
    else:
        picked = sample_packages(packages, samples, seed)
    if not picked:
        raise SystemExit(f"no packages found under: {corpus_root}")

    results: list[InhabitPackageResult] = []
    error_count = 0
    last_partial: InhabitRunResult | None = None
    if resume:
        if out_path is None:
            raise SystemExit("--resume requires --out")
        if out_path.exists():
            cp = _load_checkpoint(out_path)
            if cp.agent != agent_name or cp.seed != seed or cp.rpc_url != rpc_url or cp.sender != sender:
                raise SystemExit("checkpoint mismatch: expected same agent/seed/rpc_url/sender for resume")
            if cp.gas_budget != gas_budget:
                raise SystemExit(
                    f"checkpoint mismatch: out has gas_budget={cp.gas_budget}, expected gas_budget={gas_budget}"
                )
            if cp.gas_coin != gas_coin:
                raise SystemExit(f"checkpoint mismatch: out has gas_coin={cp.gas_coin}, expected gas_coin={gas_coin}")
            results, seen, error_count, started = _resume_results_from_checkpoint(cp, logger=logger)
            picked = [p for p in picked if p.package_id not in seen]
            console.print(f"[yellow]resuming:[/yellow] already_done={len(seen)} remaining={len(picked)}")

    if package_ids_file is not None:
        # Treat --samples as a batch size over the manifest order (works with --resume).
        if samples > 0 and samples < len(picked):
            picked = picked[:samples]

    real_agent: RealAgent | None = None
    if agent_name == "real-openai-compatible":
        cfg = load_real_agent_config(env_overrides)
        real_agent = RealAgent(cfg)
        if logger is not None:
            logger.event("agent_effective_config", **real_agent.debug_effective_config())
    elif agent_name == "template-search":
        cfg = load_real_agent_config(env_overrides)
        real_agent = RealAgent(cfg)
        if logger is not None:
            logger.event("agent_effective_config", **real_agent.debug_effective_config())
    elif agent_name in {"mock-empty", "mock-planfile", "baseline-search"}:
        pass
    else:
        raise SystemExit(f"unknown agent: {agent_name}")

    done_already = len(results)
    for pkg_i, pkg in enumerate(track(picked, description="phase2"), start=done_already + 1):
        pkg_started = time.monotonic()
        deadline = pkg_started + per_package_timeout_seconds

        pkg_guard_error: str | None = None
        pkg_guard_timed_out = False
        try:
            _check_run_guards(parent_pid=parent_pid, run_deadline=run_deadline)
        except TimeoutError as e:
            pkg_guard_error = str(e)
            pkg_guard_timed_out = True
        except RuntimeError as e:
            pkg_guard_error = str(e)

        if logger is not None:
            logger.event("package_started", package_id=pkg.package_id, i=pkg_i)

        # Defaults for row
        err: str | None = None
        timed_out = False
        created_types: set[str] = set()
        sim_mode: str | None = None
        fell_back = False
        ptb_parse_ok = True
        tx_build_ok = False
        dry_run_ok = False
        dry_run_exec_ok: bool | None = None
        dry_run_status: str | None = None
        dry_run_effects_error: str | None = None
        dry_run_abort_code: int | None = None
        dry_run_abort_location: str | None = None
        dev_inspect_ok = False
        dry_run_error: str | None = None
        plan_attempts = 0
        sim_attempts = 0
        gas_budget_used: int | None = None
        plan_variant: str | None = None
        schema_violation_count = 0
        schema_violation_attempts_until_first_valid: int | None = None
        semantic_failure_count = 0
        semantic_failure_attempts_until_first_success: int | None = None
        formatting_corrections: list[str] = []
        formatting_corrections_histogram: dict[str, int] = {}
        causality_valid: bool | None = None
        causality_score: float | None = None
        causality_errors: list[str] = []

        try:
            if pkg_guard_error is not None:
                raise TimeoutError(pkg_guard_error) if pkg_guard_timed_out else RuntimeError(pkg_guard_error)

            iface = _run_rust_emit_bytecode_json(Path(pkg.package_dir), rust_bin)
            truth_key_types = _extract_key_types_from_interface_json(iface)

            inventory = {}
            needs_inventory = agent_name in {"baseline-search", "real-openai-compatible", "template-search"}
            if needs_inventory and sender and sender != "0x0":
                inventory = _fetch_inventory(rpc_url, sender)

            ladder = _parse_gas_budget_ladder(gas_budget_ladder)
            budgets = _gas_budgets_to_try(base=gas_budget, ladder=ladder)

            plans_to_try: list[dict[str, Any] | None]
            if agent_name in {"baseline-search", "template-search"}:
                analysis = analyze_package(iface)
                candidates = analysis.candidates_ok
                max_cand = baseline_max_candidates if agent_name == "baseline-search" else 5
                if max_cand > 0:
                    candidates = candidates[:max_cand]
                if not candidates:
                    plans_to_try = [{"calls": []}]
                else:
                    plans_to_try = [{"calls": c} for c in candidates]
            elif agent_name == "mock-empty":
                plans_to_try = [{"calls": []}]
            elif agent_name == "mock-planfile":
                ptb = plan_by_id.get(pkg.package_id)
                if ptb is None:
                    raise RuntimeError(f"No PTB plan found in plan file for package: {pkg.package_id}")
                plans_to_try = [ptb]
            else:
                plans_to_try = [None] * max(1, int(max_plan_attempts))

            last_failure_ctx: dict[str, object] | None = None
            best_score: InhabitationScore | None = None

            for plan_i, plan_item in enumerate(plans_to_try):
                _check_run_guards(parent_pid=parent_pid, run_deadline=run_deadline)
                plan_attempts = plan_i + 1
                remaining = max(0.0, deadline - time.monotonic())
                if remaining <= 0:
                    raise TimeoutError(f"per-package timeout exceeded ({per_package_timeout_seconds}s)")

                # Build a PTB spec base
                if agent_name == "template-search":
                    assert real_agent is not None
                    prompt = _build_template_agent_prompt(
                        package_id=pkg.package_id,
                        target_key_types=truth_key_types,
                        calls=plan_item["calls"] if isinstance(plan_item, dict) else [],
                    )
                    try:
                        ai_calls = real_agent.complete_json(prompt, timeout_s=max(1.0, remaining))
                    except Exception as e:
                        last_failure_ctx = {"harness_error": str(e)}
                        # Let the next plan attempt retry; if we run out of attempts,
                        # the package will be recorded as failed below.
                        continue
                    if isinstance(ai_calls, list):
                        ptb_spec_base = {"calls": ai_calls}
                    elif isinstance(ai_calls, dict):
                        ptb_spec_base = ai_calls
                    else:
                        ptb_spec_base = plan_item if isinstance(plan_item, dict) else {"calls": []}
                elif isinstance(plan_item, dict):
                    ptb_spec_base = copy.deepcopy(plan_item)
                elif agent_name == "real-openai-compatible":
                    assert real_agent is not None
                    iface_summary = summarize_interface(iface, max_functions=60, mode="entry_only")
                    prompt = (
                        _build_real_agent_retry_prompt(
                            package_id=pkg.package_id,
                            target_key_types=truth_key_types,
                            last_failure=last_failure_ctx,
                        )
                        if (plan_attempts > 1 and last_failure_ctx is not None)
                        else _build_real_agent_prompt(
                            package_id=pkg.package_id,
                            target_key_types=truth_key_types,
                            interface_summary=iface_summary,
                            inventory_summary=_summarize_inventory(inventory),
                            max_planning_calls=int(max_planning_calls),
                        )
                    )
                    try:
                        ptb_spec_base = real_agent.complete_json(
                            prompt,
                            timeout_s=max(1.0, remaining),
                            logger=logger,
                            log_context={
                                "package_id": pkg.package_id,
                                "plan_attempt": plan_attempts,
                                "planning_call": 1,
                            },
                        )
                        if not isinstance(ptb_spec_base, dict) or "calls" not in ptb_spec_base:
                            raise ValueError("missing field calls")
                    except Exception as e:
                        last_failure_ctx = {"harness_error": str(e)}
                        # Let the next plan attempt retry; if we run out of attempts,
                        # the package will be recorded as failed below.
                        continue
                else:
                    ptb_spec_base = {"calls": []}

                variants = _ptb_variants(
                    ptb_spec_base,
                    sender=sender,
                    max_variants=max(1, int(max_heuristic_variants)),
                )
                if not variants:
                    variants = [("base", ptb_spec_base)]

                for variant_name, ptb_spec in variants:
                    plan_variant = variant_name

                    causality_result = validate_ptb_causality_detailed(ptb_spec)
                    if causality_valid is None:
                        causality_valid = causality_result.valid
                        causality_score = causality_result.causality_score
                        causality_errors = causality_result.errors[:5]

                    norm_result = normalize_ptb_spec(ptb_spec)
                    ptb_spec = norm_result.spec
                    if norm_result.had_corrections:
                        formatting_corrections.extend(norm_result.corrections)
                        for k, v in norm_result.correction_counts.items():
                            formatting_corrections_histogram[k] = formatting_corrections_histogram.get(k, 0) + v

                    for sim_i, budget in enumerate(budgets, start=1):
                        _check_run_guards(parent_pid=parent_pid, run_deadline=run_deadline)
                        sim_attempts += 1
                        gas_budget_used = budget
                        remaining = max(0.0, deadline - time.monotonic())
                        if remaining <= 0:
                            raise TimeoutError(f"per-package timeout exceeded ({per_package_timeout_seconds}s)")

                        if simulation_mode == "dry-run":
                            (
                                tx_out,
                                attempt_created_types,
                                attempt_static_types,
                                sim_mode,
                                fell_back,
                                _rpc_ok,
                                dry_run_error,
                            ) = _run_tx_sim_with_fallback(
                                sim_bin=dev_inspect_bin,
                                rpc_url=rpc_url,
                                sender=sender,
                                gas_budget=budget,
                                gas_coin=gas_coin,
                                bytecode_package_dir=Path(pkg.package_dir),
                                ptb_spec=ptb_spec,
                                timeout_s=max(1.0, remaining),
                                require_dry_run=require_dry_run,
                            )
                            tx_build_ok = True
                            dev_inspect_ok = bool(fell_back)
                            if isinstance(tx_out, dict):
                                exec_ok, failure = classify_dry_run_response(tx_out)
                                dry_run_exec_ok = exec_ok
                                dry_run_ok = exec_ok
                                if isinstance(failure, DryRunFailure):
                                    dry_run_status = failure.status
                                    dry_run_effects_error = failure.error
                                    dry_run_abort_code = failure.abort_code
                                    dry_run_abort_location = failure.abort_location
                            if fell_back:
                                attempt_created_types = attempt_created_types | attempt_static_types
                        else:
                            _tx_out, attempt_created_types, attempt_static_types, sim_mode = _run_tx_sim_via_helper(
                                dev_inspect_bin=dev_inspect_bin,
                                rpc_url=rpc_url,
                                sender=sender,
                                mode=simulation_mode,
                                gas_budget=budget,
                                gas_coin=gas_coin,
                                bytecode_package_dir=Path(pkg.package_dir),
                                ptb_spec=ptb_spec,
                                timeout_s=max(1.0, remaining),
                            )
                            tx_build_ok = True
                            dev_inspect_ok = simulation_mode == "dev-inspect"

                        attempt_created_norm = {normalize_type_string(t) for t in attempt_created_types}
                        attempt_score = score_inhabitation(
                            target_key_types=(truth_key_types if "truth_key_types" in locals() else set()),
                            created_object_types=attempt_created_norm,
                        )

                        if best_score is None or attempt_score.created_hits > best_score.created_hits:
                            best_score = attempt_score
                            created_types = attempt_created_norm

                        if attempt_score.targets > 0 and attempt_score.created_hits == attempt_score.targets:
                            break
                if best_score is not None and best_score.targets > 0 and best_score.created_hits == best_score.targets:
                    break

            # Agent policy: stop replanning if success.
            if agent_name != "baseline-search" and (simulation_mode != "dry-run" or dry_run_ok):
                pass

        except TimeoutError as e:
            timed_out = True
            err = str(e)
        except subprocess.TimeoutExpired:
            timed_out = True
            err = f"tx-sim timeout exceeded ({per_package_timeout_seconds}s)"
        except Exception as e:
            err = str(e)
            ptb_parse_ok = False

        # Treat guard-triggered exits as errors for aggregate reporting.
        if err is not None:
            error_count += 1

        score = score_inhabitation(
            target_key_types=(truth_key_types if "truth_key_types" in locals() else set()),
            created_object_types=created_types,
        )
        elapsed_s = time.monotonic() - pkg_started

        results.append(
            InhabitPackageResult(
                package_id=pkg.package_id,
                score=score,
                error=err,
                elapsed_seconds=elapsed_s,
                timed_out=timed_out,
                created_object_types_list=sorted(created_types) if include_created_types else None,
                simulation_mode=sim_mode,
                fell_back_to_dev_inspect=fell_back,
                ptb_parse_ok=ptb_parse_ok,
                tx_build_ok=tx_build_ok,
                dry_run_ok=dry_run_ok,
                dry_run_exec_ok=dry_run_exec_ok,
                dry_run_status=dry_run_status,
                dry_run_effects_error=dry_run_effects_error,
                dry_run_abort_code=dry_run_abort_code,
                dry_run_abort_location=dry_run_abort_location,
                dev_inspect_ok=dev_inspect_ok,
                dry_run_error=dry_run_error,
                plan_attempts=plan_attempts,
                sim_attempts=sim_attempts,
                gas_budget_used=gas_budget_used,
                plan_variant=plan_variant,
                schema_violation_count=schema_violation_count,
                schema_violation_attempts_until_first_valid=schema_violation_attempts_until_first_valid,
                semantic_failure_count=semantic_failure_count,
                semantic_failure_attempts_until_first_success=semantic_failure_attempts_until_first_success,
                formatting_corrections=formatting_corrections if formatting_corrections else None,
                formatting_corrections_histogram=(
                    formatting_corrections_histogram if formatting_corrections_histogram else None
                ),
                causality_valid=causality_valid,
                causality_score=causality_score,
                causality_errors=causality_errors if causality_errors else None,
            )
        )

        if logger is not None:
            logger.event(
                "package_finished",
                package_id=pkg.package_id,
                i=pkg_i,
                elapsed_seconds=elapsed_s,
                error=err,
                timed_out=timed_out,
                dry_run_ok=dry_run_ok,
                created_hits=score.created_hits,
                targets=score.targets,
                plan_variant=plan_variant,
            )

        if out_path is not None and checkpoint_every > 0 and pkg_i % checkpoint_every == 0:
            # Calculate intermediate metrics
            current_hit_rates = [(r.score.created_hits / r.score.targets) if r.score.targets else 0.0 for r in results]
            current_avg_hit_rate = (sum(current_hit_rates) / len(current_hit_rates)) if current_hit_rates else 0.0
            current_max_hit_rate = max(current_hit_rates) if current_hit_rates else 0.0

            k = Phase2ResultKeys
            _write_checkpoint(
                out_path,
                InhabitRunResult(
                    schema_version=2,
                    started_at_unix_seconds=started,
                    finished_at_unix_seconds=int(time.time()),
                    corpus_root_name=corpus_root.name,
                    samples=len(results),
                    seed=seed,
                    agent=agent_name,
                    rpc_url=rpc_url,
                    sender=sender,
                    gas_budget=gas_budget,
                    gas_coin=gas_coin,
                    aggregate={
                        k.AVG_HIT_RATE: current_avg_hit_rate,
                        k.MAX_HIT_RATE: current_max_hit_rate,
                        k.ERRORS: error_count,
                    },
                    packages=[_to_package_dict(r) for r in results],
                ),
            )
            if logger is not None:
                logger.event("checkpoint_written", path=str(out_path), packages_count=len(results))

        if pkg_guard_error is not None:
            break

    # Finalize + always write a checkpoint if requested
    finished = int(time.time())
    hit_rates = [(r.score.created_hits / r.score.targets) if r.score.targets else 0.0 for r in results]
    avg_hit_rate = (sum(hit_rates) / len(hit_rates)) if hit_rates else 0.0
    max_hit_rate = max(hit_rates) if hit_rates else 0.0
    k = Phase2ResultKeys
    run_result = InhabitRunResult(
        schema_version=2,
        started_at_unix_seconds=started,
        finished_at_unix_seconds=finished,
        corpus_root_name=corpus_root.name,
        samples=len(results),
        seed=seed,
        agent=agent_name,
        rpc_url=rpc_url,
        sender=sender,
        gas_budget=gas_budget,
        gas_coin=gas_coin,
        aggregate={
            k.AVG_HIT_RATE: avg_hit_rate,
            k.MAX_HIT_RATE: max_hit_rate,
            k.ERRORS: error_count,
        },
        packages=[_to_package_dict(r) for r in results],
    )
    if out_path is not None:
        _write_checkpoint(out_path, run_result)

    if logger is not None:
        logger.event(
            "run_finished",
            finished_at_unix_seconds=finished,
            samples=len(results),
            errors=error_count,
            avg_hit_rate=avg_hit_rate,
        )

    console.print(
        "phase2 avg_hit_rate="
        f"{run_result.aggregate.get('avg_hit_rate'):.3f} max_hit_rate={run_result.aggregate.get('max_hit_rate'):.3f} "
        f"errors={run_result.aggregate.get('errors')}"
    )
    return run_result


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="Phase II: PTB inhabitation (dry-run/dev-inspect) benchmark")
    p.add_argument("--corpus-root", type=Path, required=True)
    p.add_argument("--samples", type=int, default=25)
    p.add_argument("--seed", type=int, default=0)
    p.add_argument(
        "--package-ids-file",
        type=Path,
        help="Optional file of package ids to restrict to (1 per line; '#' comments allowed).",
    )
    p.add_argument(
        "--dataset",
        type=str,
        help="Named dataset under manifests/datasets/<name>.txt (ignored if --package-ids-file is set).",
    )
    p.add_argument(
        "--subset",
        type=str,
        help="Deprecated alias for --dataset.",
    )
    p.add_argument(
        "--prefer-signal-package-ids",
        action="store_true",
        help="If set and --package-ids-file is not provided, use results/signal_ids_hit_ge_1.txt when it exists.",
    )
    p.add_argument(
        "--agent",
        type=str,
        default="mock-empty",
        choices=[
            "mock-empty",
            "mock-planfile",
            "real-openai-compatible",
            "baseline-search",
            "template-search",
        ],
    )
    p.add_argument("--plan-file", type=Path, help="JSON mapping package_id -> PTB spec (required for mock-planfile).")
    p.add_argument("--rust-bin", type=Path, default=default_rust_binary())
    p.add_argument("--dev-inspect-bin", type=Path, default=_default_dev_inspect_binary())
    p.add_argument("--rpc-url", type=str, default="https://fullnode.mainnet.sui.io:443")
    p.add_argument(
        "--sender",
        type=str,
        default=None,
        help="Sender address for tx simulation (defaults to SMI_SENDER from --env-file, otherwise 0x0).",
    )
    p.add_argument(
        "--gas-budget",
        type=int,
        default=10_000_000,
        help="Gas budget used for dry-run transaction simulation.",
    )
    p.add_argument(
        "--gas-coin",
        type=str,
        help="Optional gas coin object id to use for dry-run (defaults to first Coin<SUI> for sender).",
    )
    p.add_argument(
        "--gas-budget-ladder",
        type=str,
        default="20000000,50000000",
        help="Comma-separated gas budgets to retry on InsufficientGas (in addition to --gas-budget).",
    )
    p.add_argument(
        "--max-plan-attempts",
        type=int,
        default=5,
        help="Max PTB replanning attempts per package (real agent only).",
    )
    p.add_argument(
        "--max-planning-calls",
        type=int,
        default=50,
        help="Maximum LLM planning calls per package for progressive exposure (2-3 recommended).",
    )
    p.add_argument(
        "--baseline-max-candidates",
        type=int,
        default=25,
        help="Max candidates to try per package in baseline-search mode.",
    )
    p.add_argument(
        "--max-heuristic-variants",
        type=int,
        default=4,
        help="Max deterministic PTB plan variants to try per plan attempt (includes the base plan).",
    )
    p.add_argument("--out", type=Path)
    p.add_argument(
        "--resume",
        action="store_true",
        help="If --out exists, resume from it (skip completed package_ids and append).",
    )
    p.add_argument(
        "--per-package-timeout-seconds",
        type=float,
        default=120.0,
        help="Maximum wall-clock time to spend per package.",
    )
    p.add_argument(
        "--include-created-types",
        action="store_true",
        help="Include full created object type lists per package in the output JSON (larger files).",
    )
    p.add_argument(
        "--require-dry-run",
        action="store_true",
        help="Fail the package if dry-run cannot be executed (no dev-inspect fallback).",
    )
    p.add_argument(
        "--simulation-mode",
        type=str,
        default="dry-run",
        choices=["dry-run", "dev-inspect", "build-only"],
        help="Tx simulation mode (default: dry-run).",
    )
    p.add_argument(
        "--continue-on-error",
        action="store_true",
        help="Continue benchmark even if a package fails (records error and scores it as zero created types).",
    )
    p.add_argument(
        "--max-errors",
        type=int,
        default=25,
        help="Stop the run if more than this many package errors occur (with --continue-on-error).",
    )
    p.add_argument(
        "--checkpoint-every",
        type=int,
        default=10,
        help="Save partial results to --out every N packages (default: 10).",
    )
    p.add_argument(
        "--env-file",
        type=Path,
        default=Path(".env"),
        help="Path to a dotenv file (default: .env in the current working directory).",
    )
    p.add_argument(
        "--parent-pid",
        type=int,
        default=None,
        help="If set, exit the run when this PID no longer exists (prevents orphaned spend).",
    )
    p.add_argument(
        "--max-run-seconds",
        type=float,
        default=None,
        help="If set, exit the run after this many wall-clock seconds.",
    )
    p.add_argument(
        "--log-dir",
        type=Path,
        default=Path("logs"),
        help="Directory to write JSONL logs under (default: benchmark/logs). Use --no-log to disable.",
    )
    p.add_argument("--run-id", type=str, help="Optional run id for log directory naming.")
    p.add_argument("--no-log", action="store_true", help="Disable JSONL logging.")
    args = p.parse_args(argv)

    if args.dataset and args.subset:
        raise SystemExit("Use only one of --dataset or --subset")

    env_file = args.env_file if args.env_file.exists() else None
    if env_file is None:
        fallback = Path("benchmark/.env")
        if fallback.exists():
            env_file = fallback

    plan_file = args.plan_file
    if args.agent == "mock-planfile" and plan_file is None:
        raise SystemExit("--agent mock-planfile requires --plan-file")
    if args.require_dry_run and args.simulation_mode != "dry-run":
        raise SystemExit("--require-dry-run requires --simulation-mode dry-run")

    run(
        corpus_root=args.corpus_root,
        samples=args.samples,
        seed=args.seed,
        package_ids_file=args.package_ids_file,
        agent_name=args.agent,
        rust_bin=args.rust_bin,
        dev_inspect_bin=args.dev_inspect_bin,
        rpc_url=args.rpc_url,
        sender=args.sender,
        gas_budget=args.gas_budget,
        gas_coin=args.gas_coin,
        gas_budget_ladder=args.gas_budget_ladder,
        max_planning_calls=args.max_planning_calls,
        max_plan_attempts=args.max_plan_attempts,
        baseline_max_candidates=args.baseline_max_candidates,
        max_heuristic_variants=args.max_heuristic_variants,
        plan_file=plan_file,
        env_file=env_file,
        out_path=args.out,
        resume=args.resume,
        continue_on_error=args.continue_on_error,
        max_errors=args.max_errors,
        checkpoint_every=args.checkpoint_every,
        per_package_timeout_seconds=args.per_package_timeout_seconds,
        include_created_types=args.include_created_types,
        require_dry_run=args.require_dry_run,
        simulation_mode=args.simulation_mode,
        log_dir=args.log_dir,
        run_id=args.run_id,
        parent_pid=args.parent_pid,
        max_run_seconds=args.max_run_seconds,
    )


if __name__ == "__main__":
    main()
