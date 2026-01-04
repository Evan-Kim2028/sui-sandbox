"""
Phase I benchmark runner (key-struct discovery).

Data flow (per package):
1) Load a local bytecode package dir from a sui-packages-style corpus.
2) Call the Rust extractor to emit bytecode-derived interface JSON to stdout.
3) Compute ground truth key types from the interface JSON (`abilities` contains "key").
4) Build an LLM prompt that intentionally omits `abilities` (to avoid trivial leakage) and may
   truncate struct context (bounded by --max-structs-in-prompt).
5) Score predictions via deterministic set matching (precision/recall/F1).

Maintainability notes:
- Keep output schema stable (see RunResult.schema_version).
- If you change prompt shaping (e.g., max structs), document it with results; it affects difficulty.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

import httpx
from rich.console import Console
from rich.progress import track

from smi_bench.agents.mock_agent import MockAgent
from smi_bench.agents.real_agent import RealAgent, load_real_agent_config
from smi_bench.checkpoint import (
    load_checkpoint,
    validate_checkpoint_compatibility,
    write_checkpoint,
)
from smi_bench.dataset import collect_packages, sample_packages
from smi_bench.env import load_dotenv
from smi_bench.judge import KeyTypeScore, score_key_types
from smi_bench.logging import JsonlLogger, default_run_id
from smi_bench.rust import build_rust as run_build_rust
from smi_bench.rust import default_rust_binary, emit_bytecode_json, validate_rust_binary
from smi_bench.schema import validate_phase1_run_json
from smi_bench.utils import (
    extract_key_types_from_interface_json,
    find_git_root,
    log_exception,
    retry_with_backoff,
    safe_read_lines,
)

console = Console()


def _redact_secret(v: str | None) -> str:
    if not v:
        return "<missing>"
    suffix = v[-6:] if len(v) >= 6 else v
    return f"len={len(v)} suffix={suffix}"


def _doctor_real_agent(cfg_env: dict[str, str]) -> None:
    cfg = load_real_agent_config(cfg_env)
    console.print("[bold]Real-agent diagnostics[/bold]")
    console.print(f"provider: {cfg.provider}")
    console.print(f"base_url: {cfg.base_url}")
    console.print(f"model: {cfg.model}")
    console.print(f"api_key: {_redact_secret(cfg.api_key)}")

    headers = {"Authorization": f"Bearer {cfg.api_key}"}
    base = cfg.base_url.rstrip("/")

    # Probe models listing (helps confirm auth + endpoint wiring).
    try:
        r = httpx.get(f"{base}/models", headers=headers, timeout=30)
        console.print(f"GET {base}/models -> {r.status_code}")
        if r.status_code == 200:
            data = r.json().get("data", [])
            ids = [m.get("id") for m in data if isinstance(m, dict)]
            console.print(f"models: {ids}")
        else:
            console.print(r.text[:400])
    except (httpx.RequestError, httpx.HTTPStatusError, httpx.TimeoutException) as e:
        console.print(f"[red]GET {base}/models failed:[/red] {e!r}")

    # Probe one minimal chat completion.
    payload = {"model": cfg.model, "messages": [{"role": "user", "content": 'Return {"key_types": []} as JSON.'}]}
    if cfg.thinking:
        payload["thinking"] = {"type": cfg.thinking}
        if cfg.clear_thinking is not None:
            payload["thinking"]["clear_thinking"] = cfg.clear_thinking
    if cfg.response_format:
        payload["response_format"] = {"type": cfg.response_format}
    try:
        r = httpx.post(
            f"{base}/chat/completions",
            headers={"Content-Type": "application/json", **headers},
            json=payload,
            timeout=60,
        )
        console.print(f"POST {base}/chat/completions -> {r.status_code}")
        console.print(r.text[:400])
    except (httpx.RequestError, httpx.HTTPStatusError, httpx.TimeoutException) as e:
        console.print(f"[red]POST {base}/chat/completions failed:[/red] {e!r}")


def _build_agent_prompt(interface_json: dict[str, Any], *, max_structs: int) -> str:
    """
    Build a prompt that hides `abilities` to avoid trivial extraction.

    The model must infer likely `key` structs from structure/fields alone.
    This prevents the benchmark from being trivial (if abilities were shown,
    the model could just return all structs with "key" ability).

    Args:
        interface_json: Parsed bytecode interface JSON (from Rust extractor).
        max_structs: Maximum number of structs to include in prompt (truncation limit).

    Returns:
        Formatted prompt string for the LLM.
    """
    package_id = interface_json.get("package_id", "<unknown>")
    modules = interface_json.get("modules", {})

    prompt_modules: dict[str, object] = {}
    structs_added = 0

    if isinstance(modules, dict):
        for module_name, module_def in modules.items():
            if not isinstance(module_name, str) or not isinstance(module_def, dict):
                continue
            addr = module_def.get("address")
            structs = module_def.get("structs")
            if not isinstance(addr, str) or not isinstance(structs, dict):
                continue

            prompt_structs: dict[str, object] = {}
            for struct_name, struct_def in structs.items():
                if structs_added >= max_structs:
                    break
                if not isinstance(struct_name, str) or not isinstance(struct_def, dict):
                    continue
                fields = struct_def.get("fields")
                if not isinstance(fields, list):
                    continue

                prompt_fields = []
                for f in fields:
                    if not isinstance(f, dict):
                        continue
                    name = f.get("name")
                    ty = f.get("type")
                    if isinstance(name, str):
                        prompt_fields.append({"name": name, "type": ty})

                prompt_structs[struct_name] = {
                    "fields": prompt_fields,
                }
                structs_added += 1

            if prompt_structs:
                prompt_modules[module_name] = {"address": addr, "structs": prompt_structs}
            if structs_added >= max_structs:
                break

    payload = {
        "package_id": package_id,
        "modules": prompt_modules,
        "note": "Struct abilities are intentionally omitted.",
    }

    instructions = (
        "Given the Sui Move package structure below, return a JSON object with a single key "
        '"key_types" that maps to a JSON array of type strings in the form "0xADDR::module::Struct". '
        "Include only structs that you believe have the Move ability `key`.\n"
        "Output ONLY valid JSON.\n"
    )
    return instructions + json.dumps(payload, indent=2, sort_keys=True)


def _git_head_for_path(path: Path) -> dict[str, str] | None:
    """
    Get git HEAD commit SHA for a path's repository.

    Used for run attribution metadata to track which corpus version was used.

    Args:
        path: Path within a git repository.

    Returns:
        Dict with "head" key containing commit SHA, or None if not in git repo or git fails.
    """
    root = find_git_root(path)
    if root is None:
        return None
    try:
        head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, FileNotFoundError):
        return None
    return {"head": head}


def _load_ids_file_ordered(path: Path) -> list[str]:
    """
    Load a newline-delimited package id list.

    Preserves file order, ignores blank lines and '#' comments, and deduplicates
    while preserving first occurrence.

    Args:
        path: Path to newline-delimited file.

    Returns:
        List of package IDs in file order (deduplicated).
    """
    seen: set[str] = set()
    out: list[str] = []
    for line in safe_read_lines(path, context=f"ids-file {path}"):
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        if s in seen:
            continue
        seen.add(s)
        out.append(s)
    return out


@dataclass
class PackageResult:
    package_id: str
    truth_key_types: int
    predicted_key_types: int
    score: KeyTypeScore
    error: str | None = None
    elapsed_seconds: float | None = None
    attempts: int | None = None
    max_structs_used: int | None = None
    timed_out: bool | None = None
    truth_key_types_list: list[str] | None = None
    predicted_key_types_list: list[str] | None = None


def _resume_results_from_checkpoint(
    cp: RunResult, *, logger: JsonlLogger | None = None
) -> tuple[list[PackageResult], set[str], int, int]:
    """
    Resume package results from a checkpoint.

    Extracts completed packages from checkpoint and reconstructs PackageResult objects.
    Gracefully skips malformed package rows and logs skip events.

    Args:
        cp: Checkpoint RunResult to resume from.
        logger: Optional logger for skip events.

    Returns:
        Tuple of (results_list, seen_package_ids_set, error_count, started_timestamp).
    """
    results: list[PackageResult] = []
    seen: set[str] = set()
    errors = cp.aggregate.get("errors") if isinstance(cp.aggregate, dict) else 0
    if errors is None:
        errors = 0
    try:
        error_count = int(errors)
    except (ValueError, TypeError):
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
            score = KeyTypeScore(**score_d)
        except (TypeError, ValueError) as e:
            # Log but continue - malformed score shouldn't break resume
            if logger is not None:
                logger.event("checkpoint_resume_skip", package_id=pkg_id, reason=f"invalid score: {e}")
            continue
        truth_k = int(row.get("truth_key_types", 0))
        pred_k = int(row.get("predicted_key_types", 0))
        err = row.get("error")
        results.append(
            PackageResult(
                package_id=pkg_id,
                truth_key_types=truth_k,
                predicted_key_types=pred_k,
                score=score,
                error=err if isinstance(err, str) else None,
            )
        )
        seen.add(pkg_id)

    started = cp.started_at_unix_seconds
    return results, seen, error_count, started


@dataclass
class RunResult:
    """
    Phase I benchmark run result (schema contract).

    Schema versioning:
    - Increment `schema_version` when adding/removing/renaming fields or changing semantics.
    - Backward compatibility: older readers should handle missing optional fields gracefully.
    - Determinism: `packages` list order should be stable (sorted by package_id) for reproducible diffs.

    Field contracts:
    - `schema_version`: integer, must match expected version for strict validation.
    - `aggregate`: dict with keys like `"avg_f1"`, `"avg_precision"`, `"avg_recall"`, `"errors"`, `"timeouts"`.
    - `packages`: list of dicts, each with `"package_id"`, `"score"` (KeyTypeScore dict),
      `"truth_key_types"`, `"predicted_key_types"`.
    """

    schema_version: int
    started_at_unix_seconds: int
    finished_at_unix_seconds: int
    corpus_root_name: str
    corpus_git: dict[str, str] | None
    target_ids_file: str | None
    target_ids_total: int | None
    samples: int
    seed: int
    agent: str
    aggregate: dict[str, Any]
    packages: list[dict[str, Any]]


def run(
    *,
    corpus_root: Path,
    samples: int,
    seed: int,
    package_ids_file: Path | None,
    agent_name: str,
    rust_bin: Path,
    build_rust: bool,
    out_path: Path | None,
    env_file: Path | None,
    max_structs_in_prompt: int,
    smoke_agent: bool,
    doctor_agent: bool,
    continue_on_error: bool,
    max_errors: int,
    checkpoint_every: int,
    resume: bool,
    per_package_timeout_seconds: float,
    include_type_lists: bool,
    log_dir: Path | None,
    run_id: str | None,
) -> RunResult:
    """
    Run Phase I benchmark (key-struct discovery).

    Processes packages from corpus, extracts key types from bytecode, prompts LLM
    to predict key types, and scores predictions. Supports checkpointing and resume.

    Args:
        corpus_root: Root directory of sui-packages-style corpus.
        samples: Number of packages to sample (0 = all).
        seed: Random seed for sampling.
        package_ids_file: Optional file with specific package IDs to process.
        agent_name: Agent name ("mock-*" or "real-openai-compatible").
        rust_bin: Path to Rust extractor binary.
        build_rust: If True, build Rust binary before running.
        out_path: Optional path to write checkpoint JSON.
        env_file: Optional path to .env file for agent config.
        max_structs_in_prompt: Maximum structs to include in prompt (truncation).
        smoke_agent: If True, run smoke test on agent and exit.
        doctor_agent: If True, run diagnostics on agent and exit.
        continue_on_error: If True, continue processing after errors.
        max_errors: Maximum errors before stopping (if continue_on_error=True).
        checkpoint_every: Write checkpoint every N packages (0 = disabled).
        resume: If True, resume from existing checkpoint at out_path.
        per_package_timeout_seconds: Timeout per package in seconds.
        include_type_lists: If True, include type lists in output.
        log_dir: Directory for JSONL logs (None = disabled).
        run_id: Optional run ID for logs (auto-generated if None).

    Returns:
        RunResult with aggregated scores and package results.

    Raises:
        SystemExit: If binary validation fails or too many errors encountered.
    """
    if build_rust:
        console.print("[bold]building rust…[/bold]")
        run_build_rust()

    # Validate binary exists and is executable
    try:
        rust_bin = validate_rust_binary(rust_bin)
    except (FileNotFoundError, PermissionError) as e:
        raise SystemExit(str(e)) from e

    env_overrides = load_dotenv(env_file) if env_file is not None else {}

    logger: JsonlLogger | None = None
    if log_dir is not None:
        rid = run_id or default_run_id(prefix="phase1")
        logger = JsonlLogger(base_dir=log_dir, run_id=rid)

    if doctor_agent:
        _doctor_real_agent(env_overrides)
        raise SystemExit(0)

    if smoke_agent:
        cfg = load_real_agent_config(env_overrides)
        agent = RealAgent(cfg)
        _ = agent.smoke()
        console.print("[green]real agent smoke ok[/green]")
        raise SystemExit(0)

    started = int(time.time())
    if logger is not None:
        logger.write_run_metadata(
            {
                "schema_version": 1,
                "benchmark": "phase1_key_struct_discovery",
                "started_at_unix_seconds": started,
                "agent": agent_name,
                "seed": seed,
                "corpus_root": str(corpus_root),
                "argv": list(map(str, sys.argv)),
            }
        )
        logger.event("run_started", started_at_unix_seconds=started, agent=agent_name, seed=seed)

    packages = collect_packages(corpus_root)
    target_ids_file_s: str | None = None
    target_ids_total: int | None = None
    if package_ids_file is not None:
        ids = _load_ids_file_ordered(package_ids_file)
        by_id = {p.package_id: p for p in packages}
        picked = [by_id[i] for i in ids if i in by_id]
        target_ids_file_s = package_ids_file.name
        target_ids_total = len(picked)
    else:
        picked = sample_packages(packages, samples, seed)

    if not picked:
        raise SystemExit(f"no packages found under: {corpus_root}")

    results: list[PackageResult] = []
    error_count = 0
    if resume:
        if out_path is None:
            raise SystemExit("--resume requires --out")
        if out_path.exists():
            cp_data = load_checkpoint(out_path)
            validate_checkpoint_compatibility(
                cp_data,
                {
                    "agent": agent_name,
                    "seed": seed,
                    "schema_version": 1,
                },
            )
            cp = RunResult(**cp_data)
            results, seen, error_count, started = _resume_results_from_checkpoint(cp)
            picked = [p for p in picked if p.package_id not in seen]
            console.print(f"[yellow]resuming:[/yellow] already_done={len(seen)} remaining={len(picked)}")

    if package_ids_file is not None:
        # Treat --samples as a batch size over the manifest order (works with --resume).
        if samples > 0 and samples < len(picked):
            picked = picked[:samples]

    if agent_name.startswith("mock-"):
        agent = MockAgent(behavior=agent_name.replace("mock-", ""), seed=seed)
        real_agent: RealAgent | None = None
    elif agent_name == "real-openai-compatible":
        cfg = load_real_agent_config(env_overrides)
        real_agent = RealAgent(cfg)
        agent = None
    else:
        raise SystemExit(f"unknown agent: {agent_name}")

    done_already = len(results)
    for pkg_i, pkg in enumerate(track(picked, description="benchmark"), start=done_already + 1):
        pkg_started = time.monotonic()
        deadline = pkg_started + per_package_timeout_seconds
        if logger is not None:
            logger.event("package_started", package_id=pkg.package_id, i=pkg_i)

        try:
            interface_json = retry_with_backoff(
                lambda: emit_bytecode_json(package_dir=Path(pkg.package_dir), rust_bin=rust_bin),
                max_attempts=3,
                base_delay=2.0,
                retryable_exceptions=(RuntimeError, TimeoutError),
            )
        except (RuntimeError, TimeoutError) as e:
            # Handle failure early before scoring
            err = f"bytecode extraction failed after retries: {e}"
            interface_json = {"package_id": pkg.package_id, "modules": {}}

        truth = extract_key_types_from_interface_json(interface_json)
        predicted: set[str] = set()
        err: str | None = None
        attempts = 0
        max_structs_used: int | None = None
        timed_out = False
        if real_agent is not None:
            cur_max_structs = max_structs_in_prompt
            last_exc: Exception | None = None
            for _attempt in range(4):
                if time.monotonic() >= deadline:
                    timed_out = True
                    last_exc = TimeoutError(f"per-package timeout exceeded ({per_package_timeout_seconds}s)")
                    break
                prompt = _build_agent_prompt(interface_json, max_structs=cur_max_structs)
                try:
                    attempts += 1
                    max_structs_used = cur_max_structs
                    remaining = max(0.0, deadline - time.monotonic())
                    predicted = real_agent.complete_type_list(prompt, timeout_s=max(1.0, remaining))
                    last_exc = None
                    break
                except (RuntimeError, ValueError, httpx.RequestError, TimeoutError) as e:
                    # Catch specific exceptions from agent or logic.
                    last_exc = e
                    msg = str(e)
                    if isinstance(e, TimeoutError):
                        timed_out = True
                        break
                    if isinstance(e, ValueError) and ("empty content" in msg or "hit max_tokens" in msg):
                        next_max = max(5, cur_max_structs // 2)
                        if next_max == cur_max_structs:
                            break
                        cur_max_structs = next_max
                        continue
                    break
            else:
                last_exc = last_exc or RuntimeError("unknown error")

            if predicted == set() and last_exc is not None:
                err = f"real-agent call failed: {last_exc}"
                predicted = set()
        else:
            assert agent is not None
            predicted = agent.predict_key_types(truth_key_types=truth)

        if err is not None:
            error_count += 1
            log_exception("Package processing failed", extra={"package_id": pkg.package_id, "error": err})
            if not continue_on_error:
                raise RuntimeError(
                    f"Package processing failed: {pkg.package_id}\n"
                    f"  Error: {err}\n"
                    f"  Check package structure and agent configuration."
                )
            if error_count > max_errors:
                raise RuntimeError(
                    f"Too many errors encountered: {error_count} > {max_errors}\n"
                    f"  Last error: {err}\n"
                    f"  Increase --max-errors or fix underlying issues.\n"
                    f"  Check logs for details on failed packages."
                )

        score = score_key_types(truth, predicted)
        elapsed_s = time.monotonic() - pkg_started
        truth_list = sorted(truth) if include_type_lists else None
        predicted_list = sorted(predicted) if include_type_lists else None
        results.append(
            PackageResult(
                package_id=pkg.package_id,
                truth_key_types=len(truth),
                predicted_key_types=len(predicted),
                score=score,
                error=err,
                elapsed_seconds=elapsed_s,
                attempts=attempts,
                max_structs_used=max_structs_used,
                timed_out=timed_out,
                truth_key_types_list=truth_list,
                predicted_key_types_list=predicted_list,
            )
        )
        if logger is not None:
            logger.package_row(
                {
                    "package_id": pkg.package_id,
                    "truth_key_types": len(truth),
                    "predicted_key_types": len(predicted),
                    "score": asdict(score),
                    "error": err,
                    "elapsed_seconds": elapsed_s,
                    "attempts": attempts,
                    "max_structs_used": max_structs_used,
                    "timed_out": timed_out,
                }
            )
            logger.event(
                "package_finished",
                package_id=pkg.package_id,
                i=pkg_i,
                elapsed_seconds=elapsed_s,
                error=err,
                timed_out=timed_out,
            )

        if out_path is not None and checkpoint_every > 0 and (pkg_i % checkpoint_every) == 0:
            finished = int(time.time())
            if len(results) > 0:
                avg_f1 = sum(r.score.f1 for r in results) / len(results)
                avg_recall = sum(r.score.recall for r in results) / len(results)
                avg_precision = sum(r.score.precision for r in results) / len(results)
            else:
                avg_f1 = avg_recall = avg_precision = 0.0
            partial = RunResult(
                schema_version=1,
                started_at_unix_seconds=started,
                finished_at_unix_seconds=finished,
                corpus_root_name=corpus_root.name,
                corpus_git=_git_head_for_path(corpus_root),
                target_ids_file=target_ids_file_s,
                target_ids_total=target_ids_total,
                samples=len(results),
                seed=seed,
                agent=agent_name,
                aggregate={
                    "avg_precision": avg_precision,
                    "avg_recall": avg_recall,
                    "avg_f1": avg_f1,
                    "errors": error_count,
                },
                packages=[
                    {
                        "package_id": r.package_id,
                        "truth_key_types": r.truth_key_types,
                        "predicted_key_types": r.predicted_key_types,
                        "score": asdict(r.score),
                        "error": r.error,
                        "elapsed_seconds": r.elapsed_seconds,
                        "attempts": r.attempts,
                        "max_structs_used": r.max_structs_used,
                        "timed_out": r.timed_out,
                        "truth_key_types_list": r.truth_key_types_list,
                        "predicted_key_types_list": r.predicted_key_types_list,
                    }
                    for r in results
                ],
            )
            write_checkpoint(out_path, partial, validate_fn=validate_phase1_run_json)

    finished = int(time.time())

    if len(results) > 0:
        avg_f1 = sum(r.score.f1 for r in results) / len(results)
        avg_recall = sum(r.score.recall for r in results) / len(results)
        avg_precision = sum(r.score.precision for r in results) / len(results)
    else:
        avg_f1 = avg_recall = avg_precision = 0.0

    run_result = RunResult(
        schema_version=1,
        started_at_unix_seconds=started,
        finished_at_unix_seconds=finished,
        corpus_root_name=corpus_root.name,
        corpus_git=_git_head_for_path(corpus_root),
        target_ids_file=target_ids_file_s,
        target_ids_total=target_ids_total,
        samples=len(results),
        seed=seed,
        agent=agent_name,
        aggregate={
            "avg_precision": avg_precision,
            "avg_recall": avg_recall,
            "avg_f1": avg_f1,
            "errors": error_count,
        },
        packages=[
            {
                "package_id": r.package_id,
                "truth_key_types": r.truth_key_types,
                "predicted_key_types": r.predicted_key_types,
                "score": asdict(r.score),
                "error": r.error,
                "elapsed_seconds": r.elapsed_seconds,
                "attempts": r.attempts,
                "max_structs_used": r.max_structs_used,
                "timed_out": r.timed_out,
                "truth_key_types_list": r.truth_key_types_list,
                "predicted_key_types_list": r.predicted_key_types_list,
            }
            for r in results
        ],
    )

    if out_path is not None:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        write_checkpoint(out_path, run_result, validate_fn=validate_phase1_run_json)

    if logger is not None:
        logger.event(
            "run_finished",
            finished_at_unix_seconds=finished,
            samples=len(results),
            errors=error_count,
            avg_f1=avg_f1,
        )

    return run_result


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Key-struct target discovery benchmark")
    parser.add_argument("--corpus-root", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=25)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument(
        "--package-ids-file",
        type=Path,
        help="Optional file of package ids to run in order (1 per line; '#' comments allowed).",
    )
    parser.add_argument(
        "--agent",
        type=str,
        default="mock-empty",
        choices=[
            "mock-perfect",
            "mock-empty",
            "mock-random",
            "mock-noisy",
            "real-openai-compatible",
        ],
    )
    parser.add_argument("--rust-bin", type=Path, default=default_rust_binary())
    parser.add_argument("--build-rust", action="store_true")
    parser.add_argument("--out", type=Path)
    parser.add_argument(
        "--resume",
        action="store_true",
        help="If --out exists, resume from it (skip completed package_ids and append).",
    )
    parser.add_argument(
        "--env-file",
        type=Path,
        default=Path(".env"),
        help="Path to a dotenv file (default: .env in the current working directory).",
    )
    parser.add_argument("--max-structs-in-prompt", type=int, default=200)
    parser.add_argument(
        "--per-package-timeout-seconds",
        type=float,
        default=120.0,
        help="Maximum wall-clock time to spend per package (real-agent mode).",
    )
    parser.add_argument(
        "--include-type-lists",
        action="store_true",
        help="Include full truth/predicted key type lists per package in the output JSON (larger files).",
    )
    parser.add_argument(
        "--smoke-agent",
        action="store_true",
        help="Run a minimal real-agent call and exit (requires env vars).",
    )
    parser.add_argument(
        "--doctor-agent",
        action="store_true",
        help="Print real-agent config (redacted) and probe /models + /chat/completions.",
    )
    parser.add_argument(
        "--continue-on-error",
        action="store_true",
        help="Continue benchmark even if a package fails (records error and scores it as empty prediction).",
    )
    parser.add_argument(
        "--max-errors",
        type=int,
        default=25,
        help="Stop the run if more than this many package errors occur (with --continue-on-error).",
    )
    parser.add_argument(
        "--checkpoint-every",
        type=int,
        default=10,
        help="Write partial results to --out every N packages (0 disables).",
    )
    parser.add_argument(
        "--log-dir",
        type=Path,
        default=Path("logs"),
        help="Directory to write JSONL logs under (default: benchmark/logs). Use --no-log to disable.",
    )
    parser.add_argument("--run-id", type=str, help="Optional run id for log directory naming.")
    parser.add_argument("--no-log", action="store_true", help="Disable JSONL logging.")
    args = parser.parse_args(argv)

    env_file = args.env_file if args.env_file.exists() else None
    if env_file is None:
        # Convenience: if user runs from repo root, allow benchmark/.env without extra flags.
        fallback = Path("benchmark/.env")
        if fallback.exists():
            env_file = fallback

    run(
        corpus_root=args.corpus_root,
        samples=args.samples,
        seed=args.seed,
        package_ids_file=args.package_ids_file,
        agent_name=args.agent,
        rust_bin=args.rust_bin,
        build_rust=args.build_rust,
        out_path=args.out,
        env_file=env_file,
        max_structs_in_prompt=args.max_structs_in_prompt,
        smoke_agent=args.smoke_agent,
        doctor_agent=args.doctor_agent,
        continue_on_error=args.continue_on_error,
        max_errors=args.max_errors,
        checkpoint_every=args.checkpoint_every,
        resume=args.resume,
        per_package_timeout_seconds=args.per_package_timeout_seconds,
        include_type_lists=args.include_type_lists,
        log_dir=None if args.no_log else args.log_dir,
        run_id=args.run_id,
    )


if __name__ == "__main__":
    main()
