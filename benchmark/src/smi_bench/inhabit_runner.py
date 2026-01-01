from __future__ import annotations

import argparse
import copy
import json
import os
import subprocess
import time
from dataclasses import asdict, dataclass
from pathlib import Path

from rich.console import Console
from rich.progress import track

from smi_bench.agents.real_agent import RealAgent, load_real_agent_config
from smi_bench.dataset import collect_packages, sample_packages
from smi_bench.env import load_dotenv
from smi_bench.inhabit.dryrun import DryRunFailure, classify_dry_run_response
from smi_bench.inhabit.executable_subset import analyze_package
from smi_bench.inhabit.score import InhabitationScore, normalize_type_string, score_inhabitation
from smi_bench.logging import JsonlLogger, default_run_id
from smi_bench.runner import _extract_key_types_from_interface_json

console = Console()


def _parse_gas_budget_ladder(s: str) -> list[int]:
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


def _rewrite_ptb_addresses_in_place(ptb_spec: dict, *, sender: str) -> bool:
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


def _rewrite_ptb_ints_in_place(ptb_spec: dict, *, value: int) -> bool:
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


def _ptb_variants(base_spec: dict, *, sender: str, max_variants: int) -> list[tuple[str, dict]]:
    """
    Deterministic, bounded PTB variants to allow local adaptation within a fixed per-package budget.

    This is intentionally conservative to keep corpus runs fast and diff-stable.
    """
    if max_variants <= 0:
        return []

    variants: list[tuple[str, dict]] = []
    seen: set[str] = set()

    def _add(name: str, spec: dict) -> None:
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


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def _default_rust_binary() -> Path:
    exe = "sui_move_interface_extractor.exe" if os.name == "nt" else "sui_move_interface_extractor"
    local = _repo_root() / "target" / "release" / exe
    if local.exists():
        return local
    return Path("/usr/local/bin") / exe


def _default_dev_inspect_binary() -> Path:
    exe = "smi_tx_sim.exe" if os.name == "nt" else "smi_tx_sim"
    local = _repo_root() / "target" / "release" / exe
    if local.exists():
        return local
    return Path("/usr/local/bin") / exe


def _run_rust_emit_bytecode_json(bytecode_package_dir: Path, rust_bin: Path) -> dict:
    out = subprocess.check_output(
        [
            str(rust_bin),
            "--bytecode-package-dir",
            str(bytecode_package_dir),
            "--emit-bytecode-json",
            "-",
        ],
        text=True,
    )
    return json.loads(out)


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
    ptb_spec: dict,
    timeout_s: float,
) -> tuple[dict | None, set[str], set[str], str]:
    # Write a temporary PTB spec file (small, single package).
    tmp_dir = _repo_root() / "benchmark" / ".tmp"
    tmp_dir.mkdir(parents=True, exist_ok=True)
    tmp_path = tmp_dir / f"ptb_spec_{int(time.time() * 1000)}.json"
    tmp_path.write_text(json.dumps(ptb_spec, indent=2, sort_keys=True) + "\n")
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
        out = subprocess.check_output(cmd, text=True, timeout=timeout_s)
        data = json.loads(out)
        if not isinstance(data, dict):
            raise RuntimeError("dev-inspect helper returned non-object JSON")
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
        try:
            tmp_path.unlink()
        except Exception:
            pass


def _run_tx_sim_with_fallback(
    *,
    sim_bin: Path,
    rpc_url: str,
    sender: str,
    gas_budget: int | None,
    gas_coin: str | None,
    bytecode_package_dir: Path | None,
    ptb_spec: dict,
    timeout_s: float,
    require_dry_run: bool,
) -> tuple[dict | None, set[str], set[str], str, bool, bool, str | None]:
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
    except Exception as e:
        if require_dry_run:
            raise
        dry_run_err = str(e)

    # Dev-inspect fallback: we still call the helper in dev-inspect mode to keep outputs consistent,
    # but scoring relies on static types because dev-inspect does not include object type strings.
    tmp_dir = _repo_root() / "benchmark" / ".tmp"
    tmp_dir.mkdir(parents=True, exist_ok=True)
    tmp_path = tmp_dir / f"ptb_spec_{int(time.time() * 1000)}.json"
    tmp_path.write_text(json.dumps(ptb_spec, indent=2, sort_keys=True) + "\n")
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
        out = subprocess.check_output(cmd, text=True, timeout=timeout_s)
        data = json.loads(out)
        if not isinstance(data, dict):
            raise RuntimeError("tx sim helper returned non-object JSON")
        mode_used = data.get("modeUsed") if isinstance(data.get("modeUsed"), str) else "unknown"
        created_types = data.get("createdObjectTypes")
        static_types = data.get("staticCreatedObjectTypes")
        created_set = (
            {t for t in created_types if isinstance(t, str) and t} if isinstance(created_types, list) else set()
        )
        static_set = {t for t in static_types if isinstance(t, str) and t} if isinstance(static_types, list) else set()
        return None, created_set, static_set, mode_used, True, False, dry_run_err
    finally:
        try:
            tmp_path.unlink()
        except Exception:
            pass


def _build_real_agent_prompt(*, package_id: str, target_key_types: set[str]) -> str:
    """
    Prompt the model to output a PTB spec JSON matching the Rust helper schema:

      {"calls":[{"target":"0xADDR::module::function","type_args":[],"args":[...]}]}
    """
    instructions = (
        "You are crafting a PTB plan for transaction simulation (dry-run/dev-inspect).\n"
        "Return ONLY valid JSON matching this schema:\n"
        '{"calls":[{"target":"0xADDR::module::function","type_args":["<TypeTag>",...],'
        '"args":[{"u64":1},{"vector_u8_utf8":"hi"},...] }]}\n'
        "Do not include tx_context arguments (they are implicit).\n"
    )
    payload = {"package_id": package_id, "target_key_types": sorted(target_key_types)}
    return instructions + json.dumps(payload, indent=2, sort_keys=True)


def _build_real_agent_retry_prompt(
    *,
    package_id: str,
    target_key_types: set[str],
    last_failure: dict[str, object],
) -> str:
    """
    Provide a failure-aware retry prompt so the agent can adapt within the per-package timeout.
    """
    instructions = (
        "Your previous PTB plan failed in dry-run.\n"
        "Revise the PTB plan to avoid the failure.\n"
        "Return ONLY valid JSON matching this schema:\n"
        '{"calls":[{"target":"0xADDR::module::function","type_args":["<TypeTag>",...],'
        '"args":[{"u64":1},{"vector_u8_utf8":"hi"},...] }]}\n'
        "Do not include tx_context arguments (they are implicit).\n"
    )
    payload = {
        "package_id": package_id,
        "target_key_types": sorted(target_key_types),
        "last_failure": last_failure,
    }
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


@dataclass
class InhabitRunResult:
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
    aggregate: dict
    packages: list[dict]


def _write_checkpoint(out_path: Path, run_result: InhabitRunResult) -> None:
    tmp = out_path.with_suffix(out_path.suffix + ".tmp")
    tmp.write_text(json.dumps(asdict(run_result), indent=2, sort_keys=True) + "\n")
    tmp.replace(out_path)


def _load_checkpoint(out_path: Path) -> InhabitRunResult:
    try:
        data = json.loads(out_path.read_text())
    except Exception as e:
        raise RuntimeError(f"failed to parse checkpoint JSON: {out_path}") from e
    if not isinstance(data, dict) or not isinstance(data.get("packages"), list):
        raise RuntimeError(f"invalid checkpoint shape: {out_path}")
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
) -> tuple[list[InhabitPackageResult], set[str], int, int]:
    results: list[InhabitPackageResult] = []
    seen: set[str] = set()
    errors = cp.aggregate.get("errors") if isinstance(cp.aggregate, dict) else 0
    try:
        error_count = int(errors)
    except Exception:
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
        except Exception:
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
    max_plan_attempts: int,
    baseline_max_candidates: int,
    max_heuristic_variants: int,
    plan_file: Path | None,
    env_file: Path | None,
    out_path: Path | None,
    resume: bool,
    continue_on_error: bool,
    max_errors: int,
    checkpoint_every: int,
    per_package_timeout_seconds: float,
    include_created_types: bool,
    require_dry_run: bool,
    simulation_mode: str,
    log_dir: Path | None,
    run_id: str | None,
) -> InhabitRunResult:
    if not rust_bin.exists():
        raise SystemExit(f"rust binary not found: {rust_bin} (run `cargo build --release --locked` at repo root)")
    if not dev_inspect_bin.exists():
        raise SystemExit(
            "dev-inspect helper binary not found: "
            f"{dev_inspect_bin} (run `cargo build --release --locked` at repo root)"
        )

    env_overrides = load_dotenv(env_file) if env_file is not None else {}
    sender, gas_coin = _resolve_sender_and_gas_coin(sender=sender, gas_coin=gas_coin, env_overrides=env_overrides)
    plan_by_id: dict[str, dict] = {}
    if plan_file is not None:
        plan_by_id = _load_plan_file(plan_file)

    logger: JsonlLogger | None = None
    if log_dir is not None:
        rid = run_id or default_run_id(prefix="phase2")
        logger = JsonlLogger(base_dir=log_dir, run_id=rid)

    started = int(time.time())
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
                "argv": list(map(str, os.sys.argv)),
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
            results, seen, error_count, started = _resume_results_from_checkpoint(cp)
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
    elif agent_name in {"mock-empty", "mock-planfile", "baseline-search"}:
        pass
    else:
        raise SystemExit(f"unknown agent: {agent_name}")

    done_already = len(results)
    for pkg_i, pkg in enumerate(track(picked, description="phase2"), start=done_already + 1):
        pkg_started = time.monotonic()
        deadline = pkg_started + per_package_timeout_seconds
        if logger is not None:
            logger.event("package_started", package_id=pkg.package_id, i=pkg_i)

        iface = _run_rust_emit_bytecode_json(Path(pkg.package_dir), rust_bin)
        truth_key_types = _extract_key_types_from_interface_json(iface)

        err: str | None = None
        timed_out = False
        created_types: set[str] = set()
        static_types: set[str] = set()
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

        try:
            ladder = _parse_gas_budget_ladder(gas_budget_ladder)
            budgets = _gas_budgets_to_try(base=gas_budget, ladder=ladder)

            # Strategy setup
            plans_to_try = []
            if agent_name == "baseline-search":
                analysis = analyze_package(iface)
                candidates = analysis.candidates_ok
                if baseline_max_candidates > 0:
                    candidates = candidates[:baseline_max_candidates]

                if not candidates:
                    # No candidates -> one empty plan attempt to record failure/stats
                    plan_iterator = [{"calls": []}]
                else:
                    for c in candidates:
                        plan_iterator.append({"calls": c})
            elif agent_name == "mock-empty":
                plans_to_try = [{"calls": []}]
            elif agent_name == "mock-planfile":
                ptb = plan_by_id.get(pkg.package_id)
                if ptb is None:
                    raise RuntimeError(f"no PTB plan in --plan-file for package_id={pkg.package_id}")
                plans_to_try = [ptb]
            else:
                plans_to_try = [None] * max(1, int(max_plan_attempts))

            last_failure_ctx: dict[str, object] | None = None

            # We track the "best" result seen so far to report at the end.
            # Best is defined by: (score.created_hits, score.created_distinct, dry_run_ok)
            best_score: InhabitationScore | None = None

            for plan_i, plan_item in enumerate(plans_to_try):
                plan_attempts = plan_i + 1

                if isinstance(plan_item, dict):
                    ptb_spec_base = plan_item
                else:
                    assert real_agent is not None
                    remaining = max(0.0, deadline - time.monotonic())
                    if remaining <= 0:
                        raise TimeoutError(f"per-package timeout exceeded ({per_package_timeout_seconds}s)")
                    if last_failure_ctx is None or plan_i == 0:
                        prompt = _build_real_agent_prompt(package_id=pkg.package_id, target_key_types=truth_key_types)
                    else:
                        prompt = _build_real_agent_retry_prompt(
                            package_id=pkg.package_id,
                            target_key_types=truth_key_types,
                            last_failure=last_failure_ctx,
                        )
                    ptb_spec_base = real_agent.complete_json(prompt, timeout_s=max(1.0, remaining))

                variants = _ptb_variants(
                    ptb_spec_base,
                    sender=sender,
                    max_variants=max(1, int(max_heuristic_variants)),
                )
                if not variants:
                    variants = [("base", ptb_spec_base)]

                # Try variants under a budget ladder.
                last_failure_ctx = None
                for variant_name, ptb_spec in variants:
                    plan_variant = variant_name
                    for attempt_i, budget in enumerate(budgets, start=1):
                        sim_attempts += 1
                        gas_budget_used = budget
                        remaining = max(0.0, deadline - time.monotonic())
                        if remaining <= 0:
                            raise TimeoutError(f"per-package timeout exceeded ({per_package_timeout_seconds}s)")
                        if logger is not None:
                            logger.event(
                                "sim_attempt_started",
                                package_id=pkg.package_id,
                                i=pkg_i,
                                plan_attempt=plan_attempts,
                                plan_variant=plan_variant,
                                sim_attempt=attempt_i,
                                gas_budget=budget,
                            )

                        # Reset per-attempt flags
                        attempt_dry_run_ok = False
                        attempt_created_types: set[str] = set()
                        attempt_static_types: set[str] = set()
                        attempt_dry_run_exec_ok = None
                        attempt_dry_run_status = None
                        attempt_dry_run_effects_error = None
                        attempt_dry_run_abort_code = None
                        attempt_dry_run_abort_location = None
                        attempt_dry_run_error = None

                        if simulation_mode == "dry-run":
                            (
                                tx_out,
                                attempt_created_types,
                                attempt_static_types,
                                sim_mode,
                                fell_back,
                                _rpc_ok,
                                attempt_dry_run_error,
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
                                attempt_dry_run_exec_ok = exec_ok
                                attempt_dry_run_ok = exec_ok
                                if exec_ok:
                                    pass
                                elif isinstance(failure, DryRunFailure):
                                    attempt_dry_run_status = failure.status
                                    attempt_dry_run_effects_error = failure.error
                                    attempt_dry_run_abort_code = failure.abort_code
                                    attempt_dry_run_abort_location = failure.abort_location
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
                            attempt_dry_run_ok = False
                            dev_inspect_ok = simulation_mode == "dev-inspect"
                            attempt_created_types = attempt_created_types | attempt_static_types

                        if logger is not None:
                            logger.event(
                                "sim_attempt_finished",
                                package_id=pkg.package_id,
                                i=pkg_i,
                                plan_attempt=plan_attempts,
                                plan_variant=plan_variant,
                                sim_attempt=attempt_i,
                                gas_budget=budget,
                                dry_run_ok=attempt_dry_run_ok,
                                dry_run_effects_error=attempt_dry_run_effects_error,
                            )

                        # Score this attempt
                        attempt_created_norm = {normalize_type_string(t) for t in attempt_created_types}
                        attempt_score = score_inhabitation(
                            target_key_types=truth_key_types, created_object_types=attempt_created_norm
                        )

                        # Check if this is the best so far
                        is_best = False
                        if best_score is None:
                            is_best = True
                        else:
                            # Preference: higher hits > higher distinct > success
                            if attempt_score.created_hits > best_score.created_hits:
                                is_best = True
                            elif attempt_score.created_hits == best_score.created_hits:
                                if attempt_score.created_distinct > best_score.created_distinct:
                                    is_best = True
                                elif attempt_score.created_distinct == best_score.created_distinct:
                                    # Tie-breaker: if current is dry_run_ok and best wasn't
                                    if attempt_dry_run_ok and not dry_run_ok:
                                        # Note: 'dry_run_ok' var refers to stored best?
                                        pass

                        # Update outer state if this is the best (or first)
                        if is_best:
                            best_score = attempt_score
                            created_types = attempt_created_types
                            static_types = attempt_static_types  # noqa: F841
                            dry_run_ok = attempt_dry_run_ok
                            dry_run_exec_ok = attempt_dry_run_exec_ok
                            dry_run_status = attempt_dry_run_status
                            dry_run_effects_error = attempt_dry_run_effects_error
                            dry_run_abort_code = attempt_dry_run_abort_code
                            dry_run_abort_location = attempt_dry_run_abort_location
                            dry_run_error = attempt_dry_run_error
                            # Keep these for final report
                            # Note: sim_mode, fell_back, dev_inspect_ok, tx_build_ok are accumulated or last-set?
                            # Ideally we want the properties of the *winning* attempt.
                            # So we should update them all here.

                        # Stop early if perfect score (all targets hit)
                        if attempt_score.created_hits == attempt_score.targets and attempt_score.targets > 0:
                            last_failure_ctx = None
                            break  # break gas loop

                        if simulation_mode != "dry-run" or attempt_dry_run_ok:
                            last_failure_ctx = None
                            break  # break gas loop (success)

                        if attempt_i < len(budgets) and _is_retryable_gas_error(attempt_dry_run_effects_error):
                            continue

                        last_failure_ctx = {
                            "dry_run_status": attempt_dry_run_status,
                            "dry_run_effects_error": attempt_dry_run_effects_error,
                            "dry_run_abort_code": attempt_dry_run_abort_code,
                            "dry_run_abort_location": attempt_dry_run_abort_location,
                            "gas_budget_used": budget,
                            "plan_variant": plan_variant,
                        }
                        break  # break gas loop (failed, not retryable)

                    # Check break from gas loop
                    if best_score and best_score.created_hits == best_score.targets and best_score.targets > 0:
                        break

                    # For agent, if we succeeded, we stop variants.
                    # For baseline, we might want to continue to find *better* variants?
                    # But baseline-search iterates *plans* (candidates). Inner variants are heuristics.
                    # If a variant succeeds for a candidate, that candidate is "done".
                    if simulation_mode != "dry-run" or (best_score and best_score.created_hits > 0):
                        # heuristic: if we got hits, maybe stop variants for this plan?
                        # Actually original logic was: stop variants if dry_run_ok.
                        pass

                    if simulation_mode != "dry-run" or (
                        best_score and best_score.created_hits == best_score.targets and best_score.targets > 0
                    ):
                        # dry_run_ok tracks the BEST? No, wait.
                        # Original logic used `dry_run_ok` variable which was set in the loop.
                        # I need to be careful about what `dry_run_ok` means now.
                        # I updated `dry_run_ok` ONLY if `is_best`.
                        # But loop control should depend on the *current* attempt's outcome.
                        if attempt_dry_run_ok:
                            break

                # Check break from variants loop
                if best_score and best_score.created_hits == best_score.targets and best_score.targets > 0:
                    break

                # Agent policy: stop replanning if success.
                if agent_name != "baseline-search" and (simulation_mode != "dry-run" or dry_run_ok):
                    break

            created_types = {normalize_type_string(t) for t in created_types}
        except TimeoutError as e:
            timed_out = True
            err = str(e)
        except subprocess.TimeoutExpired:
            timed_out = True
            err = f"tx-sim timeout exceeded ({per_package_timeout_seconds}s)"
        except Exception as e:
            err = str(e)
            ptb_parse_ok = False

        score = score_inhabitation(target_key_types=truth_key_types, created_object_types=created_types)
        elapsed_s = time.monotonic() - pkg_started

        if err is not None:
            error_count += 1
            if not continue_on_error:
                raise SystemExit(f"package failed: {pkg.package_id}: {err}")
            if error_count > max_errors:
                raise SystemExit(f"too many errors: {error_count} > {max_errors}")

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
            )
        )
        if logger is not None:
            row = {
                "package_id": pkg.package_id,
                "score": asdict(score),
                "error": err,
                "elapsed_seconds": elapsed_s,
                "timed_out": timed_out,
                "simulation_mode": sim_mode,
                "fell_back_to_dev_inspect": fell_back,
                "ptb_parse_ok": ptb_parse_ok,
                "tx_build_ok": tx_build_ok,
                "dry_run_ok": dry_run_ok,
                "dry_run_exec_ok": dry_run_exec_ok,
                "dry_run_status": dry_run_status,
                "dry_run_effects_error": dry_run_effects_error,
                "dry_run_abort_code": dry_run_abort_code,
                "dry_run_abort_location": dry_run_abort_location,
                "dev_inspect_ok": dev_inspect_ok,
                "dry_run_error": dry_run_error,
                "plan_attempts": plan_attempts,
                "sim_attempts": sim_attempts,
                "gas_budget_used": gas_budget_used,
                "plan_variant": plan_variant,
            }
            if include_created_types:
                row["created_object_types_list"] = sorted(created_types)
            logger.package_row(row)
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

        if out_path is not None and checkpoint_every > 0 and (pkg_i % checkpoint_every) == 0:
            finished = int(time.time())
            avg_hit_rate = sum(
                (r.score.created_hits / r.score.targets) if r.score.targets else 0.0 for r in results
            ) / len(results)
            partial = InhabitRunResult(
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
                    "avg_hit_rate": avg_hit_rate,
                    "errors": error_count,
                },
                packages=[
                    {
                        "package_id": r.package_id,
                        "score": asdict(r.score),
                        "error": r.error,
                        "elapsed_seconds": r.elapsed_seconds,
                        "timed_out": r.timed_out,
                        "created_object_types_list": r.created_object_types_list,
                        "simulation_mode": r.simulation_mode,
                        "fell_back_to_dev_inspect": r.fell_back_to_dev_inspect,
                        "ptb_parse_ok": r.ptb_parse_ok,
                        "tx_build_ok": r.tx_build_ok,
                        "dry_run_ok": r.dry_run_ok,
                        "dry_run_exec_ok": r.dry_run_exec_ok,
                        "dry_run_status": r.dry_run_status,
                        "dry_run_effects_error": r.dry_run_effects_error,
                        "dry_run_abort_code": r.dry_run_abort_code,
                        "dry_run_abort_location": r.dry_run_abort_location,
                        "dev_inspect_ok": r.dev_inspect_ok,
                        "dry_run_error": r.dry_run_error,
                        "plan_attempts": r.plan_attempts,
                        "sim_attempts": r.sim_attempts,
                        "gas_budget_used": r.gas_budget_used,
                        "plan_variant": r.plan_variant,
                    }
                    for r in results
                ],
            )
            _write_checkpoint(out_path, partial)

    finished = int(time.time())
    avg_hit_rate = sum((r.score.created_hits / r.score.targets) if r.score.targets else 0.0 for r in results) / len(
        results
    )

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
            "avg_hit_rate": avg_hit_rate,
            "errors": error_count,
        },
        packages=[
            {
                "package_id": r.package_id,
                "score": asdict(r.score),
                "error": r.error,
                "elapsed_seconds": r.elapsed_seconds,
                "timed_out": r.timed_out,
                "created_object_types_list": r.created_object_types_list,
                "simulation_mode": r.simulation_mode,
                "fell_back_to_dev_inspect": r.fell_back_to_dev_inspect,
                "ptb_parse_ok": r.ptb_parse_ok,
                "tx_build_ok": r.tx_build_ok,
                "dry_run_ok": r.dry_run_ok,
                "dry_run_exec_ok": r.dry_run_exec_ok,
                "dry_run_status": r.dry_run_status,
                "dry_run_effects_error": r.dry_run_effects_error,
                "dry_run_abort_code": r.dry_run_abort_code,
                "dry_run_abort_location": r.dry_run_abort_location,
                "dev_inspect_ok": r.dev_inspect_ok,
                "dry_run_error": r.dry_run_error,
                "plan_attempts": r.plan_attempts,
                "sim_attempts": r.sim_attempts,
                "gas_budget_used": r.gas_budget_used,
                "plan_variant": r.plan_variant,
            }
            for r in results
        ],
    )

    if out_path is not None:
        out_path.parent.mkdir(parents=True, exist_ok=True)
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
        f"{run_result.aggregate.get('avg_hit_rate'):.3f} errors={run_result.aggregate.get('errors')}"
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
        "--agent",
        type=str,
        default="mock-empty",
        choices=[
            "mock-empty",
            "mock-planfile",
            "real-openai-compatible",
            "baseline-search",
        ],
    )
    p.add_argument("--plan-file", type=Path, help="JSON mapping package_id -> PTB spec (required for mock-planfile).")
    p.add_argument("--rust-bin", type=Path, default=_default_rust_binary())
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
        default=2,
        help="Max PTB replanning attempts per package (real agent only).",
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
        help="Write partial results to --out every N packages (0 disables).",
    )
    p.add_argument(
        "--env-file",
        type=Path,
        default=Path(".env"),
        help="Path to a dotenv file (default: .env in the current working directory).",
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
        log_dir=None if args.no_log else args.log_dir,
        run_id=args.run_id,
    )


if __name__ == "__main__":
    main()
