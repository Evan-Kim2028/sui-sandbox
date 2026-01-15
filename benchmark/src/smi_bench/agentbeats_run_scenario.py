from __future__ import annotations

import argparse
import sys
from pathlib import Path

from smi_bench.env import load_dotenv


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="Run an AgentBeats scenario from the local repo")
    p.add_argument("scenario_root", type=Path, help="Path to scenario directory (containing scenario.toml)")
    p.add_argument("--launch-mode", choices=["tmux", "separate", "current"], default="current")
    p.add_argument("--backend", type=str, default=None, help="Backend URL (optional; starts battle if set)")
    p.add_argument("--frontend", type=str, default=None, help="Frontend URL (optional; starts battle if set)")
    p.add_argument(
        "--status",
        action="store_true",
        help="Print whether the scenario's agent ports are listening and exit",
    )
    p.add_argument(
        "--kill",
        action="store_true",
        help="Kill the scenario manager process for this scenario (best-effort).",
    )
    p.add_argument(
        "--env-file",
        type=Path,
        default=Path(".env"),
        help="Dotenv file to load for agent subprocesses (process env still wins).",
    )
    args = p.parse_args(argv)

    scenario_root = args.scenario_root.resolve()
    if not (scenario_root / "scenario.toml").exists():
        raise SystemExit(f"scenario.toml not found: {scenario_root / 'scenario.toml'}")

    pid_file = scenario_root / ".scenario_pids.json"

    if args.status:
        import shutil
        import socket
        import subprocess

        def is_listening(host: str, port: int) -> bool:
            try:
                with socket.create_connection((host, port), timeout=0.5):
                    return True
            except (TimeoutError, OSError):
                return False

        def get_pid_on_port(port: int) -> str | None:
            if not shutil.which("lsof"):
                return "unknown (lsof missing)"
            try:
                lines = (
                    subprocess.check_output(
                        [
                            "lsof",
                            "-nP",
                            f"-iTCP:{port}",
                            "-sTCP:LISTEN",
                            "-t",
                            f"-i:{port}",
                        ],
                        stderr=subprocess.DEVNULL,
                    )
                    .decode()
                    .strip()
                    .split("\n")
                )
                if lines:
                    return lines[0]
                else:
                    return None
            except (subprocess.CalledProcessError, FileNotFoundError, OSError):
                return None

        # Default ports for scenario_smi
        for name, port in [("green", 9999), ("purple", 9998)]:
            listening = is_listening("127.0.0.1", port)
            pid = get_pid_on_port(port) if listening else "N/A"
            print(f"{name}_port_{port}_listening={listening} pid={pid}")

        # Check credentials
        env_path = args.env_file
        print(f"env_file={env_path} exists={env_path.exists()}")
        if env_path.exists():
            env = load_dotenv(env_path)
            for key in ["OPENROUTER_API_KEY", "SMI_API_KEY"]:
                val = env.get(key)
                masked = f"{val[:6]}...{val[-4:]}" if val and len(val) > 10 else ("set" if val else "MISSING")
                print(f"credential_{key}={masked}")
        return

    if args.kill:
        if not pid_file.exists():
            print(f"pid_file_missing={pid_file}")
            return

        import json
        import os
        import shutil
        import signal
        import subprocess

        try:
            data = json.loads(pid_file.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError) as e:
            print(f"pid_file_unreadable={pid_file}: {e}")
            return

        # ScenarioManager does not expose child process handles; kill is best-effort.
        # We at least stop the scenario manager process (which in turn should stop children).
        pids = [data.get("scenario_manager_pid")]
        for pid in [p for p in pids if isinstance(p, int) and p > 0]:
            try:
                print(f"terminating_pid={pid}")
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                print(f"pid_not_found={pid}")
            except Exception as e:
                print(f"kill_error={e}")

        # Best-effort: also kill anything on the default ports if we are on a system with lsof
        if shutil.which("lsof"):
            for port in [9999, 9998]:
                try:
                    pids_on_port = (
                        subprocess.check_output(
                            ["lsof", "-t", f"-i:{port}"],
                            stderr=subprocess.DEVNULL,
                        )
                        .decode()
                        .strip()
                        .split("\n")
                    )
                    for p in pids_on_port:
                        if p:
                            print(f"terminating_port_process={p}_on_{port}")
                            os.kill(int(p), signal.SIGTERM)
                except (OSError, ValueError, subprocess.CalledProcessError):
                    pass

        if pid_file.exists():
            pid_file.unlink()
        return

    # The upstream `agentbeats load_scenario` CLI resolves scenario_root relative to its
    # own installed location (site-packages). To run local scenarios, we must bypass
    # that and use ScenarioManager directly.
    from agentbeats.utils.deploy.scenario_manager import ScenarioManager

    # Ensure common provider keys (e.g. OPENROUTER_API_KEY) are available to subprocesses.
    # ScenarioManager launches agents via `subprocess.Popen(..., shell=True)`.
    # Ensure we load `.env` from the benchmark project dir so the launched agents
    # inherit keys (OPENROUTER_API_KEY, etc.).
    env = load_dotenv(args.env_file)
    # IMPORTANT: override, don't setdefault.
    # Otherwise, an already-exported SMI_MODEL/SMI_* in the parent shell can leak into
    # subprocesses and cause confusing model switches across runs.
    import os

    for k, v in env.items():
        if k.endswith("_API_KEY") or k.startswith("SMI_"):
            os.environ[k] = v

    manager = ScenarioManager(scenario_root=scenario_root, project_dir=Path.cwd())

    # Patch the loaded agent commands to launch the repo's A2A servers instead of
    # `agentbeats run_agent ...`.
    for agent in manager.agents:
        if agent.name == "smi-bench-green":
            agent.get_command = lambda a=agent: f"cd {Path.cwd()} && uv run smi-a2a-green --host {a.agent_host} --port {a.agent_port} --card-url http://{a.agent_host}:{a.agent_port}/"
        elif agent.name == "smi-bench-purple":
            agent.get_command = lambda a=agent: f"cd {Path.cwd()} && uv run smi-a2a-purple --host {a.agent_host} --port {a.agent_port} --card-url http://{a.agent_host}:{a.agent_port}/"

    manager.load_scenario(mode=args.launch_mode)

    # Best-effort: persist PID for later --kill.
    try:
        import json

        pids = {
            "scenario_manager_pid": os.getpid(),
        }
        pid_file.write_text(json.dumps(pids, indent=2, sort_keys=True), encoding="utf-8")
    except OSError as e:
        print(f"Warning: Failed to save PID file {pid_file}: {e}", file=sys.stderr)

    if args.backend and args.frontend:
        manager.start_battle(backend_url=args.backend, frontend_url=args.frontend)


if __name__ == "__main__":
    main()
