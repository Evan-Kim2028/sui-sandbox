"""A2A protocol error type definitions.

This module defines standard error types for the A2A protocol implementation,
enabling consistent error reporting with proper JSON-RPC error codes and data.
"""

from __future__ import annotations

from typing import Any


class A2AError(Exception):
    """Base class for A2A protocol errors."""

    def __init__(self, code: int, message: str, data: dict[str, Any] | None = None):
        self.code = code
        self.message = message
        self.data = data or {}
        super().__init__(self.message)

    def to_dict(self) -> dict[str, Any]:
        """Convert error to JSON-RPC error dictionary."""
        return {
            "code": self.code,
            "message": self.message,
            "data": self.data,
        }


class TaskNotCancelableError(A2AError):
    """Task cannot be cancelled (already in terminal state)."""

    def __init__(self, task_id: str, current_state: str):
        super().__init__(
            code=-32001,  # A2A custom error code
            message=f"Task {task_id} cannot be cancelled (current state: {current_state})",
            data={"taskId": task_id, "currentState": current_state},
        )


class InvalidConfigError(A2AError):
    """Invalid configuration provided."""

    def __init__(self, field: str, reason: str):
        super().__init__(
            code=-32602,  # Invalid params (standard JSON-RPC)
            message=f"Invalid config: {field} - {reason}",
            data={"field": field, "reason": reason},
        )


class ContentTypeNotSupportedError(A2AError):
    """Content type not supported."""

    def __init__(self, content_type: str, supported: list[str]):
        super().__init__(
            code=-32002,  # A2A custom error code
            message=f"Content type '{content_type}' not supported",
            data={"contentType": content_type, "supported": supported},
        )
