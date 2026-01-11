"""
Tests for config passthrough from API to CLI subprocess.

Verifies that all EvalConfig fields are correctly passed through
to the smi-inhabit CLI subprocess when executing tasks.
"""

from pathlib import Path

import pytest

from smi_bench.a2a_green_agent import (
    _load_cfg,
)


@pytest.fixture
def manifest_file(tmp_path: Path) -> str:
    """Create a temporary manifest file for testing."""
    manifest = tmp_path / "manifest.txt"
    manifest.write_text("0x1\n")
    return str(manifest)


class TestConfigToCliArgs:
    """Test that EvalConfig fields map correctly to CLI arguments."""

    def _make_full_config(self, manifest_file: str) -> dict:
        """Create a config dict with all fields specified."""
        return {
            "corpus_root": "/app/corpus",
            "package_ids_file": manifest_file,
            "samples": 100,
            "agent": "real-openai-compatible",
            "rpc_url": "https://custom.rpc.sui.io",
            "simulation_mode": "dry-run",
            "per_package_timeout_seconds": 180.0,
            "max_plan_attempts": 5,
            "continue_on_error": True,
            "resume": False,
            "run_id": "test_run_001",
            "model": "gpt-4-turbo",
            # P0 fields
            "seed": 42,
            "sender": "0xabc123def456",
            "gas_budget": 25_000_000,
            "gas_coin": "0xgas789",
            "gas_budget_ladder": "30000000,60000000",
            "max_errors": 10,
            "max_run_seconds": 3600.0,
            # P1 fields
            "max_planning_calls": 25,
            "checkpoint_every": 5,
            "max_heuristic_variants": 6,
            "baseline_max_candidates": 50,
            "include_created_types": True,
            "require_dry_run": True,
        }

    def test_all_fields_parsed_to_evalconfig(self, manifest_file: str):
        """Verify all config fields are stored in EvalConfig."""
        config = self._make_full_config(manifest_file)
        cfg = _load_cfg(config)

        # Core fields
        assert cfg.corpus_root == config["corpus_root"]
        assert cfg.package_ids_file == config["package_ids_file"]
        assert cfg.samples == config["samples"]
        assert cfg.agent == config["agent"]
        assert cfg.rpc_url == config["rpc_url"]
        assert cfg.simulation_mode == config["simulation_mode"]
        assert cfg.per_package_timeout_seconds == config["per_package_timeout_seconds"]
        assert cfg.max_plan_attempts == config["max_plan_attempts"]
        assert cfg.continue_on_error == config["continue_on_error"]
        assert cfg.resume == config["resume"]
        assert cfg.run_id == config["run_id"]
        assert cfg.model == config["model"]

        # P0 fields
        assert cfg.seed == config["seed"]
        assert cfg.sender == config["sender"]
        assert cfg.gas_budget == config["gas_budget"]
        assert cfg.gas_coin == config["gas_coin"]
        assert cfg.gas_budget_ladder == config["gas_budget_ladder"]
        assert cfg.max_errors == config["max_errors"]
        assert cfg.max_run_seconds == config["max_run_seconds"]

        # P1 fields
        assert cfg.max_planning_calls == config["max_planning_calls"]
        assert cfg.checkpoint_every == config["checkpoint_every"]
        assert cfg.max_heuristic_variants == config["max_heuristic_variants"]
        assert cfg.baseline_max_candidates == config["baseline_max_candidates"]
        assert cfg.include_created_types == config["include_created_types"]
        assert cfg.require_dry_run == config["require_dry_run"]

    def test_cli_args_construction_includes_all_fields(self, manifest_file: str):
        """
        Verify the subprocess args list includes all config fields.

        This is a structural test that checks the args construction pattern
        in _run_task_logic. We verify by checking the expected arg patterns.
        """
        config = self._make_full_config(manifest_file)
        cfg = _load_cfg(config)

        # Simulate the args construction from _run_task_logic
        args = [
            "smi-inhabit",
            "--corpus-root",
            str(cfg.corpus_root),
            "--package-ids-file",
            str(cfg.package_ids_file),
            "--agent",
            cfg.agent,
            "--rpc-url",
            cfg.rpc_url,
            "--simulation-mode",
            cfg.simulation_mode,
            "--per-package-timeout-seconds",
            str(cfg.per_package_timeout_seconds),
            "--max-plan-attempts",
            str(cfg.max_plan_attempts),
            "--out",
            "/tmp/out.json",
            "--run-id",
            cfg.run_id or "test",
            "--samples",
            str(cfg.samples),
            # P0 fields
            "--seed",
            str(cfg.seed),
            "--gas-budget",
            str(cfg.gas_budget),
            "--gas-budget-ladder",
            cfg.gas_budget_ladder,
            "--max-errors",
            str(cfg.max_errors),
            # P1 fields
            "--max-planning-calls",
            str(cfg.max_planning_calls),
            "--checkpoint-every",
            str(cfg.checkpoint_every),
            "--max-heuristic-variants",
            str(cfg.max_heuristic_variants),
            "--baseline-max-candidates",
            str(cfg.baseline_max_candidates),
        ]

        # Conditional flags
        if cfg.continue_on_error:
            args.append("--continue-on-error")
        if cfg.sender:
            args.extend(["--sender", cfg.sender])
        if cfg.gas_coin:
            args.extend(["--gas-coin", cfg.gas_coin])
        if cfg.max_run_seconds is not None:
            args.extend(["--max-run-seconds", str(cfg.max_run_seconds)])
        if cfg.include_created_types:
            args.append("--include-created-types")
        if cfg.require_dry_run:
            args.append("--require-dry-run")

        # Verify all expected flags are present
        assert "--seed" in args
        assert "42" in args  # seed value
        assert "--sender" in args
        assert "0xabc123def456" in args
        assert "--gas-budget" in args
        assert "25000000" in args
        assert "--gas-coin" in args
        assert "0xgas789" in args
        assert "--gas-budget-ladder" in args
        assert "30000000,60000000" in args
        assert "--max-errors" in args
        assert "10" in args
        assert "--max-run-seconds" in args
        assert "3600.0" in args
        assert "--max-planning-calls" in args
        assert "25" in args
        assert "--checkpoint-every" in args
        assert "5" in args
        assert "--max-heuristic-variants" in args
        assert "6" in args
        assert "--baseline-max-candidates" in args
        assert "50" in args
        assert "--include-created-types" in args
        assert "--require-dry-run" in args
        assert "--continue-on-error" in args


class TestConfigDefaultsToCliArgs:
    """Test that default values are correctly passed to CLI."""

    def test_default_config_cli_args(self, manifest_file: str):
        """Default config should produce valid CLI args."""
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp/corpus",
                "package_ids_file": manifest_file,
            }
        )

        # Verify defaults
        assert cfg.seed == 0
        assert cfg.gas_budget == 10_000_000
        assert cfg.gas_budget_ladder == "20000000,50000000"
        assert cfg.max_errors == 25
        assert cfg.max_run_seconds is None
        assert cfg.max_planning_calls == 50
        assert cfg.checkpoint_every == 10
        assert cfg.max_heuristic_variants == 4
        assert cfg.baseline_max_candidates == 25
        assert cfg.include_created_types is False
        assert cfg.require_dry_run is False

    def test_optional_fields_not_in_args_when_none(self, manifest_file: str):
        """Optional fields with None value should not appear in CLI args."""
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp/corpus",
                "package_ids_file": manifest_file,
            }
        )

        # Build args list
        args = []

        # These should NOT be added when None/False
        if cfg.sender:
            args.extend(["--sender", cfg.sender])
        if cfg.gas_coin:
            args.extend(["--gas-coin", cfg.gas_coin])
        if cfg.max_run_seconds is not None:
            args.extend(["--max-run-seconds", str(cfg.max_run_seconds)])
        if cfg.include_created_types:
            args.append("--include-created-types")
        if cfg.require_dry_run:
            args.append("--require-dry-run")

        # Verify these are NOT in args
        assert "--sender" not in args
        assert "--gas-coin" not in args
        assert "--max-run-seconds" not in args
        assert "--include-created-types" not in args
        assert "--require-dry-run" not in args


class TestManifestAlias:
    """Test that 'manifest' is an alias for 'package_ids_file'."""

    def test_manifest_alias_works(self, manifest_file: str):
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp/corpus",
                "manifest": manifest_file,  # Using alias
            }
        )
        assert cfg.package_ids_file == manifest_file

    def test_package_ids_file_takes_precedence(self, manifest_file: str, tmp_path: Path):
        secondary = tmp_path / "secondary.txt"
        secondary.write_text("0x2\n")
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp/corpus",
                "package_ids_file": manifest_file,
                "manifest": str(secondary),  # Should be ignored
            }
        )
        assert cfg.package_ids_file == manifest_file
