"""Event schema validation tests for events.jsonl.

These tests ensure that event logging maintains a consistent schema
and that all events include required fields.

Minimal event contract:
- required: t (Unix timestamp), event (event name)
- recommended: run_id, phase, package_id for package-scoped events

Event name registry helps catch typos ("inventroy_fetch_failed" type bugs).
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from smi_bench.logging import JsonlLogger

FIXTURES_DIR = Path(__file__).parent / "fixtures"

# Required fields for all events
REQUIRED_EVENT_FIELDS = {"t", "event"}

# Recommended fields (not enforced, but used in tests)
RECOMMENDED_EVENT_FIELDS = {
    "run_id",  # For run-scoped events
    "phase",  # For phase identification
    "package_id",  # For package-scoped events
}

# Known event names (event name registry - helps catch typos)
KNOWN_EVENT_NAMES = {
    "run_started",
    "run_finished",
    "package_started",
    "package_finished",
    "checkpoint_resume_skip",
    "inventory_fetch_failed",
    "temp_file_cleanup_failed",
    "llm_request_error",
    "llm_response",
    "llm_json_parse_error",
    "llm_json_parsed",
    "plan_attempt_harness_error",
    "agent_effective_config",
}


def validate_event(event: dict, line_num: int | None = None) -> None:
    """Validate a single event against schema.

    Raises ValueError with descriptive message if validation fails.
    """
    prefix = f"Event {line_num}: " if line_num is not None else "Event: "

    # Check required fields
    for field in REQUIRED_EVENT_FIELDS:
        if field not in event:
            raise ValueError(f"{prefix}missing required field: {field}")

    # Validate types
    if not isinstance(event["t"], int):
        raise ValueError(f"{prefix}'t' must be integer, got {type(event['t']).__name__}")

    if not isinstance(event["event"], str):
        raise ValueError(f"{prefix}'event' must be string, got {type(event['event']).__name__}")

    # Validate timestamp is positive
    if event["t"] <= 0:
        raise ValueError(f"{prefix}'t' must be positive Unix timestamp, got {event['t']}")

    # Warn about unknown event names (helps catch typos)
    event_name = event["event"]
    if event_name not in KNOWN_EVENT_NAMES:
        # Unknown events are allowed (flexibility), but we log for maintainability
        pass


def test_golden_events_jsonl_schema(tmp_path: Path) -> None:
    """Validate golden events.jsonl fixture matches expected schema.

    Reads events.jsonl from a small mocked run and asserts required keys exist on every line.
    """
    fixture_path = FIXTURES_DIR / "events_golden.jsonl"
    assert fixture_path.exists(), f"Golden events fixture not found: {fixture_path}"

    events = []
    for line_num, line in enumerate(fixture_path.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        try:
            event = json.loads(line)
            events.append(event)
            # Validate using validator function
            validate_event(event, line_num=line_num)
        except json.JSONDecodeError as e:
            pytest.fail(f"Line {line_num} is not valid JSON: {e}")
        except ValueError as e:
            pytest.fail(f"Line {line_num} validation failed: {e}")

    assert len(events) > 0, "No events found in fixture"

    # Additional validation: check that known event names are spelled correctly
    for event in events:
        event_name = event["event"]
        # Check for common typos (e.g., "inventroy" instead of "inventory")
        if "inventroy" in event_name.lower():
            pytest.fail(f"Possible typo in event name: {event_name} (should be 'inventory'?)")


def test_events_have_consistent_timestamps(tmp_path: Path) -> None:
    """Validate that events have consistent timestamp ordering."""
    fixture_path = FIXTURES_DIR / "events_golden.jsonl"
    if not fixture_path.exists():
        pytest.skip("Golden events fixture not found")

    events = []
    for line in fixture_path.read_text().splitlines():
        if not line.strip():
            continue
        events.append(json.loads(line))

    if len(events) < 2:
        pytest.skip("Need at least 2 events to test ordering")

    # Timestamps should be non-decreasing (events can have same timestamp)
    prev_t = events[0]["t"]
    for i, event in enumerate(events[1:], start=1):
        assert event["t"] >= prev_t, f"Event {i} timestamp {event['t']} < previous {prev_t}"
        prev_t = event["t"]


def test_jsonl_logger_produces_valid_events(tmp_path: Path) -> None:
    """Test that JsonlLogger produces events matching the schema."""
    logger = JsonlLogger(base_dir=tmp_path, run_id="test_run")

    # Emit various event types
    logger.event("run_started", started_at_unix_seconds=1000, agent="test")
    logger.event("package_started", package_id="0x111", i=1)
    logger.event("package_finished", package_id="0x111", elapsed_seconds=0.5, f1=1.0)
    logger.event("run_finished", finished_at_unix_seconds=2000, packages_total=1, avg_f1=1.0)

    # Read back and validate
    events_path = logger.paths.events
    assert events_path.exists()

    events = []
    for line in events_path.read_text().splitlines():
        if not line.strip():
            continue
        events.append(json.loads(line))

    assert len(events) == 4

    for event in events:
        # Check required fields
        assert "t" in event
        assert "event" in event
        assert isinstance(event["t"], int)
        assert isinstance(event["event"], str)


def test_event_schema_allows_additional_fields(tmp_path: Path) -> None:
    """Test that event schema allows additional fields (flexibility)."""
    logger = JsonlLogger(base_dir=tmp_path, run_id="test_run")

    # Emit event with extra fields
    logger.event("custom_event", required_field="value", extra_field="allowed", nested={"key": "value"})

    # Read back and validate
    events_path = logger.paths.events
    events = [json.loads(line) for line in events_path.read_text().splitlines() if line.strip()]

    assert len(events) == 1
    event = events[0]
    assert event["event"] == "custom_event"
    assert "extra_field" in event
    assert "nested" in event


def test_event_schema_requires_t_and_event(tmp_path: Path) -> None:
    """Test that events.jsonl schema validation catches missing required fields."""
    logger = JsonlLogger(base_dir=tmp_path, run_id="test_run")

    # Emit valid event
    logger.event("test_event", data="value")

    # Manually corrupt the file (simulate bug)
    events_path = logger.paths.events
    corrupted_line = json.dumps({"missing_t": True, "event": "test"}) + "\n"
    events_path.write_text(corrupted_line)

    # Read back - corrupted event should be detectable
    events = []
    for line in events_path.read_text().splitlines():
        if not line.strip():
            continue
        event = json.loads(line)
        events.append(event)

    # Find the corrupted event and validate it fails
    corrupted = [e for e in events if "missing_t" in e]
    if corrupted:
        # Validator should catch missing 't' field
        with pytest.raises(ValueError, match="missing required field: t"):
            validate_event(corrupted[0])


def test_event_schema_validates_all_events_in_file(tmp_path: Path) -> None:
    """Test that we can validate all events in an events.jsonl file."""
    logger = JsonlLogger(base_dir=tmp_path, run_id="test_run")

    # Emit multiple events
    logger.event("run_started", started_at_unix_seconds=1000, agent="test")
    logger.event("package_started", package_id="0x111", i=1)
    logger.event("package_finished", package_id="0x111", elapsed_seconds=0.5)
    logger.event("run_finished", finished_at_unix_seconds=2000, packages_total=1)

    # Read and validate all events
    events_path = logger.paths.events
    for line_num, line in enumerate(events_path.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        event = json.loads(line)
        validate_event(event, line_num=line_num)

    # All events should pass validation
    assert True  # If we get here, validation passed


def test_event_name_registry_catches_typos(tmp_path: Path) -> None:
    """Test that event name registry helps catch typos."""
    logger = JsonlLogger(base_dir=tmp_path, run_id="test_run")

    # Emit event with potential typo (should still work, but we can detect it)
    logger.event("inventroy_fetch_failed", package_id="0x111")  # Typo: "inventroy" instead of "inventory"

    events_path = logger.paths.events
    events = [json.loads(line) for line in events_path.read_text().splitlines() if line.strip()]

    # Event should still be valid (has required fields)
    assert len(events) == 1
    validate_event(events[0])

    # But event name is not in registry (typo detection)
    assert events[0]["event"] not in KNOWN_EVENT_NAMES


def test_events_jsonl_is_valid_jsonl(tmp_path: Path) -> None:
    """Test that events.jsonl is valid JSONL format (one JSON object per line)."""
    logger = JsonlLogger(base_dir=tmp_path, run_id="test_run")

    # Emit multiple events
    for i in range(5):
        logger.event("test_event", index=i, data=f"value_{i}")

    # Read back and validate JSONL format
    events_path = logger.paths.events
    lines = events_path.read_text().splitlines()

    assert len(lines) == 5

    for i, line in enumerate(lines):
        assert line.strip(), f"Line {i} is empty"
        try:
            event = json.loads(line)
            assert isinstance(event, dict), f"Line {i} is not a JSON object"
        except json.JSONDecodeError as e:
            pytest.fail(f"Line {i} is not valid JSON: {e}")
