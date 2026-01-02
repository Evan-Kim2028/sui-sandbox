from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from typing import Any

import httpx


def _port_is_listening(port: int) -> bool:
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(0.2)
        return s.connect_ex(("127.0.0.1", port)) == 0


def _kill_listeners(port: int) -> None:
    import subprocess

    # Best-effort: find PIDs listening on the port and terminate them.
    out = subprocess.run(
        ["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-t"],
        capture_output=True,
        text=True,
        check=False,
    )
    pids = []
    for line in (out.stdout or "").splitlines():
        line = line.strip()
        if line.isdigit():
            pids.append(int(line))
    if not pids:
        return

    for pid in pids:
        subprocess.run(["kill", str(pid)], check=False)


def _truncate_manifest(in_path: Path, *, n: int, out_path: Path) -> None:
    lines: list[str] = []
    for raw in in_path.read_text(encoding="utf-8").splitlines():
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        lines.append(s)
        if len(lines) >= n:
            break
    out_path.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")


def _default_request(
    *,
    corpus_root: str,
    package_ids_file: str,
    samples: int,
    timeout_s: float,
    rpc_url: str,
    simulation_mode: str,
    sender: str | None,
    max_plan_attempts: int,
    max_planning_calls: int | None,
    continue_on_error: bool,
    resume: bool,
) -> dict[str, Any]:
    cfg: dict[str, Any] = {
        "corpus_root": corpus_root,
        "package_ids_file": package_ids_file,
        "samples": samples,
        "rpc_url": rpc_url,
        "simulation_mode": simulation_mode,
        "per_package_timeout_seconds": timeout_s,
        "max_plan_attempts": max(1, int(max_plan_attempts)),
        "continue_on_error": bool(continue_on_error),
        "resume": bool(resume),
    }
    if max_planning_calls is not None:
        cfg["max_planning_calls"] = max(1, int(max_planning_calls))
    if sender:
        cfg["sender"] = sender
    return {
        "jsonrpc": "2.0",
        "id": "1",
        "method": "message/send",
        "params": {
            "message": {
                "messageId": f"smi_a2a_smoke_{int(time.time())}",
                "role": "user",
                "parts": [{"text": json.dumps({"config": cfg})}],
            }
        },
    }


def _extract_bundle(resp: dict[str, Any]) -> dict[str, Any]:
    result = resp.get("result")
    if not isinstance(result, dict):
        raise ValueError("missing result")
    artifacts = result.get("artifacts")
    if not isinstance(artifacts, list):
        raise ValueError("missing artifacts")
    for a in artifacts:
        if isinstance(a, dict) and a.get("name") == "evaluation_bundle":
            parts = a.get("parts")
            if isinstance(parts, list) and parts:
                p0 = parts[0]
                if isinstance(p0, dict) and isinstance(p0.get("text"), str):
                    return json.loads(p0["text"])
    raise ValueError("evaluation_bundle not found")


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="Run local A2A smoke test")
    p.add_argument("--scenario", type=str, default="scenario_smi")
    p.add_argument("--green-url", type=str, default="http://127.0.0.1:9999/")
    p.add_argument(
        "--env-file",
        type=Path,
        default=Path(".env"),
        help="Dotenv file to load for scenario-launched agents (process env still wins).",
    )
    p.add_argument("--corpus-root", type=str, required=True)
    p.add_argument("--package-ids-file", type=str, default=None)
    p.add_argument("--samples", type=int, default=1)
    p.add_argument(
        "--smoke",
        action="store_true",
        help="Fast feedback mode (default: 2 packages, 120s timeout, low budgets).",
    )
    p.add_argument(
        "--smoke-packages",
        type=int,
        default=2,
        help="How many packages to run in --smoke mode (default: 2).",
    )
    p.add_argument(
        "--kill-stale",
        action="store_true",
        help="If ports are already in use, attempt to kill listeners on 9999/9998 before starting scenario.",
    )
    p.add_argument("--rpc-url", type=str, default="https://fullnode.mainnet.sui.io:443")
    p.add_argument("--simulation-mode", type=str, default="dry-run")
    p.add_argument(
        "--sender",
        type=str,
        default=None,
        help="Optional sender address (public). Only needed for some simulation modes.",
    )
    p.add_argument("--per-package-timeout-seconds", type=float, default=90)
    p.add_argument(
        "--max-plan-attempts",
        type=int,
        default=2,
        help="Max PTB replanning attempts per package.",
    )
    p.add_argument(
        "--max-planning-calls",
        type=int,
        default=None,
        help="Maximum progressive-exposure planning calls per package (omit to use default).",
    )
    p.add_argument(
        "--continue-on-error",
        action="store_true",
        help="Continue even if a package fails.",
    )
    p.add_argument(
        "--resume",
        action="store_true",
        help="Resume an existing output bundle (if supported by server).",
    )
    p.add_argument("--out-response", type=Path, default=Path("results/a2a_smoke_response.json"))
    args = p.parse_args(argv)

    # Default to smoke behavior unless the caller is explicitly doing a larger run.
    if not args.smoke:
        args.smoke = True

    if args.smoke:
        # Clamp to avoid accidental long runs.
        args.samples = max(1, int(args.smoke_packages))
        args.per_package_timeout_seconds = float(args.per_package_timeout_seconds or 120.0)
        if args.per_package_timeout_seconds < 1:
            args.per_package_timeout_seconds = 120.0
        if args.max_plan_attempts > 2:
            # Keep smoke attempts small by default.
            args.max_plan_attempts = 2
        if args.max_planning_calls is None:
            args.max_planning_calls = 10

    if not args.package_ids_file:
        raise SystemExit("--package-ids-file is required")

    package_ids_file = Path(args.package_ids_file)
    if not package_ids_file.exists():
        raise SystemExit(f"package ids file not found: {package_ids_file}")

    # In smoke mode, always truncate the manifest so 'samples' truly means 'packages to attempt'.
    if args.smoke:
        truncated = args.out_response.parent / f"smoke_manifest_{int(time.time())}.txt"
        args.out_response.parent.mkdir(parents=True, exist_ok=True)
        _truncate_manifest(package_ids_file, n=int(args.smoke_packages), out_path=truncated)
        package_ids_file = truncated

    started_pid: int | None = None
    try:
        if args.scenario:
            import subprocess

            # Preflight: ensure ports are available (or kill stale listeners if requested).
            for port in (9999, 9998):
                if _port_is_listening(port):
                    if args.kill_stale:
                        _kill_listeners(port)
                    if _port_is_listening(port):
                        raise SystemExit(
                            f"A2A port already in use: {port} (try stopping stale agents or pass --kill-stale)"
                        )

            args.out_response.parent.mkdir(parents=True, exist_ok=True)
            log_path = args.out_response.parent / "a2a_smoke_scenario.log"
            scenario_root = Path.cwd() / args.scenario
            agentbeats_run = [
                "uv",
                "run",
                "smi-agentbeats-scenario",
                str(scenario_root),
                "--launch-mode",
                "current",
                "--env-file",
                str(args.env_file),
            ]
            proc = subprocess.Popen(
                agentbeats_run,
                cwd=str(Path.cwd()),
                stdout=log_path.open("w"),
                stderr=subprocess.STDOUT,
            )
            started_pid = proc.pid

            # Wait briefly for the green server to come up.
            client = httpx.Client(timeout=2.0)
            deadline = time.time() + 10
            while True:
                try:
                    r = client.get(args.green_url.rstrip("/") + "/.well-known/agent-card.json")
                    if r.status_code == 200:
                        break
                except Exception:
                    pass
                if time.time() > deadline:
                    raise RuntimeError("green agent did not become healthy in time")
                time.sleep(0.5)

        req = _default_request(
            corpus_root=args.corpus_root,
            package_ids_file=str(package_ids_file),
            samples=args.samples,
            timeout_s=args.per_package_timeout_seconds,
            rpc_url=args.rpc_url,
            simulation_mode=args.simulation_mode,
            sender=args.sender,
            max_plan_attempts=args.max_plan_attempts,
            max_planning_calls=args.max_planning_calls,
            continue_on_error=args.continue_on_error,
            resume=args.resume,
        )
        with httpx.Client(timeout=None) as client:
            r = client.post(args.green_url, json=req)
            r.raise_for_status()
            resp = r.json()

        args.out_response.parent.mkdir(parents=True, exist_ok=True)
        args.out_response.write_text(json.dumps(resp, indent=2, sort_keys=True), encoding="utf-8")

        bundle = _extract_bundle(resp)
        metrics = bundle.get("metrics")
        errors = bundle.get("errors")
        artifacts = bundle.get("artifacts")

        print(f"run_id={bundle.get('run_id')} exit_code={bundle.get('exit_code')}")
        print(f"metrics={json.dumps(metrics, sort_keys=True)}")
        print(f"errors_len={len(errors) if isinstance(errors, list) else 'unknown'}")
        if isinstance(artifacts, dict):
            print(f"results_path={artifacts.get('results_path')}")
            print(f"events_path={artifacts.get('events_path')}")
        print(f"response_path={args.out_response}")
    finally:
        if started_pid is not None:
            try:
                import os
                import signal

                os.kill(started_pid, signal.SIGTERM)
            except Exception:
                pass


if __name__ == "__main__":
    main()
