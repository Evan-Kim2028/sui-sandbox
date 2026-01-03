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
import logging
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
from smi_bench.checkpoint import load_checkpoint, write_checkpoint
from smi_bench.constants import DEFAULT_RPC_URL
from smi_bench.dataset import collect_packages, sample_packages
from smi_bench.env import load_dotenv
from smi_bench.inhabit.dryrun import DryRunFailure, classify_dry_run_response
from smi_bench.inhabit.engine import (
    check_run_guards,
    fetch_inventory,
    ptb_variants,
    resolve_placeholders,
    run_tx_sim_via_helper,
)
from smi_bench.inhabit.executable_subset import (
    analyze_package,
    summarize_interface,
)
from smi_bench.inhabit.normalize import normalize_ptb_spec
from smi_bench.inhabit.score import InhabitationScore, normalize_type_string, score_inhabitation
from smi_bench.inhabit.validator import validate_ptb_causality_detailed
from smi_bench.logging import JsonlLogger, default_run_id
from smi_bench.rust import default_rust_binary, emit_bytecode_json, validate_rust_binary
from smi_bench.schema import Phase2ResultKeys, validate_phase2_run_json
from smi_bench.utils import (
    extract_key_types_from_interface_json,
    validate_binary,
)

console = Console()
logger = logging.getLogger(__name__)


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


def _run_preflight_checks(
    *,
    rpc_url: str,
    sender: str,
    simulation_mode: str,
) -> None:
    """
    Fail fast if the environment is not ready for the requested simulation mode.
    """
    console.print(f"[bold]Pre-flight:[/bold] Checking environment for mode={simulation_mode}...")

    # 1. RPC Check
    try:
        # Simple health check payload
        payload = {"jsonrpc": "2.0", "id": 1, "method": "rpc.discover", "params": []}
        r = httpx.post(rpc_url, json=payload, timeout=5.0)
        if r.status_code != 200:
            raise RuntimeError(f"RPC {rpc_url} returned status {r.status_code}")
    except Exception as e:
        raise RuntimeError(f"RPC connection failed: {e}") from e

    # 2. Sender Check (Critical for dry-run)
    if simulation_mode == "dry-run":
        if sender == "0x0" or not sender:
            raise RuntimeError(
                "FAIL FAST: simulation_mode='dry-run' requires a valid sender address.\n"
                "  Current sender: 0x0\n"
                "  Fix: Set --sender (or SMI_SENDER env var) to a funded address."
            )

        # Check balance
        try:
            payload = {"jsonrpc": "2.0", "id": 1, "method": "suix_getCoins", "params": [sender, "0x2::sui::SUI"]}
            r = httpx.post(rpc_url, json=payload, timeout=10.0)
            data = r.json()
            coins = data.get("result", {}).get("data", [])
            if not coins:
                raise RuntimeError(f"FAIL FAST: Sender {sender} has no SUI coins on {rpc_url}.")
            console.print(f"[green]OK:[/green] Sender {sender} is funded (found {len(coins)} coins).")
        except Exception as e:
            raise RuntimeError(f"Could not verify sender balance: {e}") from e


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
    Attempt dry-run first to get transaction-ground-truth created types.
    If dry-run fails and require_dry_run is false, fall back to dev-inspect (static types only).
    """
    try:
        tx_out, created, static_created, mode_used = run_tx_sim_via_helper(
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
        return tx_out, created, static_created, mode_used, False, True, None
    except Exception as e:
        if require_dry_run:
            raise
        dry_run_err = str(e)

    # Dev-inspect fallback
    _tx_out, created, static_created, mode_used = run_tx_sim_via_helper(
        dev_inspect_bin=sim_bin,
        rpc_url=rpc_url,
        sender=sender,
        mode="dev-inspect",
        gas_budget=gas_budget,
        gas_coin=gas_coin,
        bytecode_package_dir=bytecode_package_dir,
        ptb_spec=ptb_spec,
        timeout_s=timeout_s,
    )
    return None, created, static_created, mode_used, True, False, dry_run_err


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
        "- Use '0xADDR::module' to request all functions in a module.\n"
        "- The 'init' function is private and cannot be called; do not request it.\n"
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

    # Run preflight checks before starting any heavy work
    _run_preflight_checks(rpc_url=rpc_url, sender=sender, simulation_mode=simulation_mode)

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
    total_prompt_tokens = 0
    total_completion_tokens = 0
    last_partial: InhabitRunResult | None = None
    if resume:
        if out_path is None:
            raise SystemExit("--resume requires --out")
        if out_path.exists():
            cp = InhabitRunResult(**load_checkpoint(out_path))
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
            check_run_guards(parent_pid=parent_pid, run_deadline=run_deadline)
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
            iface = emit_bytecode_json(package_dir=Path(pkg.package_dir), rust_bin=rust_bin)
            truth_key_types = extract_key_types_from_interface_json(iface)

            if pkg_guard_error is not None:
                raise TimeoutError(pkg_guard_error) if pkg_guard_timed_out else RuntimeError(pkg_guard_error)

            inventory = {}
            needs_inventory = agent_name in {"baseline-search", "real-openai-compatible", "template-search"}
            if needs_inventory and sender and sender != "0x0":
                inventory = fetch_inventory(rpc_url=rpc_url, sender=sender)

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
                check_run_guards(parent_pid=parent_pid, run_deadline=run_deadline)
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

                    # Progressive exposure loop
                    planning_calls_remaining = int(max_planning_calls)
                    current_requested_targets: set[str] = set()
                    already_requested_targets: set[str] = set()
                    ptb_spec_base = None

                    for planning_call_i in range(1, planning_calls_remaining + 1):
                        check_run_guards(parent_pid=parent_pid, run_deadline=run_deadline)
                        remaining = max(0.0, deadline - time.monotonic())
                        if remaining <= 0:
                            raise TimeoutError(f"per-package timeout exceeded ({per_package_timeout_seconds}s)")

                        if current_requested_targets:
                            iface_summary = summarize_interface(
                                iface, max_functions=60, mode="focused", requested_targets=current_requested_targets
                            )
                        else:
                            iface_summary = summarize_interface(iface, max_functions=60, mode="entry_then_public")

                        prompt = (
                            _build_real_agent_retry_prompt(
                                package_id=pkg.package_id,
                                target_key_types=truth_key_types,
                                last_failure=last_failure_ctx,
                                interface_summary=iface_summary if last_failure_ctx is not None else None,
                                max_planning_calls=planning_calls_remaining - planning_call_i + 1,
                            )
                            if (plan_attempts > 1 and last_failure_ctx is not None)
                            else _build_real_agent_prompt(
                                package_id=pkg.package_id,
                                target_key_types=truth_key_types,
                                interface_summary=iface_summary,
                                inventory_summary=_summarize_inventory(inventory),
                                max_planning_calls=planning_calls_remaining - planning_call_i + 1,
                            )
                        )

                        try:
                            ai_response = real_agent.complete_json(
                                prompt,
                                timeout_s=max(1.0, remaining),
                                logger=logger,
                                log_context={
                                    "package_id": pkg.package_id,
                                    "plan_attempt": plan_attempts,
                                    "planning_call": planning_call_i,
                                },
                            )

                            if isinstance(ai_response, dict) and "need_more" in ai_response:
                                # Update targets for next focused summary
                                new_targets = ai_response["need_more"]
                                if isinstance(new_targets, list):
                                    targets_to_add = [
                                        str(t) for t in new_targets if str(t) not in already_requested_targets
                                    ]
                                    if not targets_to_add:
                                        # Model is asking for same things again - force it to plan
                                        # Terminate progressive loop and re-invoke with "force plan" instruction
                                        iface_summary = summarize_interface(
                                            iface,
                                            max_functions=60,
                                            mode="focused",
                                            requested_targets=current_requested_targets,
                                        )
                                        prompt = _build_real_agent_retry_prompt(
                                            package_id=pkg.package_id,
                                            target_key_types=truth_key_types,
                                            last_failure={
                                                "harness_error": (
                                                    "You already requested those details. "
                                                    "No other public functions found. "
                                                    "Please provide a best-effort PTB plan now "
                                                    "using only public functions."
                                                )
                                            },
                                            interface_summary=iface_summary,
                                        )
                                        ai_response = real_agent.complete_json(
                                            prompt,
                                            timeout_s=max(1.0, remaining),
                                            logger=logger,
                                            log_context={
                                                "package_id": pkg.package_id,
                                                "plan_attempt": plan_attempts,
                                                "planning_call": planning_call_i + 1,
                                            },
                                        )
                                        if isinstance(ai_response, dict) and "calls" in ai_response:
                                            ptb_spec_base = ai_response
                                        break

                                    current_requested_targets.update(targets_to_add)
                                    already_requested_targets.update(targets_to_add)
                                continue

                            if isinstance(ai_response, dict) and "calls" in ai_response:
                                ptb_spec_base = ai_response
                                break

                            keys_summary = (
                                list(ai_response.keys()) if isinstance(ai_response, dict) else type(ai_response)
                            )
                            raise ValueError(f"unexpected response keys: {keys_summary}")

                        except Exception as e:
                            last_failure_ctx = {"harness_error": str(e)}
                            break

                    if ptb_spec_base is None:
                        # Failed to get a plan within the planning call budget or hit an error
                        if last_failure_ctx is None:
                            last_failure_ctx = {"harness_error": "reached max planning calls without plan"}
                        continue
                else:
                    ptb_spec_base = {"calls": []}

                # Resolve any placeholders ($smi_placeholder) against inventory
                if agent_name in {"baseline-search", "template-search"}:
                    _ = resolve_placeholders(ptb_spec_base, inventory=inventory)

                variants = ptb_variants(
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
                        check_run_guards(parent_pid=parent_pid, run_deadline=run_deadline)
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
                            _tx_out, attempt_created_types, attempt_static_types, sim_mode = run_tx_sim_via_helper(
                                dev_inspect_bin=dev_inspect_bin,
                                rpc_url=rpc_url,
                                sender=sender,
                                ptb_spec=ptb_spec,
                                simulation_mode=simulation_mode,
                                call_timeout_seconds=max(1.0, remaining),
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
        except (RuntimeError, ValueError, httpx.RequestError) as e:
            err = str(e)
            ptb_parse_ok = False

        # Treat any per-package failure as an error for aggregate reporting.
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
            write_checkpoint(
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
                        k.TOTAL_PROMPT_TOKENS: total_prompt_tokens,
                        k.TOTAL_COMPLETION_TOKENS: total_completion_tokens,
                    },
                    packages=[_to_package_dict(r) for r in results],
                ),
                validate_fn=validate_phase2_run_json,
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
            k.TOTAL_PROMPT_TOKENS: total_prompt_tokens,
            k.TOTAL_COMPLETION_TOKENS: total_completion_tokens,
        },
        packages=[_to_package_dict(r) for r in results],
    )
    if out_path is not None:
        write_checkpoint(out_path, run_result, validate_fn=validate_phase2_run_json)

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
    p.add_argument("--rpc-url", type=str, default=DEFAULT_RPC_URL)
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

    # Resolve --dataset to package_ids_file path
    package_ids_file = args.package_ids_file
    if args.dataset:
        if args.package_ids_file:
            raise SystemExit("Use only one of --dataset or --package-ids-file")
        dataset_path = Path("manifests/datasets") / f"{args.dataset}.txt"
        if not dataset_path.exists():
            raise SystemExit(f"Dataset not found: {dataset_path}")
        package_ids_file = dataset_path

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
        package_ids_file=package_ids_file,
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
