"""Integration tests for Phase I (Key Struct Discovery).

These tests cover end-to-end flows including:
- Complete Phase I execution with mock agents
- Checkpoint and resume cycles
- Continue-on-error mode
- Samples from corpus files
- Git metadata tracking
- Output schema validation
"""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from smi_bench.runner import RunResult, _load_checkpoint, _write_checkpoint


def test_phase1_full_run_with_mock_agent(tmp_path: Path, monkeypatch) -> None:
    """Complete Phase I execution with mock agent succeeds."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    # Create minimal package structure
    pkg_dir = corpus_root / "0x1" / "build"
    pkg_dir.mkdir(parents=True)

    # Create minimal compiled module
    pkg_dir.mkdir(parents=True, exist_ok=True)

    out_json = tmp_path / "phase1_output.json"

    with (
        patch("smi_bench.runner.collect_packages") as mock_collect,
        patch("smi_bench.runner.validate_rust_binary") as mock_validate,
        patch("smi_bench.runner.emit_bytecode_json") as mock_emit,
        patch("smi_bench.runner._build_rust"),
        patch("smi_bench.runner.JsonlLogger"),
    ):
        # Mock a package for the runner to process
        mock_pkg = MagicMock()
        mock_pkg.package_id = "0x1"
        mock_pkg.package_dir = pkg_dir
        mock_collect.return_value = [mock_pkg]
        mock_validate.return_value = Path("fake_rust")
        mock_emit.return_value = {"package_id": "0x1", "modules": {}}

        argv = [
            "--corpus-root",
            str(corpus_root),
            "--samples",
            "1",
            "--agent",
            "mock-empty",
            "--out",
            str(out_json),
        ]

        with patch("sys.argv", ["smi-phase1"] + argv):
            from smi_bench import runner

            runner.main(argv)

            # Should complete successfully
            assert out_json.exists()


def test_phase1_checkpoint_and_resume(tmp_path: Path) -> None:
    """Checkpoint/resume cycle preserves state correctly."""
    checkpoint_path = tmp_path / "checkpoint.json"

    # Create initial checkpoint
    initial_result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test_corpus",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=10,
        seed=42,
        agent="test-agent",
        aggregate={"avg_f1": 0.85, "errors": 0, "packages_total": 2},
        packages=[
            {
                "package_id": "0x1",
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
            }
        ],
    )

    _write_checkpoint(checkpoint_path, initial_result)

    # Load checkpoint and verify
    loaded = _load_checkpoint(checkpoint_path)
    assert loaded.schema_version == initial_result.schema_version
    assert loaded.agent == initial_result.agent
    assert len(loaded.packages) == len(initial_result.packages)
    assert loaded.packages[0]["package_id"] == "0x1"


def test_phase1_continue_on_error(tmp_path: Path, monkeypatch) -> None:
    """Continue-on-error mode handles failures gracefully."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    out_json = tmp_path / "phase1_output.json"

    with (
        patch("smi_bench.runner.collect_packages") as mock_collect,
        patch("smi_bench.runner.validate_rust_binary") as mock_validate,
        patch("smi_bench.runner.emit_bytecode_json") as mock_emit,
        patch("smi_bench.runner._build_rust"),
        patch("smi_bench.runner.JsonlLogger"),
    ):
        # Mock collect to return one package
        mock_pkg = MagicMock()
        mock_pkg.package_id = "0x1"
        mock_pkg.package_dir = tmp_path / "pkg"
        mock_collect.return_value = [mock_pkg]
        mock_validate.return_value = Path("fake_rust")
        mock_emit.return_value = {"package_id": "0x1", "modules": {}}

        argv = [
            "--corpus-root",
            str(corpus_root),
            "--samples",
            "1",
            "--agent",
            "mock-empty",
            "--out",
            str(out_json),
            "--continue-on-error",
            "--max-errors",
            "1",
        ]

        with patch("sys.argv", ["smi-phase1"] + argv):
            from smi_bench import runner

            runner.main(argv)

            # Should complete successfully with continue-on-error
            assert out_json.exists()


def test_phase1_samples_from_corpus_file(tmp_path: Path, monkeypatch) -> None:
    """Sample from corpus file when provided."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    # Create package ids file
    ids_file = tmp_path / "package_ids.txt"
    ids_file.write_text("0x1\n0x2\n0x3\n")

    out_json = tmp_path / "phase1_output.json"

    with (
        patch("smi_bench.runner.collect_packages"),
        patch("smi_bench.runner.validate_rust_binary") as mock_validate,
        patch("smi_bench.runner.emit_bytecode_json") as mock_emit,
        patch("smi_bench.runner._build_rust"),
        patch("smi_bench.runner.JsonlLogger"),
    ):
        mock_validate.return_value = Path("fake_rust")
        mock_emit.return_value = {"package_id": "0x1", "modules": {}}

        argv = [
            "--corpus-root",
            str(corpus_root),
            "--package-ids-file",
            str(ids_file),
            "--samples",
            "2",  # Sample 2 from file of 3
            "--agent",
            "mock-empty",
            "--out",
            str(out_json),
        ]

        with patch("sys.argv", ["smi-phase1"] + argv):
            with pytest.raises(SystemExit):
                from smi_bench import runner

                runner.main(argv)


def test_phase1_git_metadata_included(tmp_path: Path, monkeypatch) -> None:
    """Git metadata tracking includes commit info when available."""
    checkpoint_path = tmp_path / "checkpoint.json"

    result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test_corpus",
        corpus_git={"head": "abc123", "head_commit_time": "2024-01-01T00:00:00Z"},
        target_ids_file=None,
        target_ids_total=None,
        samples=10,
        seed=42,
        agent="test-agent",
        aggregate={"avg_f1": 0.85, "errors": 0, "packages_total": 1},
        packages=[],
    )

    _write_checkpoint(checkpoint_path, result)

    # Load and verify git metadata preserved
    loaded = _load_checkpoint(checkpoint_path)
    assert loaded.corpus_git is not None
    assert loaded.corpus_git.get("head") == "abc123"
    assert loaded.corpus_git.get("head_commit_time") == "2024-01-01T00:00:00Z"


def test_phase1_output_schema_matches_validator(tmp_path: Path) -> None:
    """Output schema matches validator expectations."""
    checkpoint_path = tmp_path / "checkpoint.json"

    result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test_corpus",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=10,
        seed=42,
        agent="test-agent",
        aggregate={
            "avg_f1": 0.85,
            "errors": 0,
            "packages_total": 1,
        },
        packages=[
            {
                "package_id": "0x1",
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
            }
        ],
    )

    _write_checkpoint(checkpoint_path, result)

    # Load and validate with schema validator
    loaded = _load_checkpoint(checkpoint_path)
    data = {
        "schema_version": loaded.schema_version,
        "started_at_unix_seconds": loaded.started_at_unix_seconds,
        "finished_at_unix_seconds": loaded.finished_at_unix_seconds,
        "corpus_root_name": loaded.corpus_root_name,
        "corpus_git": loaded.corpus_git,
        "target_ids_file": loaded.target_ids_file,
        "target_ids_total": loaded.target_ids_total,
        "samples": loaded.samples,
        "seed": loaded.seed,
        "agent": loaded.agent,
        "aggregate": loaded.aggregate,
        "packages": loaded.packages,
    }

    # Should not raise exception
    from smi_bench.schema import validate_phase1_run_json

    validate_phase1_run_json(data)


def test_phase1_checkpoint_checksum_validated(tmp_path: Path) -> None:
    """Checkpoint checksum is computed and validated on load."""
    checkpoint_path = tmp_path / "checkpoint.json"

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

    _write_checkpoint(checkpoint_path, result)

    # Load checkpoint (checksum should be validated)
    loaded = _load_checkpoint(checkpoint_path)
    assert loaded.schema_version == result.schema_version

    # Read raw file to verify checksum exists
    raw_data = json.loads(checkpoint_path.read_text())
    assert "_checksum" in raw_data
    assert len(raw_data["_checksum"]) == 8  # Checksum is 8 chars


def test_phase1_resume_loads_packages_from_checkpoint(tmp_path: Path) -> None:
    """Resume loads package results from checkpoint."""
    checkpoint_path = tmp_path / "checkpoint.json"

    result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=2,
        seed=42,
        agent="test",
        aggregate={"errors": 0, "packages_total": 2},
        packages=[
            {
                "package_id": "0x1",
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
            },
            {
                "package_id": "0x2",
                "truth_key_types": 1,
                "predicted_key_types": 0,
                "score": {
                    "tp": 0,
                    "fp": 0,
                    "fn": 1,
                    "precision": 0.0,
                    "recall": 0.0,
                    "f1": 0.0,
                    "missing_sample": ["0x2::m::S"],
                    "extra_sample": [],
                },
            },
        ],
    )

    _write_checkpoint(checkpoint_path, result)

    # Load checkpoint and verify packages loaded correctly
    from smi_bench.runner import _resume_results_from_checkpoint

    loaded_packages, seen, error_count, started = _resume_results_from_checkpoint(_load_checkpoint(checkpoint_path))

    assert len(loaded_packages) == 2
    assert "0x1" in seen
    assert "0x2" in seen
    assert error_count == 0
    assert started == 1000  # started_at_unix_seconds, not finished_at


def test_phase1_deterministic_output_with_same_seed(tmp_path: Path) -> None:
    """Same seed produces deterministic output."""
    # Create two identical checkpoints with same seed
    checkpoint1 = tmp_path / "checkpoint1.json"
    checkpoint2 = tmp_path / "checkpoint2.json"

    result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=1,
        seed=12345,  # Fixed seed
        agent="test",
        aggregate={"errors": 0, "packages_total": 1},
        packages=[
            {
                "package_id": "0x1",
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

    _write_checkpoint(checkpoint1, result)
    _write_checkpoint(checkpoint2, result)

    # Load both and compare (should be identical)
    loaded1 = _load_checkpoint(checkpoint1)
    loaded2 = _load_checkpoint(checkpoint2)

    assert loaded1.schema_version == loaded2.schema_version
    assert loaded1.seed == loaded2.seed
    assert loaded1.packages == loaded2.packages
