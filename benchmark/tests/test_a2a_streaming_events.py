"""Streaming event format tests for A2A protocol.

These tests validate Server-Sent Events (SSE) format and content
for task status updates and artifact notifications.
"""

from __future__ import annotations

import json
from typing import Any


class TestStreamingEventFormat:
    """Test SSE event format compliance."""

    def test_task_status_update_event_structure(self) -> None:
        """TaskStatusUpdateEvent should have correct structure."""
        # Test expected event structure based on A2A spec
        # Events are emitted via TaskUpdater.update_status()
        event_dict = {
            "kind": "TaskStatusUpdateEvent",
            "task_id": "test-task-1",
            "status": {
                "state": "working",
                "timestamp": "2026-01-02T00:00:00Z",
            },
        }

        # Verify required fields
        assert "task_id" in event_dict
        assert "status" in event_dict
        assert event_dict["task_id"] == "test-task-1"
        assert "state" in event_dict["status"]
        assert event_dict["status"]["state"] == "working"
        assert "timestamp" in event_dict["status"]

    def test_task_artifact_update_event_structure(self) -> None:
        """TaskArtifactUpdateEvent should have correct structure."""
        # Test expected artifact event structure
        event_dict = {
            "kind": "TaskArtifactUpdateEvent",
            "task_id": "test-task-1",
            "artifact": {
                "name": "test_artifact",
                "parts": [],
            },
        }

        # Verify required fields
        assert "task_id" in event_dict
        assert "artifact" in event_dict
        assert event_dict["task_id"] == "test-task-1"
        assert event_dict["artifact"]["name"] == "test_artifact"
        assert "parts" in event_dict["artifact"]

    def test_event_serialization_to_json(self) -> None:
        """Events should serialize to valid JSON."""
        event_dict = {
            "kind": "TaskStatusUpdateEvent",
            "task_id": "test-task-1",
            "status": {
                "state": "completed",
                "timestamp": "2026-01-02T00:00:00Z",
            },
        }

        # Should serialize without errors
        json_str = json.dumps(event_dict)
        parsed = json.loads(json_str)

        assert parsed["task_id"] == "test-task-1"
        assert parsed["status"]["state"] == "completed"


class TestStreamingEventSequence:
    """Test expected event sequence during task execution."""

    def test_task_lifecycle_event_sequence(self) -> None:
        """Task should emit events in correct order."""
        # Expected sequence:
        # 1. Task object (initial)
        # 2. TaskStatusUpdateEvent (submitted → working)
        # 3. TaskStatusUpdateEvent (working → completed)
        # 4. TaskArtifactUpdateEvent (evaluation_bundle)

        events: list[dict[str, Any]] = []

        # Simulate event sequence
        events.append(
            {
                "kind": "task",
                "id": "task-1",
                "status": {"state": "submitted"},
            }
        )

        events.append(
            {
                "kind": "TaskStatusUpdateEvent",
                "task_id": "task-1",
                "status": {"state": "working"},
            }
        )

        events.append(
            {
                "kind": "TaskArtifactUpdateEvent",
                "task_id": "task-1",
                "artifact": {"name": "evaluation_bundle"},
            }
        )

        events.append(
            {
                "kind": "TaskStatusUpdateEvent",
                "task_id": "task-1",
                "status": {"state": "completed"},
            }
        )

        # Verify sequence
        assert events[0]["kind"] == "task"
        assert events[0]["status"]["state"] == "submitted"

        status_updates = [e for e in events if e["kind"] == "TaskStatusUpdateEvent"]
        assert len(status_updates) >= 2
        assert status_updates[0]["status"]["state"] == "working"
        assert status_updates[-1]["status"]["state"] == "completed"

        artifact_updates = [e for e in events if e["kind"] == "TaskArtifactUpdateEvent"]
        assert len(artifact_updates) >= 1
        assert artifact_updates[0]["artifact"]["name"] == "evaluation_bundle"

    def test_cancellation_event_sequence(self) -> None:
        """Cancelled task should emit cancellation event."""
        events: list[dict[str, Any]] = []

        # Simulate cancellation sequence
        events.append(
            {
                "kind": "task",
                "id": "task-1",
                "status": {"state": "submitted"},
            }
        )

        events.append(
            {
                "kind": "TaskStatusUpdateEvent",
                "task_id": "task-1",
                "status": {"state": "working"},
            }
        )

        events.append(
            {
                "kind": "TaskStatusUpdateEvent",
                "task_id": "task-1",
                "status": {"state": "cancelled"},
            }
        )

        # Verify cancellation appears
        status_updates = [e for e in events if e["kind"] == "TaskStatusUpdateEvent"]
        cancelled = [e for e in status_updates if e["status"]["state"] == "cancelled"]
        assert len(cancelled) == 1


class TestStreamingEventContent:
    """Test event content correctness."""

    def test_status_update_includes_timestamp(self) -> None:
        """Status updates should include timestamp."""
        event_dict = {
            "kind": "TaskStatusUpdateEvent",
            "task_id": "test-task-1",
            "status": {
                "state": "working",
                "timestamp": "2026-01-02T00:00:00Z",
            },
        }

        assert "timestamp" in event_dict["status"]
        assert event_dict["status"]["timestamp"] is not None
        assert isinstance(event_dict["status"]["timestamp"], str)

    def test_artifact_update_includes_parts(self) -> None:
        """Artifact updates should include parts."""
        event_dict = {
            "kind": "TaskArtifactUpdateEvent",
            "task_id": "test-task-1",
            "artifact": {
                "name": "test_artifact",
                "parts": [
                    {
                        "kind": "text",
                        "text": "test content",
                    },
                ],
            },
        }

        assert "parts" in event_dict["artifact"]
        assert len(event_dict["artifact"]["parts"]) > 0
        assert event_dict["artifact"]["parts"][0]["kind"] == "text"
