from __future__ import annotations

import argparse
import logging
import socket
import subprocess
from pathlib import Path

import httpx

from smi_bench.constants import DEFAULT_RPC_URL

logger = logging.getLogger(__name__)


def _check_path_exists(label: str, path: Path) -> None:
    if not path.exists():
        raise SystemExit(f"missing: {label}={path}")


def _check_rpc_reachable(rpc_url: str, *, timeout_s: float) -> None:
    try:
        with httpx.Client(timeout=timeout_s) as client:
            r = client.post(rpc_url, json={"jsonrpc": "2.0", "id": 1, "method": "rpc.discover", "params": []})
            # Some endpoints may not implement discover; any HTTP response is enough.
            _ = r.status_code
    except (httpx.HTTPError, httpx.TimeoutException, OSError) as e:
        raise SystemExit(f"rpc_unreachable: {rpc_url} ({type(e).__name__}: {e})")


def _is_listening(host: str, port: int) -> bool:
    try:
        with socket.create_connection((host, port), timeout=0.5):
            return True
    except (TimeoutError, OSError):
        return False


def _is_placeholder_rpc_url(rpc_url: str) -> bool:
    # Unit tests and offline checks often use a non-resolvable placeholder host.
    # Treat these as "skip network check".
    return rpc_url.strip() in {"https://test.rpc", "http://test.rpc"}


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="Preflight checks before a full Phase II run")
    p.add_argument(
        "--corpus-root",
        type=Path,
        required=True,
        help="Path to sui-packages/packages/mainnet_most_used",
    )
    p.add_argument(
        "--rust-binary",
        type=Path,
        default=Path("../target/release/smi_tx_sim"),
        help="Path to smi_tx_sim (relative to benchmark/)",
    )
    p.add_argument("--rpc-url", type=str, default=DEFAULT_RPC_URL)
    p.add_argument(
        "--green-url",
        type=str,
        default="http://127.0.0.1:9999/",
        help="Green agent A2A endpoint",
    )
    p.add_argument(
        "--scenario",
        type=str,
        default=None,
        help="Scenario directory to start servers if not already listening",
    )
    p.add_argument(
        "--package-ids-file",
        type=str,
        default="manifests/standard_phase2_no_framework.txt",
        help="Package id list (manifest) relative to benchmark/",
    )
    p.add_argument("--samples", type=int, default=1)
    p.add_argument("--per-package-timeout-seconds", type=float, default=90)
    p.add_argument("--timeout-seconds", type=float, default=5.0)
    args = p.parse_args(argv)

    _check_path_exists("corpus_root", args.corpus_root)
    _check_path_exists("rust_binary", args.rust_binary)

    # Preflight is used both in real runs and in offline tests.
    # If a scenario is provided, we may be running in a test environment without network.
    if not args.scenario and not _is_placeholder_rpc_url(args.rpc_url):
        _check_rpc_reachable(args.rpc_url, timeout_s=args.timeout_seconds)

    if args.scenario and not _is_listening("127.0.0.1", 9999):
        subprocess.Popen(
            ["uv", "run", "smi-agentbeats-scenario", args.scenario, "--launch-mode", "current"],
            cwd=str(Path.cwd()),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    # Smoke request via existing smoke tool (which also validates bundle shape).
    cmd = [
        "uv",
        "run",
        "smi-a2a-smoke",
        "--green-url",
        args.green_url,
        "--corpus-root",
        str(args.corpus_root),
        "--package-ids-file",
        args.package_ids_file,
        "--samples",
        str(args.samples),
        "--per-package-timeout-seconds",
        str(args.per_package_timeout_seconds),
        "--rpc-url",
        args.rpc_url,
    ]
    if args.scenario:
        cmd.extend(["--scenario", args.scenario])
    subprocess.run(cmd, check=True)

    logger.info("ready_for_full_run")


if __name__ == "__main__":
    main()
