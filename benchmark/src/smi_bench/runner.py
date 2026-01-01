from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from dataclasses import asdict, dataclass
from pathlib import Path

import httpx
from rich.console import Console
from rich.progress import track

from smi_bench.agents.mock_agent import MockAgent
from smi_bench.agents.real_agent import RealAgent, load_real_agent_config
from smi_bench.dataset import collect_packages, sample_packages
from smi_bench.judge import KeyTypeScore, score_key_types
from smi_bench.env import load_dotenv

console = Console()


def _default_rust_binary() -> Path:
    repo_root = Path(__file__).resolve().parents[3]
    exe = "sui_move_interface_extractor.exe" if os.name == "nt" else "sui_move_interface_extractor"
    local = repo_root / "target" / "release" / exe
    if local.exists():
        return local
    system = Path("/usr/local/bin") / exe
    return system


def _build_rust() -> None:
    repo_root = Path(__file__).resolve().parents[3]
    subprocess.check_call(["cargo", "build", "--release", "--locked"], cwd=repo_root)


def _run_rust_emit_bytecode_json(package_dir: Path, rust_bin: Path) -> dict:
    cmd = [
        str(rust_bin),
        "--bytecode-package-dir",
        str(package_dir),
        "--emit-bytecode-json",
        "-",
    ]
    out = subprocess.check_output(cmd, text=True)
    return json.loads(out)


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
    except Exception as e:
        console.print(f"[red]GET {base}/models failed:[/red] {e!r}")

    # Probe one minimal chat completion.
    payload = {"model": cfg.model, "messages": [{"role": "user", "content": "Return {\"key_types\": []} as JSON."}]}
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
    except Exception as e:
        console.print(f"[red]POST {base}/chat/completions failed:[/red] {e!r}")


def _build_agent_prompt(interface_json: dict, *, max_structs: int) -> str:
    """
    Build a prompt that hides `abilities` to avoid trivial extraction.
    The model must infer likely `key` structs from structure/fields.
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


def _extract_key_types_from_interface_json(interface_json: dict) -> set[str]:
    out: set[str] = set()
    modules = interface_json.get("modules")
    if not isinstance(modules, dict):
        return out

    for module_name, module_def in modules.items():
        if not isinstance(module_name, str) or not isinstance(module_def, dict):
            continue
        address = module_def.get("address")
        if not isinstance(address, str):
            continue
        structs = module_def.get("structs")
        if not isinstance(structs, dict):
            continue
        for struct_name, struct_def in structs.items():
            if not isinstance(struct_name, str) or not isinstance(struct_def, dict):
                continue
            abilities = struct_def.get("abilities")
            if not isinstance(abilities, list):
                continue
            if "key" in abilities:
                out.add(f"{address}::{module_name}::{struct_name}")
    return out


def _find_git_root(start: Path) -> Path | None:
    cur = start.resolve()
    while True:
        if (cur / ".git").exists():
            return cur
        if cur.parent == cur:
            return None
        cur = cur.parent


def _git_head_for_path(path: Path) -> dict | None:
    root = _find_git_root(path)
    if root is None:
        return None
    try:
        head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    except Exception:
        return None
    return {"head": head}


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


def _write_checkpoint(out_path: Path, run_result: RunResult) -> None:
    tmp = out_path.with_suffix(out_path.suffix + ".tmp")
    tmp.write_text(json.dumps(asdict(run_result), indent=2, sort_keys=True) + "\n")
    tmp.replace(out_path)


def _load_checkpoint(out_path: Path) -> RunResult:
    try:
        data = json.loads(out_path.read_text())
    except Exception as e:
        raise RuntimeError(f"failed to parse checkpoint JSON: {out_path}") from e

    try:
        schema_version = int(data["schema_version"])
        started = int(data["started_at_unix_seconds"])
        finished = int(data["finished_at_unix_seconds"])
        corpus_root_name = str(data["corpus_root_name"])
        corpus_git = data.get("corpus_git")
        samples = int(data["samples"])
        seed = int(data["seed"])
        agent = str(data["agent"])
        aggregate = data.get("aggregate")
        packages = data.get("packages")
    except Exception as e:
        raise RuntimeError(f"invalid checkpoint shape: {out_path}") from e

    if not isinstance(aggregate, dict) or not isinstance(packages, list):
        raise RuntimeError(f"invalid checkpoint shape: {out_path}")

    return RunResult(
        schema_version=schema_version,
        started_at_unix_seconds=started,
        finished_at_unix_seconds=finished,
        corpus_root_name=corpus_root_name,
        corpus_git=corpus_git if isinstance(corpus_git, dict) else None,
        samples=samples,
        seed=seed,
        agent=agent,
        aggregate=aggregate,
        packages=packages,
    )


def _resume_results_from_checkpoint(cp: RunResult) -> tuple[list[PackageResult], set[str], int, int]:
    results: list[PackageResult] = []
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
            score = KeyTypeScore(**score_d)
        except Exception:
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
    schema_version: int
    started_at_unix_seconds: int
    finished_at_unix_seconds: int
    corpus_root_name: str
    corpus_git: dict | None
    samples: int
    seed: int
    agent: str
    aggregate: dict
    packages: list[dict]


def run(
    *,
    corpus_root: Path,
    samples: int,
    seed: int,
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
) -> RunResult:
    if build_rust:
        console.print("[bold]building rust…[/bold]")
        _build_rust()

    if not rust_bin.exists():
        raise SystemExit(
            f"rust binary not found: {rust_bin} (run `cargo build --release --locked`)"
        )

    env_overrides = load_dotenv(env_file) if env_file is not None else {}

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

    packages = collect_packages(corpus_root)
    picked = sample_packages(packages, samples, seed)

    if not picked:
        raise SystemExit(f"no packages found under: {corpus_root}")

    results: list[PackageResult] = []
    error_count = 0
    if resume:
        if out_path is None:
            raise SystemExit("--resume requires --out")
        if out_path.exists():
            cp = _load_checkpoint(out_path)
            if cp.agent != agent_name or cp.seed != seed:
                raise SystemExit(
                    f"checkpoint mismatch: out has agent={cp.agent} seed={cp.seed}, expected agent={agent_name} seed={seed}"
                )
            results, seen, error_count, started = _resume_results_from_checkpoint(cp)
            picked = [p for p in picked if p.package_id not in seen]
            console.print(f"[yellow]resuming:[/yellow] already_done={len(seen)} remaining={len(picked)}")

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
        interface_json = _run_rust_emit_bytecode_json(Path(pkg.package_dir), rust_bin)
        truth = _extract_key_types_from_interface_json(interface_json)
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
                except Exception as e:
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
            predicted = agent.predict_key_types(truth_key_types=truth)

        if err is not None:
            error_count += 1
            if not continue_on_error:
                raise RuntimeError(err)
            if error_count > max_errors:
                raise RuntimeError(f"too many errors ({error_count} > {max_errors}); last_error={err}")

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

        if out_path is not None and checkpoint_every > 0 and (pkg_i % checkpoint_every) == 0:
            finished = int(time.time())
            avg_f1 = sum(r.score.f1 for r in results) / len(results)
            avg_recall = sum(r.score.recall for r in results) / len(results)
            avg_precision = sum(r.score.precision for r in results) / len(results)
            partial = RunResult(
                schema_version=1,
                started_at_unix_seconds=started,
                finished_at_unix_seconds=finished,
                corpus_root_name=corpus_root.name,
                corpus_git=_git_head_for_path(corpus_root),
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
            _write_checkpoint(out_path, partial)

    finished = int(time.time())

    avg_f1 = sum(r.score.f1 for r in results) / len(results)
    avg_recall = sum(r.score.recall for r in results) / len(results)
    avg_precision = sum(r.score.precision for r in results) / len(results)

    run_result = RunResult(
        schema_version=1,
        started_at_unix_seconds=started,
        finished_at_unix_seconds=finished,
        corpus_root_name=corpus_root.name,
        corpus_git=_git_head_for_path(corpus_root),
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
        _write_checkpoint(out_path, run_result)

    return run_result


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Key-struct target discovery benchmark")
    parser.add_argument("--corpus-root", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=25)
    parser.add_argument("--seed", type=int, default=0)
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
    parser.add_argument("--rust-bin", type=Path, default=_default_rust_binary())
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
    )


if __name__ == "__main__":
    main()
