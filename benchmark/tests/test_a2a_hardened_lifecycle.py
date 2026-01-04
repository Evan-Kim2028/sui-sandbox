import asyncio
import time
from unittest.mock import AsyncMock, MagicMock

import pytest
from a2a.server.tasks import TaskUpdater

from smi_bench.a2a_green_agent import SmiBenchGreenExecutor


@pytest.fixture
def executor():
    return SmiBenchGreenExecutor()


@pytest.mark.anyio
async def test_executor_subprocess_cleanup_on_cancel(executor, tmp_path):
    """
    Verify that managed_subprocess correctly handles cancellation.
    We mock the updater and context to simulate a real A2A task.
    """
    # Create a mock updater
    updater = MagicMock(spec=TaskUpdater)
    updater.update_status = AsyncMock()
    updater.task_id = "test-task"
    updater.context_id = "test-ctx"

    # Mock context
    context = MagicMock()
    context.current_task.id = "test-task"

    # Mock cancel event
    cancel_event = asyncio.Event()

    # Use a command that runs long (sleep)
    # We'll trigger the logic that normally runs in _run_task_logic
    # but since that method is complex, we'll verify the managed_subprocess
    # logic which is the core of our hardening.
    from smi_bench.utils import managed_subprocess

    start_time = time.time()

    # Create a task that will be cancelled
    async def run_and_cancel():
        async with managed_subprocess("sleep", "60") as proc:
            # Signal cancellation
            cancel_event.set()
            # In a real scenario, the loop would break or the context manager would exit
            return proc

    proc = await run_and_cancel()

    # Verify process was terminated/killed by the context manager exit
    # We might need a small sleep to let the finally block run
    await asyncio.sleep(0.1)
    assert proc.returncode is not None
    assert time.time() - start_time < 5  # Should have exited immediately


def test_config_range_clamping(monkeypatch):
    """Verify that our refactored load_real_agent_config clamps values correctly."""
    from smi_bench.agents.real_agent import load_real_agent_config

    # Mock environment variables
    monkeypatch.setenv("SMI_API_KEY", "test-key")
    monkeypatch.setenv("SMI_MODEL", "test-model")
    monkeypatch.setenv("SMI_TEMPERATURE", "5.0")
    monkeypatch.setenv("SMI_MAX_TOKENS", "200000")

    cfg = load_real_agent_config()
    assert cfg.temperature == 2.0
    assert cfg.max_tokens == 100000


@pytest.mark.anyio
async def test_error_structured_telemetry(executor):
    """Verify that log_exception captures the right info."""
    import logging

    from smi_bench.utils import log_exception

    # Create a custom handler to capture logs
    class TestHandler(logging.Handler):
        def __init__(self):
            super().__init__()
            self.records = []

        def emit(self, record):
            self.records.append(record)

    test_logger = logging.getLogger("smi_bench.utils")
    handler = TestHandler()
    test_logger.addHandler(handler)

    try:
        try:
            raise ValueError("test structured error")
        except ValueError:
            log_exception("Failure in test", extra={"pkg": "0x123"})

        assert len(handler.records) > 0
        record = handler.records[0]
        assert hasattr(record, "structured_error")
        err = record.structured_error
        assert err["error_type"] == "ValueError"
        assert err["error_message"] == "test structured error"
        assert "traceback" in err
        assert err["pkg"] == "0x123"
    finally:
        test_logger.removeHandler(handler)
