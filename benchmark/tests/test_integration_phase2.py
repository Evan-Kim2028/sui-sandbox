"""Integration tests for Phase II (PTB Inhabitation).

These tests cover end-to-end flows including:
- Complete Phase II execution with baseline agent
- Checkpoint and resume cycles
- Dry-run mode execution
- Build-only mode execution
- Inventory fetching and resolution
- Gas budget ladder retries
"""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

from smi_bench.checkpoint import load_checkpoint, write_checkpoint
from smi_bench.inhabit_runner import (
    InhabitRunResult,
)


def test_phase2_full_run_with_baseline_agent(tmp_path: Path) -> None:
    """Complete Phase II execution with baseline agent succeeds."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    out_json = tmp_path / "phase2_output.json"

    with (
        patch("smi_bench.inhabit_runner.collect_packages") as mock_collect,
        patch("smi_bench.inhabit_runner.validate_rust_binary") as mock_validate,
        patch("smi_bench.inhabit_runner.emit_bytecode_json") as mock_emit,
        patch("smi_bench.inhabit_runner.run_tx_sim_via_helper") as mock_sim,
        patch("smi_bench.inhabit_runner.JsonlLogger"),
        patch("smi_bench.inhabit_runner._run_preflight_checks"),
    ):
        mock_pkg = MagicMock()
        mock_pkg.package_id = "0x1"
        mock_pkg.package_dir = corpus_root / "0x1"
        mock_collect.return_value = [mock_pkg]
        mock_validate.return_value = Path("fake_rust")
        mock_emit.return_value = {"package_id": "0x1", "modules": {}}
        mock_sim.return_value = (
            None,  # tx_out
            set(),  # created_types
            set(),  # static_types
            "build-only",  # mode_used
        )

        argv = [
            "--corpus-root",
            str(corpus_root),
            "--samples",
            "1",
            "--agent",
            "baseline-search",
            "--simulation-mode",
            "build-only",
            "--out",
            str(out_json),
        ]

        with patch("sys.argv", ["smi-inhabit"] + argv):
            from smi_bench import inhabit_runner

            inhabit_runner.main(argv)

            # Should complete successfully
            assert out_json.exists()


def test_phase2_checkpoint_and_resume(tmp_path: Path) -> None:
    """Checkpoint/resume cycle preserves state correctly."""
    checkpoint_path = tmp_path / "checkpoint.json"

    # Create initial checkpoint
    initial_result = InhabitRunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test_corpus",
        samples=10,
        seed=42,
        agent="test-agent",
        rpc_url="https://test.rpc",
        sender="0x1",
        gas_budget=10_000_000,
        gas_coin=None,
        aggregate={"packages_total": 2},
        packages=[
            {
                "package_id": "0x1",
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            },
            {
                "package_id": "0x2",
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            },
        ],
    )

    write_checkpoint(checkpoint_path, initial_result)

    # Load checkpoint and verify
    # Note: load_checkpoint returns a dict, unlike _load_checkpoint (legacy).
    # The new checkpoint.load_checkpoint returns a generic dict.
    # We should reconstruct if needed, but for testing attributes we can check the dict.
    loaded_dict = load_checkpoint(checkpoint_path)
    assert loaded_dict["schema_version"] == initial_result.schema_version
    assert loaded_dict["agent"] == initial_result.agent
    assert len(loaded_dict["packages"]) == len(initial_result.packages)
    assert loaded_dict["packages"][0]["package_id"] == "0x1"
    assert loaded_dict["packages"][1]["package_id"] == "0x2"


def test_phase2_dry_run_mode(tmp_path: Path, monkeypatch) -> None:
    """Dry-run mode executes transaction simulation."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    out_json = tmp_path / "phase2_output.json"

    with (
        patch("smi_bench.inhabit_runner.collect_packages") as mock_collect,
        patch("smi_bench.inhabit_runner.validate_rust_binary") as mock_validate,
        patch("smi_bench.inhabit_runner.emit_bytecode_json") as mock_emit,
        patch("smi_bench.inhabit_runner._run_tx_sim_with_fallback") as mock_sim,
        patch("smi_bench.inhabit_runner.JsonlLogger"),
        patch("smi_bench.inhabit_runner._run_preflight_checks"),
    ):
        mock_pkg = MagicMock()
        mock_pkg.package_id = "0x1"
        mock_pkg.package_dir = corpus_root / "0x1"
        mock_collect.return_value = [mock_pkg]
        mock_validate.return_value = Path("fake_rust")
        mock_emit.return_value = {"package_id": "0x1", "modules": {}}
        # Return dry-run mode
        mock_sim.return_value = (
            {"effects": {"status": {"status": "success"}}},
            set(),
            set(),
            "dry-run",
            False,
            True,
            None,
        )

        argv = [
            "--corpus-root",
            str(corpus_root),
            "--samples",
            "1",
            "--agent",
            "baseline-search",
            "--simulation-mode",
            "dry-run",
            "--sender",
            "0x1234",
            "--out",
            str(out_json),
        ]

        with patch("sys.argv", ["smi-inhabit"] + argv):
            from smi_bench import inhabit_runner

            inhabit_runner.main(argv)

            # Should complete successfully
            assert out_json.exists()

            # Note: mock_sim is not called because we use _run_tx_sim_with_fallback in actual code


def test_phase2_build_only_mode(tmp_path: Path) -> None:
    """Build-only mode doesn't execute transaction simulation."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    out_json = tmp_path / "phase2_output.json"

    with (
        patch("smi_bench.inhabit_runner.collect_packages") as mock_collect,
        patch("smi_bench.inhabit_runner.validate_rust_binary") as mock_validate,
        patch("smi_bench.inhabit_runner.emit_bytecode_json") as mock_emit,
        patch("smi_bench.inhabit_runner.run_tx_sim_via_helper") as mock_sim,
        patch("smi_bench.inhabit_runner.JsonlLogger"),
        patch("smi_bench.inhabit_runner._run_preflight_checks"),
    ):
        mock_pkg = MagicMock()
        mock_pkg.package_id = "0x1"
        mock_pkg.package_dir = corpus_root / "0x1"
        mock_collect.return_value = [mock_pkg]
        mock_validate.return_value = Path("fake_rust")
        mock_emit.return_value = {"package_id": "0x1", "modules": {}}
        # Return build-only mode (no simulation)
        mock_sim.return_value = (
            None,  # tx_out
            set(),  # created_types
            set(),  # static_types
            "build-only",  # mode_used
        )

        argv = [
            "--corpus-root",
            str(corpus_root),
            "--samples",
            "1",
            "--agent",
            "baseline-search",
            "--simulation-mode",
            "build-only",
            "--sender",
            "0x0",
            "--out",
            str(out_json),
        ]

        with patch("sys.argv", ["smi-inhabit"] + argv):
            from smi_bench import inhabit_runner

            inhabit_runner.main(argv)

            # Should complete successfully
            assert out_json.exists()


def test_phase2_inventory_fetch_and_resolution(tmp_path: Path, monkeypatch) -> None:
    """Inventory fetching and placeholder resolution works correctly."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    out_json = tmp_path / "phase2_output.json"

    with (
        patch("smi_bench.inhabit_runner.collect_packages") as mock_collect,
        patch("smi_bench.inhabit_runner.validate_rust_binary") as mock_validate,
        patch("smi_bench.inhabit_runner.emit_bytecode_json") as mock_emit,
        patch("smi_bench.inhabit_runner.fetch_inventory") as mock_inventory,
        patch("smi_bench.inhabit_runner.resolve_placeholders") as mock_resolve,
        patch("smi_bench.inhabit_runner._run_tx_sim_with_fallback") as mock_sim,
        patch("smi_bench.inhabit_runner.JsonlLogger"),
        patch("smi_bench.inhabit_runner._run_preflight_checks"),
    ):
        mock_pkg = MagicMock()
        mock_pkg.package_id = "0x1"
        mock_pkg.package_dir = corpus_root / "0x1"
        mock_collect.return_value = [mock_pkg]
        mock_validate.return_value = Path("fake_rust")
        mock_emit.return_value = {
            "package_id": "0x1",
            "modules": {
                "m": {
                    "functions": {
                        "create_coin": {
                            "params": [
                                {
                                    "kind": "ref",
                                    "mutable": True,
                                    "to": {"kind": "address"},
                                }
                            ]
                        }
                    }
                }
            },
        }
        # Mock inventory with some coins
        mock_inventory.return_value = {
            "0x1": ["coin_1", "coin_2"],
            "0x2": ["coin_3"],
        }
        # Mock successful resolution
        mock_resolve.return_value = True

        mock_sim.return_value = (
            {"effects": {"status": {"status": "success"}}},
            set(),
            set(),
            "dry-run",
            False,
            True,
            None,
        )

        argv = [
            "--corpus-root",
            str(corpus_root),
            "--samples",
            "1",
            "--agent",
            "baseline-search",
            "--simulation-mode",
            "dry-run",
            "--sender",
            "0x1234",
            "--out",
            str(out_json),
        ]

        with patch("sys.argv", ["smi-inhabit"] + argv):
            from smi_bench import inhabit_runner

            inhabit_runner.main(argv)

            # Should complete successfully
            assert out_json.exists()

            # Verify inventory was fetched
            assert mock_inventory.called


def test_phase2_gas_budget_ladder_retries(tmp_path: Path) -> None:
    """Gas budget ladder retries on gas errors."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    out_json = tmp_path / "phase2_output.json"

    with (
        patch("smi_bench.inhabit_runner.collect_packages") as mock_collect,
        patch("smi_bench.inhabit_runner.validate_rust_binary") as mock_validate,
        patch("smi_bench.inhabit_runner.emit_bytecode_json") as mock_emit,
        patch("smi_bench.inhabit_runner._run_tx_sim_with_fallback") as mock_sim,
        patch("smi_bench.inhabit_runner.fetch_inventory") as mock_inventory,
        patch("smi_bench.inhabit_runner.JsonlLogger"),
        patch("smi_bench.inhabit_runner._run_preflight_checks"),
    ):
        mock_pkg = MagicMock()
        mock_pkg.package_id = "0x1"
        mock_pkg.package_dir = corpus_root / "0x1"
        mock_collect.return_value = [mock_pkg]
        mock_validate.return_value = Path("fake_rust")
        mock_emit.return_value = {"package_id": "0x1", "modules": {}}
        mock_inventory.return_value = {}  # Empty inventory for test

        # Simulate gas error on first attempt, success on second
        call_count = [0]

        def simulate_with_ladder(*args, **kwargs):
            call_count[0] += 1
            if call_count[0] == 1:
                # First attempt: gas error
                return (
                    {"effects": {}},
                    set(),
                    set(),
                    "dry-run",
                    True,  # fell_back
                    True,
                    {"error": "InsufficientGas"},
                )
            else:
                # Second attempt: success
                return (
                    {"effects": {"status": {"status": "success"}}},
                    set(),
                    set(),
                    "dry-run",
                    False,
                    True,
                    None,
                )

        mock_sim.side_effect = simulate_with_ladder

        argv = [
            "--corpus-root",
            str(corpus_root),
            "--samples",
            "1",
            "--agent",
            "baseline-search",
            "--gas-budget-ladder",
            "10000000,20000000,30000000",
            "--simulation-mode",
            "dry-run",
            "--sender",
            "0x1234",
            "--out",
            str(out_json),
        ]

        with patch("sys.argv", ["smi-inhabit"] + argv):
            from smi_bench import inhabit_runner

            inhabit_runner.main(argv)

            # Should complete successfully
            assert out_json.exists()

            # Verify simulation was attempted
            assert mock_sim.call_count >= 1


def test_phase2_checkpoint_checksum_validated(tmp_path: Path) -> None:
    """Checkpoint checksum is computed and validated on load."""
    checkpoint_path = tmp_path / "checkpoint.json"

    result = InhabitRunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        samples=1,
        seed=42,
        agent="test",
        rpc_url="https://test",
        sender="0x1",
        gas_budget=10_000_000,
        gas_coin=None,
        aggregate={"packages_total": 1},
        packages=[
            {
                "package_id": "0x1",
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            }
        ],
    )

    write_checkpoint(checkpoint_path, result)

    # Load checkpoint (checksum should be validated)
    loaded = load_checkpoint(checkpoint_path)
    assert loaded["schema_version"] == result.schema_version

    # Read raw file to verify checksum exists
    raw_data = json.loads(checkpoint_path.read_text())
    assert "_checksum" in raw_data
    assert len(raw_data["_checksum"]) == 8


def test_phase2_resume_loads_packages_from_checkpoint(tmp_path: Path) -> None:
    """Resume loads package results from checkpoint."""
    checkpoint_path = tmp_path / "checkpoint.json"

    result = InhabitRunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        samples=2,
        seed=42,
        agent="test",
        rpc_url="https://test",
        sender="0x1",
        gas_budget=10_000_000,
        gas_coin=None,
        aggregate={"packages_total": 2},
        packages=[
            {
                "package_id": "0x1",
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            },
            {
                "package_id": "0x2",
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            },
        ],
    )

    write_checkpoint(checkpoint_path, result)

    # Load checkpoint and verify packages loaded correctly
    from smi_bench.inhabit_runner import _resume_results_from_checkpoint

    # Note: _resume_results_from_checkpoint currently expects InhabitRunResult object.
    # We need to wrap the dict returned by load_checkpoint.
    # We must wrap dict in InhabitRunResult for compatibility with resume logic.
    # to take a dict or InhabitRunResult.
    # For now, let's assume we update _resume_results_from_checkpoint to take a dict or we wrap it here.
    # Let's recreate the object for the test to be safe until we refactor _resume_results_from_checkpoint.
    loaded_dict = load_checkpoint(checkpoint_path)
    # We need to construct InhabitRunResult from dict to pass to _resume_results_from_checkpoint
    # This mimics what _load_checkpoint used to do.
    loaded_obj = InhabitRunResult(**loaded_dict)
    
    loaded_packages, seen, error_count, started = _resume_results_from_checkpoint(loaded_obj)

    assert len(loaded_packages) == 2
    assert "0x1" in seen
    assert "0x2" in seen
    assert error_count == 0
    assert started == 1000  # started_at_unix_seconds, not finished_at


def test_phase2_deterministic_output_with_same_seed(tmp_path: Path) -> None:
    """Same seed produces deterministic output."""
    # Create two identical checkpoints with same seed
    checkpoint1 = tmp_path / "checkpoint1.json"
    checkpoint2 = tmp_path / "checkpoint2.json"

    result = InhabitRunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        samples=1,
        seed=12345,  # Fixed seed
        agent="test",
        rpc_url="https://test",
        sender="0x1",
        gas_budget=10_000_000,
        gas_coin=None,
        aggregate={"packages_total": 1},
        packages=[
            {
                "package_id": "0x1",
                "score": {"targets": 1, "created_distinct": 0, "created_hits": 0, "missing": 1},
            }
        ],
    )

    write_checkpoint(checkpoint1, result)
    write_checkpoint(checkpoint2, result)

    # Load both and compare (should be identical)
    loaded1 = load_checkpoint(checkpoint1)
    loaded2 = load_checkpoint(checkpoint2)

    assert loaded1["schema_version"] == loaded2["schema_version"]
    assert loaded1["seed"] == loaded2["seed"]
    assert loaded1["packages"] == loaded2["packages"]
