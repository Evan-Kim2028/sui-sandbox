"""Golden fixture regression tests for Phase I/II outputs.

These tests validate that the output schema remains stable and that we can
load/validate real run outputs correctly. This prevents accidental schema
breakage during refactors.

These are snapshot-style tests that load golden Phase I/II run JSON files and assert:
- Required keys exist
- Types are correct (int vs str vs list)
- schema_version matches
- Additive fields are allowed
This catches accidental renames/removals early.
"""

from __future__ import annotations

import json
from pathlib import Path

from smi_bench.inhabit_runner import InhabitRunResult
from smi_bench.inhabit_runner import _load_checkpoint as _load_inhabit_checkpoint
from smi_bench.runner import RunResult, _load_checkpoint
from smi_bench.schema import validate_phase1_run_json, validate_phase2_run_json
from smi_bench.utils import safe_json_loads

FIXTURES_DIR = Path(__file__).parent / "fixtures"


def test_phase1_golden_fixture_schema() -> None:
    """Validate Phase I golden fixture matches expected schema using validator."""
    fixture_path = FIXTURES_DIR / "phase1_golden_run.json"
    assert fixture_path.exists(), f"Golden fixture not found: {fixture_path}"

    # Load and parse
    data = safe_json_loads(fixture_path.read_text(), context=f"golden fixture {fixture_path}")

    # Use schema validator (catches renames/removals)
    validate_phase1_run_json(data)

    # Additional assertions for golden fixture specifics
    assert data["schema_version"] == 1
    assert len(data["packages"]) > 0

    # Validate checksum if present
    stored_checksum = data.get("_checksum")
    if stored_checksum:
        # Note: fixture checksum is fake, but structure should be valid
        assert isinstance(stored_checksum, str)
        assert len(stored_checksum) == 8


def test_phase1_golden_fixture_can_load_as_runresult() -> None:
    """Validate Phase I golden fixture can be loaded as RunResult (backward compatibility)."""
    fixture_path = FIXTURES_DIR / "phase1_golden_run.json"
    # Temporarily remove checksum to test loading
    data = json.loads(fixture_path.read_text())
    data.pop("_checksum", None)

    # Write to temp file and load via checkpoint loader
    import tempfile

    with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
        json.dump(data, f)
        temp_path = Path(f.name)

    try:
        result = _load_checkpoint(temp_path)
        assert isinstance(result, RunResult)
        assert result.schema_version == 1
        assert len(result.packages) == 2
        assert result.aggregate["packages_total"] == 2
    finally:
        temp_path.unlink()


def test_phase2_golden_fixture_schema() -> None:
    """Validate Phase II golden fixture matches expected schema using validator."""
    fixture_path = FIXTURES_DIR / "phase2_golden_run.json"
    assert fixture_path.exists(), f"Golden fixture not found: {fixture_path}"

    # Load and parse
    data = safe_json_loads(fixture_path.read_text(), context=f"golden fixture {fixture_path}")

    # Use schema validator (catches renames/removals)
    validate_phase2_run_json(data)

    # Additional assertions for golden fixture specifics
    assert data["schema_version"] == 1
    assert len(data["packages"]) > 0


def test_phase2_golden_fixture_can_load_as_inhabitrresult() -> None:
    """Validate Phase II golden fixture can be loaded as InhabitRunResult."""
    fixture_path = FIXTURES_DIR / "phase2_golden_run.json"
    # Temporarily remove checksum to test loading
    data = json.loads(fixture_path.read_text())
    data.pop("_checksum", None)

    # Write to temp file and load via checkpoint loader
    import tempfile

    with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
        json.dump(data, f)
        temp_path = Path(f.name)

    try:
        result = _load_inhabit_checkpoint(temp_path)
        assert isinstance(result, InhabitRunResult)
        assert result.schema_version == 1
        assert len(result.packages) == 2
        assert result.aggregate["packages_total"] == 2
    finally:
        temp_path.unlink()


def test_golden_fixtures_have_consistent_types() -> None:
    """Validate that golden fixtures use consistent type representations."""
    phase1_path = FIXTURES_DIR / "phase1_golden_run.json"
    phase2_path = FIXTURES_DIR / "phase2_golden_run.json"

    for path in [phase1_path, phase2_path]:
        if not path.exists():
            continue
        data = json.loads(path.read_text())

        # Timestamps should be integers
        assert isinstance(data["started_at_unix_seconds"], int)
        assert isinstance(data["finished_at_unix_seconds"], int)

        # IDs should be strings
        assert isinstance(data.get("agent"), str)
        for pkg in data.get("packages", []):
            assert isinstance(pkg.get("package_id"), str)


def test_golden_fixtures_allow_additive_fields() -> None:
    """Validate that golden fixtures allow additive fields (backward compatibility)."""
    phase1_path = FIXTURES_DIR / "phase1_golden_run.json"
    phase2_path = FIXTURES_DIR / "phase2_golden_run.json"

    for path, validator in [(phase1_path, validate_phase1_run_json), (phase2_path, validate_phase2_run_json)]:
        if not path.exists():
            continue

        data = json.loads(path.read_text())

        # Add a new optional field (simulating future schema addition)
        data["_test_additive_field"] = "test_value"

        # Validator should still pass (additive fields allowed)
        validator(data)
