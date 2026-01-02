"""CLI entry point tests for smi-bench CLI router.

Tests cover the main CLI orchestration including:
- Phase I → Phase II → Execution flow
- Error propagation between phases
- Mode selection (build-only vs dry-run)
- Argument parsing and validation
"""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from smi_bench import cli


def test_run_all_orchestration_success(tmp_path: Path, monkeypatch) -> None:
    """Validates Phase I → II → Execution flow completes successfully."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    out_dir = tmp_path / "out"

    # Mock each phase to succeed
    with (
        patch("smi_bench.runner.main") as mock_phase1,
        patch("smi_bench.inhabit_manifest.main") as mock_manifest,
        patch("smi_bench.inhabit_runner.main"),
    ):
        args = MagicMock(
            corpus_root=corpus_root,
            out_dir=out_dir,
            samples=10,
            rpc_url="https://test.rpc",
            sender=None,
            command="run-all",
        )
        cli.run_all(args)

        # Verify all phases were called
        assert mock_phase1.called
        assert mock_manifest.called


def test_run_all_phase1_failure_propagates(tmp_path: Path, monkeypatch) -> None:
    """Phase I failure stops execution and propagates error code."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    out_dir = tmp_path / "out"

    with patch("smi_bench.runner.main") as mock_phase1:
        # Mock SystemExit with error code
        mock_phase1.side_effect = SystemExit(1)

        args = MagicMock(
            corpus_root=corpus_root,
            out_dir=out_dir,
            samples=10,
            rpc_url="https://test.rpc",
            sender=None,
            command="run-all",
        )

        with pytest.raises(SystemExit) as exc_info:
            cli.run_all(args)

        assert exc_info.value.code == 1


def test_run_all_phase2_manifest_failure_propagates(tmp_path: Path, monkeypatch) -> None:
    """Manifest generation failure stops execution and propagates error code."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    out_dir = tmp_path / "out"

    with (
        patch("smi_bench.runner.main"),
        patch("smi_bench.inhabit_manifest.main") as mock_manifest,
    ):
        mock_manifest.side_effect = SystemExit(2)

        args = MagicMock(
            corpus_root=corpus_root,
            out_dir=out_dir,
            samples=10,
            rpc_url="https://test.rpc",
            sender=None,
            command="run-all",
        )

        with pytest.raises(SystemExit) as exc_info:
            cli.run_all(args)

        assert exc_info.value.code == 2


def test_run_all_phase2_execution_failure_propagates(tmp_path: Path, monkeypatch) -> None:
    """Phase II execution failure stops execution and propagates error code."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    out_dir = tmp_path / "out"

    with (
        patch("smi_bench.runner.main"),
        patch("smi_bench.inhabit_manifest.main"),
        patch("smi_bench.inhabit_runner.main") as mock_phase2,
    ):
        mock_phase2.side_effect = SystemExit(3)

        args = MagicMock(
            corpus_root=corpus_root,
            out_dir=out_dir,
            samples=10,
            rpc_url="https://test.rpc",
            sender=None,
            command="run-all",
        )

        with pytest.raises(SystemExit) as exc_info:
            cli.run_all(args)

        assert exc_info.value.code == 3


def test_run_all_build_only_mode_uses_dummy_sender(tmp_path: Path, monkeypatch) -> None:
    """Verify build-only mode doesn't require sender (uses dummy 0x0)."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    out_dir = tmp_path / "out"

    with (
        patch("smi_bench.runner.main"),
        patch("smi_bench.inhabit_manifest.main"),
        patch("smi_bench.inhabit_runner.main") as mock_phase2,
    ):
        args = MagicMock(
            corpus_root=corpus_root,
            out_dir=out_dir,
            samples=10,
            rpc_url="https://test.rpc",
            sender="0x0",  # Dummy sender for build-only
            command="run-all",
        )

        cli.run_all(args)

        # Verify Phase II was called
        assert mock_phase2.called
        # Verify mode is build-only (no real sender)
        call_args = mock_phase2.call_args[0][0]
        assert "--simulation-mode" in call_args
        # Find the mode argument
        mode_idx = call_args.index("--simulation-mode")
        assert call_args[mode_idx + 1] == "build-only"


def test_run_all_dry_run_mode_uses_provided_sender(tmp_path: Path, monkeypatch) -> None:
    """Verify dry-run mode uses the provided funded sender address."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    out_dir = tmp_path / "out"

    with (
        patch("smi_bench.runner.main"),
        patch("smi_bench.inhabit_manifest.main"),
        patch("smi_bench.inhabit_runner.main") as mock_phase2,
    ):
        args = MagicMock(
            corpus_root=corpus_root,
            out_dir=out_dir,
            samples=10,
            rpc_url="https://test.rpc",
            sender="0x1234...abcd",  # Real sender for dry-run
            command="run-all",
        )

        cli.run_all(args)

        assert mock_phase2.called
        call_args = mock_phase2.call_args[0][0]
        assert "--sender" in call_args
        # Verify the provided sender is passed
        sender_idx = call_args.index("--sender")
        assert call_args[sender_idx + 1] == "0x1234...abcd"
        # Verify mode is dry-run
        mode_idx = call_args.index("--simulation-mode")
        assert call_args[mode_idx + 1] == "dry-run"


def test_run_all_creates_output_directory(tmp_path: Path, monkeypatch) -> None:
    """Verify output directory is created if it doesn't exist."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    out_dir = tmp_path / "out" / "nested" / "dir"  # Nested path

    with (
        patch("smi_bench.runner.main"),
        patch("smi_bench.inhabit_manifest.main"),
        patch("smi_bench.inhabit_runner.main"),
    ):
        args = MagicMock(
            corpus_root=corpus_root,
            out_dir=out_dir,
            samples=10,
            rpc_url="https://test.rpc",
            sender=None,
            command="run-all",
        )

        cli.run_all(args)

        # Verify directory was created
        assert out_dir.exists()
        assert out_dir.is_dir()


def test_run_all_passes_samples_limit(tmp_path: Path, monkeypatch) -> None:
    """Verify samples argument is passed through all phases."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    out_dir = tmp_path / "out"

    with (
        patch("smi_bench.runner.main") as mock_phase1,
        patch("smi_bench.inhabit_manifest.main") as mock_manifest,
        patch("smi_bench.inhabit_runner.main"),
    ):
        args = MagicMock(
            corpus_root=corpus_root,
            out_dir=out_dir,
            samples=42,  # Specific sample count
            rpc_url="https://test.rpc",
            sender=None,
            command="run-all",
        )

        cli.run_all(args)

        # Verify Phase I received samples
        phase1_args = mock_phase1.call_args[0][0]
        assert "--samples" in phase1_args
        samples_idx = phase1_args.index("--samples")
        assert phase1_args[samples_idx + 1] == "42"

        # Verify manifest received --limit
        manifest_args = mock_manifest.call_args[0][0]
        assert "--limit" in manifest_args
        limit_idx = manifest_args.index("--limit")
        assert manifest_args[limit_idx + 1] == "42"


def test_cli_main_missing_required_args(monkeypatch) -> None:
    """Error handling for missing required arguments."""
    with patch("sys.argv", ["smi-bench"]):  # No subcommand
        with pytest.raises(SystemExit):
            cli.main()


def test_cli_main_invalid_command(monkeypatch) -> None:
    """Error handling for invalid subcommand."""
    with patch("sys.argv", ["smi-bench", "invalid-command"]):
        with pytest.raises(SystemExit):
            cli.main()
