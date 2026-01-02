import asyncio
from unittest.mock import AsyncMock, MagicMock

import pytest
from a2a.server.agent_execution import RequestContext
from a2a.server.events import EventQueue
from a2a.types import Task, TaskState

from smi_bench.a2a_green_agent import SmiBenchGreenExecutor


@pytest.mark.anyio
async def test_cancellation_terminates_process():
    """Verify that canceling a task signals the process to terminate."""
    executor = SmiBenchGreenExecutor()

    # Mock task and context
    task_id = "test_task_123"
    mock_task = MagicMock(spec=Task)
    mock_task.id = task_id
    mock_task.status = TaskState.working
    mock_task.context_id = "ctx_123"

    mock_context = MagicMock(spec=RequestContext)
    mock_context.current_task = mock_task

    # Mock process
    mock_proc = AsyncMock(spec=asyncio.subprocess.Process)
    mock_proc.returncode = None
    mock_proc.wait = AsyncMock(return_value=0)

    # Manually register the process in the executor
    executor._task_processes[task_id] = mock_proc
    mock_cancel_event = asyncio.Event()
    executor._task_cancel_events[task_id] = mock_cancel_event

    mock_event_queue = MagicMock(spec=EventQueue)

    # Execute cancellation
    await executor.cancel(mock_context, mock_event_queue)

    # Verify:
    # 1. Cancel event was set
    assert mock_cancel_event.is_set()
    # 2. proc.terminate() was called
    mock_proc.terminate.assert_called_once()
    # 3. proc.wait() was called
    mock_proc.wait.assert_called()


@pytest.mark.anyio
async def test_cancellation_handles_timeout_and_kills():
    """Verify that if terminate fails (timeout), kill is called."""
    executor = SmiBenchGreenExecutor()
    task_id = "test_task_456"

    mock_task = MagicMock(spec=Task)
    mock_task.id = task_id
    mock_task.status = TaskState.working
    mock_task.context_id = "ctx_456"

    mock_context = MagicMock(spec=RequestContext)
    mock_context.current_task = mock_task

    mock_proc = AsyncMock(spec=asyncio.subprocess.Process)
    mock_proc.returncode = None

    # Simulate wait_for timeout by making wait() take a long time
    async def slow_wait():
        await asyncio.sleep(10)
        return 0

    mock_proc.wait = AsyncMock(side_effect=slow_wait)

    executor._task_processes[task_id] = mock_proc
    executor._task_cancel_events[task_id] = asyncio.Event()

    mock_event_queue = MagicMock(spec=EventQueue)

    # We'll monkeypatch asyncio.wait_for to raise TimeoutError immediately
    import asyncio as aio

    async def mock_timeout_wait_for(fut, timeout):
        if timeout == 5.0:  # Match the code's timeout
            raise aio.TimeoutError()
        return await aio.wait_for(fut, timeout)

    import smi_bench.a2a_green_agent

    with pytest.MonkeyPatch().context() as mp:
        mp.setattr(smi_bench.a2a_green_agent.asyncio, "wait_for", mock_timeout_wait_for)
        await executor.cancel(mock_context, mock_event_queue)

    # Verify kill was called after terminate timed out
    mock_proc.terminate.assert_called_once()
    mock_proc.kill.assert_called_once()
