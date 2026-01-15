"""
Idempotent Docker container runner with guaranteed cleanup.

This module provides a Python-based orchestrator for running the smi-bench
Docker container with reliable lifecycle management. It replaces the brittle
shell script approach with proper cleanup guarantees.

Usage:
    from smi_bench.docker_runner import managed_container, wait_for_healthy

    with managed_container("smi-bench:test", ports={"9999/tcp": 9999}) as container:
        if not wait_for_healthy(container):
            raise RuntimeError("Container failed health check")
        # Run your tests...
    # Container is guaranteed to be stopped and removed here

CLI Usage:
    smi-docker-runner --image smi-bench:test --corpus /path/to/corpus
"""

from __future__ import annotations

import argparse
import atexit
import json
import logging
import sys
import time
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import TYPE_CHECKING, Any

from smi_bench.utils import setup_signal_handlers

if TYPE_CHECKING:
    import docker  # type: ignore
    from docker.models.containers import Container  # type: ignore

logger = logging.getLogger(__name__)

CONTAINER_NAME = "smi-bench-runner"
DEFAULT_PORT = 9999
DEFAULT_TIMEOUT = 60.0
DEFAULT_STOP_TIMEOUT = 30


def _get_docker_client() -> docker.DockerClient:
    """Get Docker client, with helpful error message if Docker is unavailable."""
    try:
        import docker  # type: ignore

        return docker.from_env()
    except ImportError as e:
        raise RuntimeError("Docker SDK not installed. Install with: pip install docker") from e
    except Exception as e:
        raise RuntimeError(f"Failed to connect to Docker daemon: {e}. Is Docker running?") from e


def cleanup_existing_container(
    client: docker.DockerClient,
    name: str = CONTAINER_NAME,
) -> bool:
    """
    Remove any existing container with the given name.

    Returns True if a container was removed, False if none existed.
    """
    try:
        container = client.containers.get(name)
        logger.info(f"Removing existing container: {name}")
        container.remove(force=True)
        return True
    except (docker.errors.NotFound, docker.errors.APIError) as e:
        # If it's APIError, it might be already gone or some other issue
        if isinstance(e, docker.errors.APIError):
            logger.debug(f"Attempting to remove container {name} failed: {e}")
        return False


def wait_for_healthy(
    container: Container,
    timeout: float = DEFAULT_TIMEOUT,
    health_url: str = "http://localhost:9999/health",
) -> bool:
    """
    Wait for container health check to pass.

    Args:
        container: Docker container object
        timeout: Maximum seconds to wait
        health_url: URL to check (used for logging only; relies on Docker healthcheck)

    Returns:
        True if container became healthy, False if timeout
    """
    deadline = time.time() + timeout
    last_status = None

    while time.time() < deadline:
        try:
            container.reload()
        except docker.errors.NotFound:
            logger.error("Container disappeared while waiting for health check")
            return False

        state = container.attrs.get("State", {})
        health = state.get("Health", {})
        status = health.get("Status", "unknown")

        if status != last_status:
            logger.info(f"Container health status: {status}")
            last_status = status

        if status == "healthy":
            return True

        if status == "unhealthy":
            # Log the last health check output for debugging
            logs = health.get("Log", [])
            if logs:
                last_log = logs[-1]
                logger.error(
                    f"Health check failed: exit={last_log.get('ExitCode')}, output={last_log.get('Output', '')[:200]}"
                )
            return False

        if state.get("Status") == "exited":
            exit_code = state.get("ExitCode", -1)
            logger.error(f"Container exited unexpectedly with code {exit_code}")
            return False

        time.sleep(0.5)

    logger.error(f"Timeout waiting for container health after {timeout}s")
    return False


def wait_for_port(
    port: int = DEFAULT_PORT,
    host: str = "127.0.0.1",
    timeout: float = 30.0,
) -> bool:
    """Wait for a port to become available (fallback if no Docker healthcheck)."""
    import socket

    deadline = time.time() + timeout
    while time.time() < deadline:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(0.5)
            if s.connect_ex((host, port)) == 0:
                return True
        time.sleep(0.3)
    return False


@contextmanager
def managed_container(
    image: str,
    name: str = CONTAINER_NAME,
    ports: dict[str, int] | None = None,
    volumes: dict[str, dict[str, str]] | None = None,
    environment: dict[str, str] | None = None,
    env_file: Path | None = None,
    user: str = "1000:1000",
    stop_timeout: int = DEFAULT_STOP_TIMEOUT,
    **extra_run_kwargs: Any,
) -> Generator[Container, None, None]:
    """
    Context manager that ensures container cleanup on ANY exit.

    This is the primary API for running smi-bench containers with guaranteed
    cleanup, even if the process is killed or crashes.

    Args:
        image: Docker image name/tag
        name: Container name (used for idempotent cleanup)
        ports: Port mappings, e.g., {"9999/tcp": 9999}
        volumes: Volume mappings, e.g., {"/host/path": {"bind": "/container/path", "mode": "ro"}}
        environment: Environment variables
        env_file: Path to .env file to load
        user: User to run as (default: 1000:1000 for non-root)
        stop_timeout: Seconds to wait for graceful stop before kill
        **extra_run_kwargs: Additional arguments to docker.containers.run()

    Yields:
        Container object

    Example:
        with managed_container("smi-bench:test", ports={"9999/tcp": 9999}) as c:
            wait_for_healthy(c)
            # ... run tests ...
    """
    client = _get_docker_client()

    # Idempotent cleanup: remove any existing container with this name
    cleanup_existing_container(client, name)

    # Load environment from file if specified
    env = dict(environment or {})
    if env_file and env_file.exists():
        try:
            for line in env_file.read_text(encoding="utf-8").splitlines():
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    key, _, value = line.partition("=")
                    env.setdefault(key.strip(), value.strip().strip('"').strip("'"))
        except (OSError, PermissionError) as e:
            logger.warning(f"Failed to read env_file {env_file}: {e}")

    # Default ports if not specified
    if ports is None:
        ports = {"9999/tcp": DEFAULT_PORT}

    logger.info(f"Starting container {name} from image {image}")
    container = client.containers.run(
        image,
        name=name,
        detach=True,
        ports=ports,
        volumes=volumes,
        environment=env or None,
        user=user,
        init=True,  # Use tini for signal handling
        **extra_run_kwargs,
    )

    # Register cleanup for ANY exit path (including kill -9 of parent)
    def cleanup() -> None:
        try:
            container.reload()
            if container.status in ("running", "created", "paused"):
                logger.info(f"Stopping container {name} (timeout={stop_timeout}s)")
                container.stop(timeout=stop_timeout)
            logger.info(f"Removing container {name}")
            container.remove(force=True)
        except Exception as e:
            # NotFound is expected if cleanup already ran
            if "not found" not in str(e).lower():
                logger.debug(f"Cleanup info (may be already removed): {e}")

    atexit.register(cleanup)
    setup_signal_handlers(cleanup)

    try:
        yield container
    finally:
        cleanup()
        try:
            atexit.unregister(cleanup)
        except (TypeError, AttributeError, ValueError) as e:
            logger.debug(f"Failed to unregister cleanup: {e}")


def run_smoke_test(
    image: str,
    corpus_path: Path,
    manifest_path: Path,
    results_path: Path,
    agent: str = "mock-empty",
    samples: int = 1,
    timeout: float = 120.0,
) -> dict[str, Any]:
    """
    Run a complete smoke test and return results.

    This is a high-level convenience function that handles the full lifecycle:
    1. Start container
    2. Wait for healthy
    3. Execute smi-inhabit
    4. Collect results
    5. Stop container

    Returns:
        Dict with keys: success, exit_code, results_file, logs
    """
    results_path.mkdir(parents=True, exist_ok=True)

    volumes = {
        str(corpus_path.resolve()): {"bind": "/app/corpus", "mode": "ro"},
        str(results_path.resolve()): {"bind": "/app/results", "mode": "rw"},
    }

    with managed_container(image, volumes=volumes) as container:
        if not wait_for_healthy(container, timeout=60):
            logs = container.logs().decode("utf-8", errors="replace")
            return {
                "success": False,
                "exit_code": -1,
                "error": "Container failed health check",
                "logs": logs[-5000:],
            }

        # Run the benchmark
        cmd = [
            "smi-inhabit",
            "--corpus-root",
            "/app/corpus",
            "--package-ids-file",
            f"/app/corpus/{manifest_path.name}",
            "--samples",
            str(samples),
            "--agent",
            agent,
            "--simulation-mode",
            "dry-run",
            "--out",
            "/app/results/smoke_test.json",
            "--no-log",
        ]

        logger.info(f"Executing: {' '.join(cmd)}")
        exit_code, output = container.exec_run(cmd, demux=False)

        logs = container.logs().decode("utf-8", errors="replace")
        result_file = results_path / "smoke_test.json"

        return {
            "success": exit_code == 0 and result_file.exists(),
            "exit_code": exit_code,
            "exec_output": output.decode("utf-8", errors="replace") if output else "",
            "results_file": str(result_file) if result_file.exists() else None,
            "logs": logs[-5000:],
        }


def main(argv: list[str] | None = None) -> None:
    """CLI entry point for docker runner."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
    )

    p = argparse.ArgumentParser(
        description="Run smi-bench Docker container with guaranteed cleanup",
    )
    p.add_argument(
        "--image",
        type=str,
        default="smi-bench:test",
        help="Docker image name/tag",
    )
    p.add_argument(
        "--corpus",
        type=Path,
        required=True,
        help="Path to corpus directory",
    )
    p.add_argument(
        "--manifest",
        type=Path,
        required=True,
        help="Path to manifest file (relative to corpus or absolute)",
    )
    p.add_argument(
        "--results",
        type=Path,
        default=Path("results"),
        help="Path to results directory",
    )
    p.add_argument(
        "--agent",
        type=str,
        default="mock-empty",
        help="Agent to use",
    )
    p.add_argument(
        "--samples",
        type=int,
        default=1,
        help="Number of samples to run",
    )
    p.add_argument(
        "--build",
        action="store_true",
        help="Build the Docker image before running",
    )
    args = p.parse_args(argv)

    if args.build:
        import subprocess

        logger.info("Building Docker image...")
        result = subprocess.run(
            ["docker", "build", "-t", args.image, "."],
            check=False,
        )
        if result.returncode != 0:
            logger.error("Docker build failed")
            sys.exit(1)

    result = run_smoke_test(
        image=args.image,
        corpus_path=args.corpus,
        manifest_path=args.manifest,
        results_path=args.results,
        agent=args.agent,
        samples=args.samples,
    )

    print(json.dumps(result, indent=2))
    sys.exit(0 if result["success"] else 1)


if __name__ == "__main__":
    main()
