"""Checkpoint compatibility tests (checksum/no-checksum/bad checksum).

These tests ensure checkpoint loading is robust and handles various
corruption/version scenarios gracefully.

Tests cover:
- Loading checkpoint with _checksum missing (backward compatibility)
- Loading checkpoint with bad checksum (should error with clear message)
- Resuming when packages contains malformed rows (should skip + log/continue)
This prevents "resume broke in refactor" regressions.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from smi_bench.inhabit_runner import InhabitRunResult
from smi_bench.inhabit_runner import _load_checkpoint as _load_inhabit_checkpoint
from smi_bench.runner import RunResult, _load_checkpoint, _write_checkpoint
from smi_bench.utils import compute_json_checksum

FIXTURES_DIR = Path(__file__).parent / "fixtures"


def test_checkpoint_without_checksum_loads_successfully(tmp_path: Path) -> None:
    """Test that checkpoints without checksum can be loaded (backward compatibility)."""
    fixture_path = FIXTURES_DIR / "phase1_checkpoint_no_checksum.json"
    assert fixture_path.exists()

    # Load checkpoint without checksum
    result = _load_checkpoint(fixture_path)
    assert isinstance(result, RunResult)
    assert result.schema_version == 1
    assert len(result.packages) == 1


def test_checkpoint_with_bad_checksum_raises_error(tmp_path: Path) -> None:
    """Test that checkpoints with bad checksum raise RuntimeError with clear message."""
    fixture_path = FIXTURES_DIR / "phase1_checkpoint_bad_checksum.json"
    assert fixture_path.exists()

    with pytest.raises(RuntimeError) as exc_info:
        _load_checkpoint(fixture_path)
    error_msg = str(exc_info.value).lower()
    assert "checksum mismatch" in error_msg
    assert "corruption" in error_msg or "badchecksum" in error_msg
    # Should provide actionable guidance
    assert "remove the checkpoint" in error_msg or "restart" in error_msg


def test_checkpoint_with_valid_checksum_loads_successfully(tmp_path: Path) -> None:
    """Test that checkpoints with valid checksum load successfully."""

    # Create a minimal valid checkpoint
    result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=1,
        seed=42,
        agent="test",
        aggregate={"errors": 0, "packages_total": 1},
        packages=[],
    )

    checkpoint_path = tmp_path / "checkpoint.json"
    _write_checkpoint(checkpoint_path, result)

    # Load it back
    loaded = _load_checkpoint(checkpoint_path)
    assert loaded.schema_version == 1
    assert loaded.agent == "test"


def test_checkpoint_resume_skips_malformed_packages(tmp_path: Path) -> None:
    """Test that checkpoint resume gracefully skips malformed package rows (should skip + log/continue)."""
    from smi_bench.logging import JsonlLogger
    from smi_bench.runner import _resume_results_from_checkpoint

    # Create checkpoint with one valid and multiple malformed packages
    checkpoint_data = {
        "schema_version": 1,
        "started_at_unix_seconds": 1000,
        "finished_at_unix_seconds": 2000,
        "corpus_root_name": "test",
        "corpus_git": None,
        "target_ids_file": None,
        "target_ids_total": None,
        "samples": 3,
        "seed": 42,
        "agent": "test",
        "aggregate": {"errors": 0, "packages_total": 3},
        "packages": [
            {
                "package_id": "0x111",
                "truth_key_types": 2,
                "predicted_key_types": 2,
                "score": {
                    "tp": 2,
                    "fp": 0,
                    "fn": 0,
                    "precision": 1.0,
                    "recall": 1.0,
                    "f1": 1.0,
                    "missing_sample": [],
                    "extra_sample": [],
                },
            },
            {
                "package_id": "0x222",
                "score": {"invalid": "score"},  # Malformed score - missing required keys
            },
            {
                "package_id": "",  # Invalid package_id (empty string)
                "score": {
                    "tp": 1,
                    "fp": 0,
                    "fn": 0,
                    "precision": 1.0,
                    "recall": 1.0,
                    "f1": 1.0,
                    "missing_sample": [],
                    "extra_sample": [],
                },
            },
            "not a dict",  # Invalid type
            {
                "package_id": "0x333",
                # Missing required fields
            },
        ],
    }

    checkpoint_path = tmp_path / "checkpoint.json"
    checkpoint_path.write_text(json.dumps(checkpoint_data))

    # Load checkpoint
    cp = _load_checkpoint(checkpoint_path)

    # Test resume without logger (should still work)
    results, seen, error_count, started = _resume_results_from_checkpoint(cp)
    assert len(results) == 1  # Only the valid package
    assert "0x111" in seen
    assert error_count == 0  # Malformed rows don't count as errors

    # Test resume with logger (should log skip events)
    logger = JsonlLogger(base_dir=tmp_path, run_id="test_resume")
    results2, seen2, error_count2, started2 = _resume_results_from_checkpoint(cp, logger=logger)
    assert len(results2) == 1
    assert "0x111" in seen2

    # Check that skip events were logged
    events = []
    for line in logger.paths.events.read_text().splitlines():
        if line.strip():
            events.append(json.loads(line))

    # Should have logged skip events for malformed packages
    skip_events = [e for e in events if e.get("event") == "checkpoint_resume_skip"]
    assert len(skip_events) > 0  # At least some malformed rows should have been logged


def test_checkpoint_checksum_is_computed_correctly(tmp_path: Path) -> None:
    """Test that checkpoint checksum computation is deterministic and correct."""

    result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=1,
        seed=42,
        agent="test",
        aggregate={"errors": 0},
        packages=[],
    )

    checkpoint_path = tmp_path / "checkpoint.json"
    _write_checkpoint(checkpoint_path, result)

    # Read back and verify checksum
    data = json.loads(checkpoint_path.read_text())
    stored_checksum = data.pop("_checksum")
    data_copy = {k: v for k, v in data.items() if k != "_checksum"}
    computed = compute_json_checksum(data_copy)

    assert isinstance(stored_checksum, str)
    assert stored_checksum == computed
    assert len(stored_checksum) == 8


def test_phase2_checkpoint_without_checksum_loads_successfully(tmp_path: Path) -> None:
    """Test Phase II checkpoint without checksum loads (backward compatibility)."""

    # Create minimal Phase II checkpoint without checksum
    checkpoint_data = {
        "schema_version": 1,
        "started_at_unix_seconds": 1000,
        "finished_at_unix_seconds": 2000,
        "corpus_root_name": "test",
        "samples": 1,
        "seed": 42,
        "agent": "test",
        "rpc_url": "https://test",
        "sender": "0x1",
        "gas_budget": 10000000,
        "gas_coin": None,
        "aggregate": {"packages_total": 1},
        "packages": [
            {
                "package_id": "0x111",
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            }
        ],
    }

    checkpoint_path = tmp_path / "phase2_checkpoint.json"
    checkpoint_path.write_text(json.dumps(checkpoint_data))

    result = _load_inhabit_checkpoint(checkpoint_path)
    assert isinstance(result, InhabitRunResult)
    assert result.schema_version == 1


def test_checkpoint_missing_required_fields_raises_error(tmp_path: Path) -> None:
    """Test that checkpoints missing required fields raise RuntimeError."""
    invalid_checkpoint = {
        "schema_version": 1,
        # Missing started_at_unix_seconds, etc.
        "packages": [],
    }

    checkpoint_path = tmp_path / "invalid.json"
    checkpoint_path.write_text(json.dumps(invalid_checkpoint))

    with pytest.raises(RuntimeError) as exc_info:
        _load_checkpoint(checkpoint_path)
    assert "invalid checkpoint shape" in str(exc_info.value).lower()


def test_checkpoint_invalid_packages_type_raises_error(tmp_path: Path) -> None:
    """Test that checkpoints with invalid packages type raise RuntimeError."""
    invalid_checkpoint = {
        "schema_version": 1,
        "started_at_unix_seconds": 1000,
        "finished_at_unix_seconds": 2000,
        "corpus_root_name": "test",
        "corpus_git": None,
        "target_ids_file": None,
        "target_ids_total": None,
        "samples": 1,
        "seed": 42,
        "agent": "test",
        "aggregate": {},
        "packages": "not a list",  # Invalid type
    }

    checkpoint_path = tmp_path / "invalid.json"
    checkpoint_path.write_text(json.dumps(invalid_checkpoint))

    with pytest.raises(RuntimeError) as exc_info:
        _load_checkpoint(checkpoint_path)
    assert "invalid checkpoint shape" in str(exc_info.value).lower()


def test_checkpoint_write_validates_schema(tmp_path: Path) -> None:
    """Test that checkpoint write validates schema before writing."""
    # Create invalid RunResult (missing required field)
    invalid_result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=1,
        seed=42,
        agent="test",
        aggregate={},  # Missing required aggregate fields
        packages=[
            {
                "package_id": "0x111",
                "truth_key_types": 1,
                "predicted_key_types": 1,
                "score": {
                    "tp": 1,
                    "fp": 0,
                    "fn": 0,
                    "precision": 1.0,
                    "recall": 1.0,
                    "f1": 1.0,
                    "missing_sample": [],
                    "extra_sample": [],
                },
            }
        ],
    )

    checkpoint_path = tmp_path / "checkpoint.json"

    # Write should succeed (aggregate is flexible)
    _write_checkpoint(checkpoint_path, invalid_result)

    # But if we create a package with missing required fields, validator should catch it
    invalid_package_result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=1,
        seed=42,
        agent="test",
        aggregate={"errors": 0},
        packages=[
            {
                "package_id": "0x111",
                # Missing truth_key_types, predicted_key_types, score
            }
        ],
    )

    # This should fail validation during write
    with pytest.raises(ValueError) as exc_info:
        _write_checkpoint(checkpoint_path, invalid_package_result)
    assert "missing required field" in str(exc_info.value).lower()


def test_phase2_checkpoint_resume_skips_malformed_packages(tmp_path: Path) -> None:
    """Test Phase II checkpoint resume skips malformed packages."""
    from smi_bench.inhabit_runner import _resume_results_from_checkpoint
    from smi_bench.logging import JsonlLogger

    checkpoint_data = {
        "schema_version": 1,
        "started_at_unix_seconds": 1000,
        "finished_at_unix_seconds": 2000,
        "corpus_root_name": "test",
        "samples": 2,
        "seed": 42,
        "agent": "test",
        "rpc_url": "https://test",
        "sender": "0x1",
        "gas_budget": 10000000,
        "gas_coin": None,
        "aggregate": {"packages_total": 2},
        "packages": [
            {
                "package_id": "0x111",
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            },
            {
                "package_id": "0x222",
                "score": {"invalid": "score"},  # Malformed score
            },
            {
                "package_id": "",  # Invalid package_id
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            },
        ],
    }

    checkpoint_path = tmp_path / "phase2_checkpoint.json"
    checkpoint_path.write_text(json.dumps(checkpoint_data))

    cp = _load_inhabit_checkpoint(checkpoint_path)

    logger = JsonlLogger(base_dir=tmp_path, run_id="test_phase2_resume")
    results, seen, error_count, started = _resume_results_from_checkpoint(cp, logger=logger)

    assert len(results) == 1  # Only valid package
    assert "0x111" in seen

    # Check skip events were logged
    events = []
    for line in logger.paths.events.read_text().splitlines():
        if line.strip():
            events.append(json.loads(line))

    skip_events = [e for e in events if e.get("event") == "checkpoint_resume_skip"]
    assert len(skip_events) > 0


def test_phase2_checkpoint_with_bad_checksum_raises_error(tmp_path: Path) -> None:
    """Test that Phase II checkpoints with bad checksum raise RuntimeError."""
    checkpoint_data = {
        "schema_version": 1,
        "started_at_unix_seconds": 1000,
        "finished_at_unix_seconds": 2000,
        "corpus_root_name": "test",
        "samples": 1,
        "seed": 42,
        "agent": "test",
        "rpc_url": "https://test",
        "sender": "0x1",
        "gas_budget": 10000000,
        "aggregate": {},
        "packages": [],
        "_checksum": "deadbeef",  # Bad checksum
    }

    checkpoint_path = tmp_path / "bad_checksum.json"
    checkpoint_path.write_text(json.dumps(checkpoint_data))

    with pytest.raises(RuntimeError, match="Checkpoint checksum mismatch"):
        _load_inhabit_checkpoint(checkpoint_path)


def test_checkpoint_truncated_file_raises_value_error(tmp_path: Path) -> None:
    """Test that truncated JSON file raises a clear ValueError (via safe_json_loads)."""
    checkpoint_path = tmp_path / "truncated.json"
    checkpoint_path.write_text('{"schema_version": 1, "packages": [')  # Truncated

    # Both Phase I and Phase II loaders use safe_json_loads
    with pytest.raises(RuntimeError, match="Checkpoint JSON parse error"):
        _load_checkpoint(checkpoint_path)

    with pytest.raises(RuntimeError, match="Checkpoint JSON parse error"):
        _load_inhabit_checkpoint(checkpoint_path)
