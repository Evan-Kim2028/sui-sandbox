"""
Phase II execution engine.

Handles transaction simulation, inventory management, and plan adaptation.
Extracted from inhabit_runner.py to minimize technical debt.
"""

from __future__ import annotations

import copy
import json
import os
import time
from pathlib import Path
from typing import Any

import httpx

from smi_bench.inhabit.score import normalize_type_string
from smi_bench.utils import get_smi_temp_dir, run_json_helper


def pid_is_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    else:
        return True


def check_run_guards(*, parent_pid: int | None, run_deadline: float | None) -> None:
    if parent_pid is not None and parent_pid > 0 and not pid_is_alive(parent_pid):
        raise RuntimeError(f"Parent process exited (pid={parent_pid})")
    if run_deadline is not None and time.monotonic() >= run_deadline:
        raise TimeoutError(f"Maximum run time exceeded ({run_deadline:.1f}s)")


def fetch_inventory(rpc_url: str, sender: str) -> dict[str, list[str]]:
    if sender == "0x0" or not sender.startswith("0x"):
        return {}

    try:
        objects = []
        cursor = None
        while True:
            payload = {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "suix_getOwnedObjects",
                "params": [sender, {"filter": None, "options": {"showType": True}}, cursor, 50],
            }
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
    except Exception:
        return {}


def resolve_placeholders(ptb_spec: dict[str, Any], inventory: dict[str, list[str]]) -> bool:
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
                ph_norm = normalize_type_string(ph)
                candidates = inventory.get(ph_norm)
                if candidates:
                    args[i] = {"imm_or_owned_object": candidates[0]}
                else:
                    args[i] = {"imm_or_owned_object": "0x0"}
                    resolved_all = False
    return resolved_all


def ptb_variants(base_spec: dict[str, Any], *, sender: str, max_variants: int) -> list[tuple[str, dict[str, Any]]]:
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


def _rewrite_ptb_addresses_in_place(ptb_spec: dict[str, Any], *, sender: str) -> bool:
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
    _INT_ARG_KEYS = ("u8", "u16", "u32", "u64", "u128", "u256")
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


def run_tx_sim_via_helper(
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
    tmp_dir = get_smi_temp_dir()
    tmp_path = tmp_dir / f"ptb_spec_{int(time.time() * 1000)}.json"
    try:
        tmp_path.write_text(json.dumps(ptb_spec, indent=2, sort_keys=True) + "\n")
    except Exception as e:
        raise RuntimeError(f"Failed to write temp PTB spec: {e}") from e

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

        data = run_json_helper(cmd, timeout_s=timeout_s, context=f"tx sim ({mode})")

        mode_used = data.get("modeUsed", "unknown")
        created_types = data.get("createdObjectTypes", [])
        static_types = data.get("staticCreatedObjectTypes", [])
        dry_run = data.get("dryRun")
        dev_inspect = data.get("devInspect")

        created_set = {t for t in created_types if isinstance(t, str) and t}
        static_set = {t for t in static_types if isinstance(t, str) and t}

        tx_out = dry_run if isinstance(dry_run, dict) else (dev_inspect if isinstance(dev_inspect, dict) else None)
        return tx_out, created_set, static_set, mode_used
    finally:
        if tmp_path.exists():
            try:
                tmp_path.unlink()
            except Exception:
                pass
