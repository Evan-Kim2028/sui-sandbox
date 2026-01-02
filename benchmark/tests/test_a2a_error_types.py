"""Tests for A2A protocol error types and mapping.

These tests ensure that exceptions are properly mapped to A2A error types
and that JSON-RPC error responses include the correct information.
"""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest

from smi_bench.a2a_errors import A2AError, ContentTypeNotSupportedError, InvalidConfigError, TaskNotCancelableError


class TestA2AErrorTypes:
    """Test A2A error type definitions."""

    def test_a2a_error_to_dict(self) -> None:
        """A2AError should correctly convert to dict."""
        error = A2AError(code=-32000, message="Test error", data={"key": "value"})
        d = error.to_dict()

        assert d["code"] == -32000
        assert d["message"] == "Test error"
        assert d["data"] == {"key": "value"}

    def test_task_not_cancelable_error(self) -> None:
        """TaskNotCancelableError should have correct properties."""
        error = TaskNotCancelableError(task_id="task-1", current_state="completed")
        d = error.to_dict()

        assert d["code"] == -32001
        assert "task-1" in d["message"]
        assert "completed" in d["message"]
        assert d["data"]["taskId"] == "task-1"
        assert d["data"]["currentState"] == "completed"

    def test_invalid_config_error(self) -> None:
        """InvalidConfigError should have correct properties."""
        error = InvalidConfigError(field="corpus_root", reason="missing")
        d = error.to_dict()

        assert d["code"] == -32602
        assert "corpus_root" in d["message"]
        assert "missing" in d["message"]
        assert d["data"]["field"] == "corpus_root"
        assert d["data"]["reason"] == "missing"

    def test_content_type_not_supported_error(self) -> None:
        """ContentTypeNotSupportedError should have correct properties."""
        error = ContentTypeNotSupportedError(content_type="text/html", supported=["application/json"])
        d = error.to_dict()

        assert d["code"] == -32002
        assert "text/html" in d["message"]
        assert d["data"]["contentType"] == "text/html"
        assert d["data"]["supported"] == ["application/json"]


class TestErrorMapping:
    """Test error mapping in green agent."""

    def test_load_cfg_raises_invalid_config_error(self) -> None:
        """_load_cfg should raise InvalidConfigError for missing fields."""
        from smi_bench.a2a_green_agent import _load_cfg

        # Missing corpus_root
        with pytest.raises(InvalidConfigError) as exc_info:
            _load_cfg({"package_ids_file": "manifest.txt"})

        assert exc_info.value.data["field"] == "corpus_root"

        # Missing package_ids_file
        with pytest.raises(InvalidConfigError) as exc_info:
            _load_cfg({"corpus_root": "/corpus"})

        assert exc_info.value.data["field"] == "package_ids_file"

    @pytest.mark.anyio
    async def test_cancel_raises_task_not_cancelable_error(self) -> None:
        """cancel should raise TaskNotCancelableError for terminal tasks."""
        from a2a.types import TaskState

        from smi_bench.a2a_green_agent import SmiBenchGreenExecutor

        executor = SmiBenchGreenExecutor()
        context = MagicMock()
        task = MagicMock()
        task.id = "task-1"
        task.status = TaskState.completed
        context.current_task = task

        with pytest.raises(TaskNotCancelableError) as exc_info:
            await executor.cancel(context, MagicMock())

        assert exc_info.value.data["taskId"] == "task-1"
        assert exc_info.value.data["currentState"] == "completed"
