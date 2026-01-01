from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from dataclasses import asdict, dataclass
from pathlib import Path

from rich.console import Console
from rich.progress import track

from smi_bench.agents.mock_agent import MockAgent
from smi_bench.dataset import collect_packages, sample_packages
from smi_bench.judge import KeyTypeScore, score_key_types

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
) -> RunResult:
    if build_rust:
        console.print("[bold]building rust…[/bold]")
        _build_rust()

    if not rust_bin.exists():
        raise SystemExit(
            f"rust binary not found: {rust_bin} (run `cargo build --release --locked`)"
        )

    started = int(time.time())

    packages = collect_packages(corpus_root)
    picked = sample_packages(packages, samples, seed)

    if not picked:
        raise SystemExit(f"no packages found under: {corpus_root}")

    agent = MockAgent(behavior=agent_name.replace("mock-", ""), seed=seed)
    results: list[PackageResult] = []

    for pkg in track(picked, description="benchmark"):
        interface_json = _run_rust_emit_bytecode_json(Path(pkg.package_dir), rust_bin)
        truth = _extract_key_types_from_interface_json(interface_json)
        predicted = agent.predict_key_types(truth_key_types=truth)
        score = score_key_types(truth, predicted)
        results.append(
            PackageResult(
                package_id=pkg.package_id,
                truth_key_types=len(truth),
                predicted_key_types=len(predicted),
                score=score,
            )
        )

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
        },
        packages=[
            {
                "package_id": r.package_id,
                "truth_key_types": r.truth_key_types,
                "predicted_key_types": r.predicted_key_types,
                "score": asdict(r.score),
            }
            for r in results
        ],
    )

    if out_path is not None:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(asdict(run_result), indent=2, sort_keys=True) + "\n")

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
        choices=["mock-perfect", "mock-empty", "mock-random", "mock-noisy"],
    )
    parser.add_argument("--rust-bin", type=Path, default=_default_rust_binary())
    parser.add_argument("--build-rust", action="store_true")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args(argv)

    run(
        corpus_root=args.corpus_root,
        samples=args.samples,
        seed=args.seed,
        agent_name=args.agent,
        rust_bin=args.rust_bin,
        build_rust=args.build_rust,
        out_path=args.out,
    )


if __name__ == "__main__":
    main()
