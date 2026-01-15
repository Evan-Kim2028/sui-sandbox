"""
Shared pytest fixtures and utilities for benchmark tests.

This module provides:
- Dynamic port allocation for Docker tests
- Common Docker container management utilities
- Shared fixtures for manifest files and test corpus
"""

from __future__ import annotations

import json
import socket
import subprocess
from collections.abc import Generator
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Port Management
# ---------------------------------------------------------------------------


def find_free_port(start: int = 10000, end: int = 20000) -> int:
    """
    Find an available port in the given range.

    Uses socket binding to ensure the port is actually free,
    avoiding race conditions with Docker port allocation.
    """
    for port in range(start, end):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                s.bind(("127.0.0.1", port))
                return port
        except OSError:
            continue
    raise RuntimeError(f"No free port found in range {start}-{end}")


def is_port_in_use(port: int) -> bool:
    """Check if a port is currently in use."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        return s.connect_ex(("127.0.0.1", port)) == 0


# ---------------------------------------------------------------------------
# Docker Utilities
# ---------------------------------------------------------------------------


def is_docker_available() -> bool:
    """Check if Docker is available and running."""
    try:
        result = subprocess.run(
            ["docker", "info"],
            check=False, capture_output=True,
            timeout=10,
        )
        return result.returncode == 0
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return False


def is_docker_port_allocated(port: int) -> bool:
    """Check if a Docker container is using the specified port."""
    try:
        res = subprocess.run(
            ["docker", "ps", "-q", "--filter", f"publish={port}"],
            check=False, capture_output=True,
            text=True,
            timeout=10,
        )
        return bool(res.stdout.strip())
    except Exception:
        return False


def get_container_by_name(container_name: str) -> dict | None:
    """Get container info by name, returns None if not found."""
    try:
        result = subprocess.run(
            ["docker", "inspect", container_name],
            check=False, capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode == 0:
            containers = json.loads(result.stdout)
            return containers[0] if containers else None
    except (subprocess.CalledProcessError, json.JSONDecodeError, subprocess.TimeoutExpired):
        pass
    return None


def cleanup_container(container_name: str) -> None:
    """Stop and remove a container if it exists."""
    try:
        subprocess.run(
            ["docker", "stop", container_name],
            check=False, capture_output=True,
            timeout=30,
        )
    except subprocess.TimeoutExpired:
        subprocess.run(["docker", "kill", container_name], check=False, capture_output=True)

    subprocess.run(
        ["docker", "rm", "-f", container_name],
        check=False, capture_output=True,
    )


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def free_port() -> int:
    """Fixture providing a dynamically allocated free port."""
    return find_free_port()


@pytest.fixture
def manifest_file(tmp_path: Path) -> str:
    """Create a temporary manifest file for testing."""
    manifest = tmp_path / "manifest.txt"
    manifest.write_text("0x1\n")
    return str(manifest)


@pytest.fixture
def real_manifest(tmp_path: Path) -> str:
    """Alias for manifest_file (used in some test files)."""
    p = tmp_path / "ids.txt"
    p.write_text("0x1\n")
    return str(p)


@pytest.fixture(scope="function")
def docker_cleanup() -> Generator[list[str], None, None]:
    """
    Fixture that tracks containers created during a test and cleans them up.

    Usage:
        def test_something(docker_cleanup):
            docker_cleanup.append("my-container-name")
            # ... test code ...
        # Container automatically cleaned up after test
    """
    containers: list[str] = []
    yield containers
    for name in containers:
        cleanup_container(name)


# ---------------------------------------------------------------------------
# Skip Conditions
# ---------------------------------------------------------------------------

skip_if_no_docker = pytest.mark.skipif(not is_docker_available(), reason="Docker not available")
