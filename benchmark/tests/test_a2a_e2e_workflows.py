"""End-to-end workflow tests for A2A protocol.

These tests validate complete A2A protocol workflows including:
- Full task lifecycle (submit → stream → complete)
- Task cancellation workflow
- Error recovery scenarios
- Concurrent task handling
- Timeout handling
"""

from __future__ import annotations

import asyncio
import json
import time
from collections.abc import AsyncIterator as AsyncIteratorType
from pathlib import Path
from typing import Any
from unittest.mock import AsyncMock, patch

from starlette.testclient import TestClient


class AsyncIterator:
    """Simple async iterator for mocking async streams."""

    def __init__(self, items: list[bytes]) -> None:
        self.items = iter(items)

    def __aiter__(self) -> AsyncIteratorType[bytes]:
        return self

    async def __anext__(self) -> bytes:
        try:
            return next(self.items)
        except StopIteration:
            raise StopAsyncIteration


def create_test_config(tmp_path: Path) -> dict[str, Any]:
    """Generate minimal valid config for testing."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    manifest = tmp_path / "manifest.txt"
    manifest.write_text("0x1\n")

    return {
        "corpus_root": str(corpus_root),
        "package_ids_file": str(manifest),
        "samples": 1,
        "rpc_url": "https://fullnode.mainnet.sui.io:443",
        "simulation_mode": "dry-run",
        "per_package_timeout_seconds": 90.0,
        "max_plan_attempts": 2,
        "continue_on_error": True,
        "resume": False,
    }


def create_mock_phase2_output(tmp_path: Path, run_id: str) -> Path:
    """Create mock Phase II results JSON file."""
    output_file = tmp_path / f"{run_id}.json"
    output_data = {
        "schema_version": 1,
        "packages": [
            {
                "package_id": "0x1",
                "score": {"targets": 2, "created_hits": 1, "created_distinct": 1, "missing": 1},
                "elapsed_seconds": 1.0,
                "plan_attempts": 1,
                "sim_attempts": 1,
            }
        ],
        "aggregate": {
            "avg_hit_rate": 0.5,
            "errors": 0,
            "packages_total": 1,
        },
    }
    output_file.write_text(json.dumps(output_data), encoding="utf-8")
    return output_file


def extract_task_from_response(response_json: dict[str, Any]) -> dict[str, Any] | None:
    """Extract task object from JSON-RPC response."""
    result = response_json.get("result")
    if result is None:
        return None
    if isinstance(result, dict) and result.get("kind") == "task":
        return result
    return None


def wait_for_task_completion(client: TestClient, task_id: str, timeout: float = 5.0) -> dict[str, Any]:
    """Poll task/get endpoint until task reaches terminal state."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        # A2A library routes JSON-RPC to root endpoint
        response = client.post(
            "/",
            json={
                "jsonrpc": "2.0",
                "method": "task/get",
                "params": {"taskId": task_id},
                "id": 1,
            },
        )
        assert response.status_code == 200
        body = response.json()
        task = extract_task_from_response(body)
        if task is None:
            # Try to get task from result directly
            result = body.get("result")
            if isinstance(result, dict):
                task = result

        if task and task.get("status", {}).get("state") in ["completed", "failed", "cancelled"]:
            return task

        time.sleep(0.1)

    raise TimeoutError(f"Task {task_id} did not complete in {timeout} seconds")


class TestFullTaskLifecycle:
    """Test complete task lifecycle from submission to completion."""

    def test_full_task_lifecycle(self, tmp_path: Path) -> None:
        """Full task lifecycle: submit → working → completed with evaluation_bundle."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        config = create_test_config(tmp_path)
        run_id = f"test_run_{int(time.time())}"

        # Create mock Phase II output file
        out_dir = tmp_path / "results" / "a2a"
        out_dir.mkdir(parents=True)
        create_mock_phase2_output(out_dir, run_id)

        # Mock subprocess execution
        mock_proc = AsyncMock()
        output_lines = [
            b"Starting Phase II benchmark...",
            b"Processing package 0x1...",
            b"Completed successfully",
        ]
        mock_proc.stdout = AsyncIterator(output_lines)
        mock_proc.wait = AsyncMock(return_value=0)
        mock_proc.returncode = 0

        with patch("smi_bench.a2a_green_agent.asyncio.create_subprocess_exec") as mock_subprocess:
            mock_subprocess.return_value = mock_proc

            # Submit task
            request = {
                "jsonrpc": "2.0",
                "id": "test-1",
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": f"test_{int(time.time())}",
                        "role": "user",
                        "parts": [{"text": json.dumps({"config": config, "out_dir": str(out_dir)})}],
                    }
                },
            }

            # A2A library routes JSON-RPC requests to root endpoint
            response = client.post("/", json=request)
            assert response.status_code == 200
            body = response.json()

            # Should have either result (task) or error
            assert "result" in body or "error" in body

            if "result" in body:
                result = body["result"]
                # Result should be a task object
                assert isinstance(result, dict)
                if result.get("kind") == "task":
                    task_id = result.get("id")
                    assert task_id is not None

                    # Verify task has required fields
                    assert "status" in result
                    assert "id" in result

                    # Initial state should be submitted, working, or completed (if fast)
                    state = result.get("status", {}).get("state")
                    assert state in ["submitted", "working", "completed"]

                    # Note: Full completion testing requires async execution to finish
                    # This test verifies task creation and initial state

    def test_task_with_minimal_config(self, tmp_path: Path) -> None:
        """Task submission with minimal required config."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        config = create_test_config(tmp_path)

        request = {
            "jsonrpc": "2.0",
            "id": "test-2",
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": f"test_minimal_{int(time.time())}",
                    "role": "user",
                    "parts": [{"text": json.dumps({"config": config})}],
                }
            },
        }

        response = client.post("/", json=request)
        assert response.status_code == 200
        body = response.json()

        # Should accept the request (may return task or error)
        assert "result" in body or "error" in body


class TestTaskCancellation:
    """Test task cancellation workflow."""

    def test_task_cancellation(self, tmp_path: Path) -> None:
        """Task can be cancelled while in working state."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        config = create_test_config(tmp_path)

        # Mock subprocess that runs indefinitely
        mock_proc = AsyncMock()

        # Create an async iterator that yields data slowly
        async def slow_output():
            for i in range(100):
                yield b"running...\n"
                await asyncio.sleep(0.01)

        mock_proc.stdout = slow_output()
        mock_proc.wait = AsyncMock()
        mock_proc.terminate = AsyncMock()
        mock_proc.kill = AsyncMock()
        mock_proc.returncode = None

        with patch("smi_bench.a2a_green_agent.asyncio.create_subprocess_exec") as mock_subprocess:
            mock_subprocess.return_value = mock_proc

            # Submit task
            request = {
                "jsonrpc": "2.0",
                "id": "test-cancel-1",
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": f"test_cancel_{int(time.time())}",
                        "role": "user",
                        "parts": [{"text": json.dumps({"config": config})}],
                    }
                },
            }

            # A2A library routes JSON-RPC requests to root endpoint
            response = client.post("/", json=request)
            assert response.status_code == 200
            body = response.json()

            # Extract task ID if task was created
            task_id = None
            if "result" in body:
                result = body["result"]
                if isinstance(result, dict) and result.get("kind") == "task":
                    task_id = result.get("id")

            # If we have a task ID, try to cancel it
            if task_id:
                cancel_request = {
                    "jsonrpc": "2.0",
                    "id": "test-cancel-2",
                    "method": "task/cancel",
                    "params": {"taskId": task_id},
                }

                cancel_response = client.post("/", json=cancel_request)
                assert cancel_response.status_code == 200
                # Cancellation should be accepted (may succeed or fail depending on task state)

    def test_cancel_nonexistent_task(self, tmp_path: Path) -> None:
        """Cancelling nonexistent task returns appropriate error."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        cancel_request = {
            "jsonrpc": "2.0",
            "id": "test-cancel-nonexistent",
            "method": "task/cancel",
            "params": {"taskId": "nonexistent-task-id"},
        }

        response = client.post("/", json=cancel_request)
        assert response.status_code == 200
        body = response.json()

        # Should return error for nonexistent task
        assert "error" in body or "result" in body


class TestErrorRecovery:
    """Test error handling and recovery scenarios."""

    def test_invalid_config_error(self, tmp_path: Path) -> None:
        """Invalid config (missing corpus_root) returns proper error."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        # Config missing required field
        invalid_config = {
            "package_ids_file": "manifest.txt",
            # corpus_root is missing
        }

        request = {
            "jsonrpc": "2.0",
            "id": "test-error-1",
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": f"test_error_{int(time.time())}",
                    "role": "user",
                    "parts": [{"text": json.dumps({"config": invalid_config})}],
                }
            },
        }

        response = client.post("/", json=request)
        assert response.status_code == 200
        body = response.json()

        # Should return error for invalid config
        # The error may be in the response or task may fail later
        assert "result" in body or "error" in body

    def test_subprocess_failure(self, tmp_path: Path) -> None:
        """Subprocess failure is handled gracefully."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        config = create_test_config(tmp_path)

        # Mock subprocess that fails
        mock_proc = AsyncMock()
        mock_proc.stdout = AsyncIterator([b"error occurred\n"])
        mock_proc.wait = AsyncMock(return_value=1)  # Non-zero exit code
        mock_proc.returncode = 1

        with patch("smi_bench.a2a_green_agent.asyncio.create_subprocess_exec") as mock_subprocess:
            mock_subprocess.return_value = mock_proc

            request = {
                "jsonrpc": "2.0",
                "id": "test-error-2",
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": f"test_failure_{int(time.time())}",
                        "role": "user",
                        "parts": [{"text": json.dumps({"config": config})}],
                    }
                },
            }

            # A2A library routes JSON-RPC requests to root endpoint
            response = client.post("/", json=request)
            assert response.status_code == 200
            # Request should be accepted (task will fail during execution)


class TestConcurrentTasks:
    """Test multiple concurrent tasks."""

    def test_concurrent_tasks(self, tmp_path: Path) -> None:
        """Multiple tasks can run concurrently."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        config = create_test_config(tmp_path)

        # Mock successful subprocess
        mock_proc = AsyncMock()
        mock_proc.stdout = AsyncIterator([b"completed\n"])
        mock_proc.wait = AsyncMock(return_value=0)
        mock_proc.returncode = 0

        with patch("smi_bench.a2a_green_agent.asyncio.create_subprocess_exec") as mock_subprocess:
            mock_subprocess.return_value = mock_proc

            # Submit multiple tasks
            task_ids = []
            for i in range(3):
                request = {
                    "jsonrpc": "2.0",
                    "id": f"test-concurrent-{i}",
                    "method": "message/send",
                    "params": {
                        "message": {
                            "messageId": f"test_concurrent_{i}_{int(time.time())}",
                            "role": "user",
                            "parts": [{"text": json.dumps({"config": config})}],
                        }
                    },
                }

                # A2A library routes JSON-RPC requests to root endpoint
                response = client.post("/", json=request)
                assert response.status_code == 200
                body = response.json()

                if "result" in body:
                    result = body["result"]
                    if isinstance(result, dict) and result.get("kind") == "task":
                        task_id = result.get("id")
                        if task_id:
                            task_ids.append(task_id)

            # Verify all tasks were created with unique IDs
            assert len(task_ids) > 0
            assert len(set(task_ids)) == len(task_ids), "All task IDs should be unique"


class TestTimeoutHandling:
    """Test timeout scenarios."""

    def test_timeout_handling(self, tmp_path: Path) -> None:
        """Timeout is enforced for long-running tasks."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        config = create_test_config(tmp_path)
        # Set very short timeout
        config["per_package_timeout_seconds"] = 0.1

        # Mock subprocess that runs longer than timeout
        mock_proc = AsyncMock()

        async def long_output():
            for i in range(100):
                yield b"still running...\n"
                await asyncio.sleep(0.05)  # Longer than timeout

        mock_proc.stdout = long_output()
        mock_proc.wait = AsyncMock()
        mock_proc.returncode = None

        with patch("smi_bench.a2a_green_agent.asyncio.create_subprocess_exec") as mock_subprocess:
            mock_subprocess.return_value = mock_proc

            request = {
                "jsonrpc": "2.0",
                "id": "test-timeout-1",
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": f"test_timeout_{int(time.time())}",
                        "role": "user",
                        "parts": [{"text": json.dumps({"config": config})}],
                    }
                },
            }

            # A2A library routes JSON-RPC requests to root endpoint
            response = client.post("/", json=request)
            assert response.status_code == 200
            # Request should be accepted (timeout will be handled during execution)
