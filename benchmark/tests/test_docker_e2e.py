"""
Docker E2E tests for SMI Bench.

These tests spin up the full service stack using Docker Compose and run
black-box integration tests against the HTTP API.
"""

import json
import subprocess
import time
from pathlib import Path
from typing import Generator

import httpx
import pytest

# Constants
SERVICE_URL = "http://localhost:9999"
ROOT_DIR = Path(__file__).resolve().parents[2]  # benchmark/tests/ -> benchmark/ -> root/


def is_docker_available() -> bool:
    try:
        subprocess.run(["docker", "--version"], capture_output=True, check=True)
        return True
    except (FileNotFoundError, subprocess.CalledProcessError):
        return False


@pytest.fixture(scope="module")
def docker_service() -> Generator[None, None, None]:
    """
    Start the smi-bench service using Docker Compose.
    Yields when healthy, tears down after.
    """
    if not is_docker_available():
        pytest.skip("Docker not available")

    # Ensure we are using the root docker-compose.yml
    compose_file = ROOT_DIR / "docker-compose.yml"
    if not compose_file.exists():
        pytest.fail(f"docker-compose.yml not found at {compose_file}")

    # 1. Cleanup & Build & Start
    # Ensure any previous run is cleaned up to avoid port/name conflicts
    subprocess.run(
        ["docker", "compose", "-f", str(compose_file), "down", "--remove-orphans"], check=False, capture_output=True
    )

    # If port 9999 is already allocated, skip docker e2e tests in this environment.
    # Note: binding can still fail if a non-docker process is using the port.
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        if s.connect_ex(("127.0.0.1", 9999)) == 0:
            pytest.skip("Port 9999 already in use; skip docker e2e")

    # Use --wait to let Docker wait for healthcheck defined in compose
    try:
        subprocess.run(
            ["docker", "compose", "-f", str(compose_file), "up", "-d", "--wait", "smi-bench"],
            check=True,
            capture_output=True,
            timeout=300,  # 5 minutes for build + healthcheck in CI
        )
    except subprocess.CalledProcessError as e:
        pytest.fail(f"Failed to start docker service:\n{e.stderr.decode()}")
    except subprocess.TimeoutExpired:
        subprocess.run(["docker", "compose", "-f", str(compose_file), "logs", "smi-bench"])
        subprocess.run(["docker", "compose", "-f", str(compose_file), "down"])
        pytest.fail("Docker service startup timed out")

    yield

    # 2. Teardown
    subprocess.run(["docker", "compose", "-f", str(compose_file), "down"], check=False, capture_output=True)


@pytest.mark.integration
@pytest.mark.xdist_group(name="docker_e2e")
def test_service_health(docker_service: None) -> None:
    """Verify service is up and responding to health checks."""
    resp = httpx.get(f"{SERVICE_URL}/health")
    assert resp.status_code == 200
    assert resp.json().get("status") == "ok"


@pytest.mark.integration
@pytest.mark.xdist_group(name="docker_e2e")
def test_agent_card(docker_service: None) -> None:
    """Verify agent card is served."""
    resp = httpx.get(f"{SERVICE_URL}/.well-known/agent-card.json")
    assert resp.status_code == 200
    data = resp.json()
    assert "name" in data
    assert "protocolVersion" in data


@pytest.mark.integration
@pytest.mark.xdist_group(name="docker_e2e")
@pytest.mark.xfail(reason="Docker service interaction issue in test environment")
def test_task_submission_cycle(docker_service: None) -> None:
    """
    Submit a build-only task and wait for completion.
    Uses 'mock-empty' agent to avoid LLM costs/secrets.
    """
    # Use build-only to avoid RPC preflight and simulation overhead
    task_config = {
        "corpus_root": "/app/corpus",
        "package_ids_file": "/app/corpus/manifest.txt",
        "agent": "mock-empty",
        "samples": 1,
        "simulation_mode": "build-only",
        "run_id": f"e2e_test_{int(time.time())}",
        "checkpoint_every": 1,
        "rpc_url": "http://localhost:9999/",  # Mock local RPC to satisfy preflight if any
        "sender": "0x0000000000000000000000000000000000000000000000000000000000000001",
    }

    payload = {
        "jsonrpc": "2.0",
        "id": "e2e-1",
        "method": "message/send",
        "params": {
            "message": {
                "messageId": f"msg_{int(time.time())}",
                "role": "user",
                "parts": [{"text": json.dumps({"config": task_config, "out_dir": "/app/results"})}],
            }
        },
    }

    # 2. Submit
    resp = httpx.post(SERVICE_URL, json=payload, timeout=10.0)
    assert resp.status_code == 200
    data = resp.json()

    assert "result" in data
    task = data["result"]
    assert task["kind"] == "task"
    task_id = task["id"]

    # 3. Poll for Completion
    deadline = time.time() + 60
    final_status = None

    while time.time() < deadline:
        try:
            poll_resp = httpx.post(
                SERVICE_URL, json={"jsonrpc": "2.0", "method": "task/get", "params": {"taskId": task_id}, "id": 1}
            )
            if poll_resp.status_code == 200:
                task_info = poll_resp.json().get("result")
                if task_info:
                    state = task_info.get("status", {}).get("state")
                    if state in ["completed", "failed", "cancelled"]:
                        final_status = state
                        break
        except httpx.RequestError:
            pass  # Transient connection issue
        time.sleep(1)

    assert final_status == "completed", f"Task did not complete. Last state: {final_status}"
