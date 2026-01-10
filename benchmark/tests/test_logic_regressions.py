"""Logic regression tests for SMI Bench.

This suite ensures that the `smi-inhabit` runner produces deterministic,
correct outputs given fixed inputs (corpus + agent responses).
It protects against prompt drift, parsing regressions, and logic errors.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock, patch

import pytest

from smi_bench.inhabit_runner import run
from smi_bench.schema import Phase2ResultKeys as Keys


@dataclass
class ReplayAgent:
    """
    Mock agent that replays pre-recorded responses based on prompt matching.
    """

    responses: dict[str, dict[str, Any]] = field(default_factory=dict)
    default_response: dict[str, Any] | None = None

    # Store history for verification
    history: list[tuple[str, str]] = field(default_factory=list)

    def complete_json(
        self,
        prompt: str,
        *,
        timeout_s: float | None = None,
        logger: Any | None = None,
        log_context: dict[str, object] | None = None,
    ) -> dict[str, Any]:
        """
        Return the first response where the key is a substring of the prompt.
        """
        self.history.append(("json", prompt))

        for key, resp in self.responses.items():
            if key in prompt:
                return resp

        if self.default_response is not None:
            return self.default_response

        raise ValueError(f"No replay response found for prompt: {prompt[:100]}...")


@pytest.fixture
def mock_corpus(tmp_path: Path) -> Path:
    """Create a minimal mock corpus with one package."""
    corpus = tmp_path / "corpus"
    corpus.mkdir()

    # Create prefix directory (e.g., 0x00)
    prefix_dir = corpus / "0x00"
    prefix_dir.mkdir()

    pkg_id = "0x123"
    pkg_dir = prefix_dir / pkg_id
    pkg_dir.mkdir()

    # Create fake bytecode (empty file is fine if we mock the extractor)
    (pkg_dir / "bytecode_modules").mkdir()
    (pkg_dir / "bytecode_modules" / "m.mv").touch()

    # Create required metadata.json
    metadata = pkg_dir / "metadata.json"
    metadata.write_text(json.dumps({"id": pkg_id}))

    return corpus


@pytest.fixture
def mock_extractor_bin(tmp_path: Path) -> Path:
    """Create a dummy rust binary that returns fixed interface JSON."""
    bin_path = tmp_path / "mock_extractor"

    # We don't actually run this because we patch emit_bytecode_json
    # But validation checks existence and permissions
    bin_path.touch()
    bin_path.chmod(0o755)
    return bin_path


def test_inhabit_logic_happy_path(mock_corpus: Path, mock_extractor_bin: Path, tmp_path: Path) -> None:
    """
    Test a full success path:
    1. Agent receives prompt with '0x123'.
    2. Agent returns a valid PTB plan.
    3. Simulator confirms the plan creates the target.
    4. Runner records success.
    """
    # 1. Setup Golden Data
    pkg_id = "0x123"
    target_type = f"{pkg_id}::m::Target"

    interface_json = {
        "modules": {
            "m": {
                "address": pkg_id,
                "functions": {"create": {"visibility": "Public", "is_entry": True, "parameters": [], "return": []}},
                "structs": {"Target": {"abilities": ["key", "store"], "type_parameters": [], "fields": []}},
            }
        }
    }

    # The expected PTB plan from the "Agent"
    golden_plan = {"calls": [{"target": f"{pkg_id}::m::create", "args": [], "type_args": []}]}

    agent = ReplayAgent(responses={pkg_id: golden_plan})

    out_file = tmp_path / "result.json"

    # 2. Mock Everything External
    with (
        patch("smi_bench.inhabit_runner.emit_bytecode_json", return_value=interface_json),
        patch("smi_bench.inhabit_runner.validate_rust_binary", return_value=mock_extractor_bin),
        patch("smi_bench.inhabit_runner.validate_binary", return_value=mock_extractor_bin),
        patch("smi_bench.inhabit_runner.load_real_agent_config"),
        patch("smi_bench.inhabit_runner.RealAgent", return_value=agent),
        patch("smi_bench.inhabit_runner.run_tx_sim_via_helper") as mock_sim,
        patch("smi_bench.inhabit_runner.check_run_guards"),
        patch("smi_bench.inhabit_runner._run_preflight_checks"),
        patch("smi_bench.inhabit_runner.fetch_inventory", return_value={}),
    ):
        # Configure Mock Sim to ALWAYS succeed for this test
        # We assume the Agent did its job (since we mocked it)
        mock_sim.return_value = ({"effects": {"status": {"status": "success"}}}, {target_type}, set(), "dry-run")

        # 3. Run Inhabit Runner
        result = run(
            corpus_root=mock_corpus,
            samples=1,
            seed=42,
            package_ids_file=None,
            agent_name="real-openai-compatible",
            rust_bin=mock_extractor_bin,
            dev_inspect_bin=mock_extractor_bin,
            rpc_url="https://mock.rpc",
            sender="0xsender",
            gas_budget=1000,
            gas_coin=None,
            gas_budget_ladder="",
            max_planning_calls=1,
            max_plan_attempts=1,
            baseline_max_candidates=0,
            max_heuristic_variants=0,
            plan_file=None,
            env_file=None,
            out_path=out_file,
            resume=False,
            continue_on_error=False,
            max_errors=0,
            per_package_timeout_seconds=10.0,
            include_created_types=True,
            require_dry_run=True,
            simulation_mode="dry-run",
            log_dir=None,
            run_id="test_run",
        )

        # 4. Assertions
        # Score is 0.2 because the interface extractor heuristic adds 4 wrapper types
        # (Coin, TreasuryCap, Currency, MetadataCap) for every module-defined key struct.
        # The mock simulation only "inhabits" the base type, so 1 hit / 5 targets = 0.2.
        assert result.aggregate[Keys.AVG_HIT_RATE] == 0.2
        assert len(result.packages) == 1
        pkg = result.packages[0]

        # Check generated plan was correct (by proxy of success)
        assert pkg[Keys.DRY_RUN_OK] is True
        assert pkg[Keys.SCORE]["created_hits"] == 1

        # Verify Agent interaction
        assert len(agent.history) == 1
        assert pkg_id in agent.history[0][1]


def test_inhabit_logic_progressive_exposure(mock_corpus: Path, mock_extractor_bin: Path, tmp_path: Path) -> None:
    """
    Test progressive exposure logic:
    1. Agent requests 'need_more' info.
    2. Runner provides focused interface.
    3. Agent provides plan.
    """
    pkg_id = "0x123"
    target_type = f"{pkg_id}::m::Target"

    interface_json = {
        "modules": {
            "m": {
                "address": pkg_id,
                "functions": {
                    "entry_fn": {"visibility": "Public", "is_entry": True, "parameters": [], "return": []},
                    "helper_fn": {"visibility": "Public", "is_entry": False, "parameters": [], "return": []},
                },
                "structs": {"Target": {"abilities": ["key"], "type_parameters": [], "fields": []}},
            }
        }
    }

    # Agent behavior:
    # 1. First call: Request more info on module 'm'
    # 2. Second call: Return plan using helper_fn
    agent = ReplayAgent()

    def agent_logic(prompt: str, **kwargs) -> dict[str, Any]:
        agent.history.append(("json", prompt))

        # Heuristic to detect first vs second call
        if "helper_fn" not in prompt:
            # First call: we only see entry_fn in summary (default mode)
            return {"need_more": [f"{pkg_id}::m"]}
        else:
            # Second call: we see helper_fn
            return {"calls": [{"target": f"{pkg_id}::m::helper_fn", "args": [], "type_args": []}]}

    # Mock complete_json to use our dynamic logic
    agent.complete_json = MagicMock(side_effect=agent_logic)  # type: ignore

    with (
        patch("smi_bench.inhabit_runner.emit_bytecode_json", return_value=interface_json),
        patch("smi_bench.inhabit_runner.validate_rust_binary", return_value=mock_extractor_bin),
        patch("smi_bench.inhabit_runner.validate_binary", return_value=mock_extractor_bin),
        patch("smi_bench.inhabit_runner.load_real_agent_config"),
        patch("smi_bench.inhabit_runner.RealAgent", return_value=agent),
        patch("smi_bench.inhabit_runner.run_tx_sim_via_helper") as mock_sim,
        patch("smi_bench.inhabit_runner.check_run_guards"),
        patch("smi_bench.inhabit_runner._run_preflight_checks"),
        patch("smi_bench.inhabit_runner.fetch_inventory", return_value={}),
    ):
        mock_sim.return_value = ({}, {target_type}, set(), "dry-run")

        run(
            corpus_root=mock_corpus,
            samples=1,
            seed=42,
            package_ids_file=None,
            agent_name="real-openai-compatible",
            rust_bin=mock_extractor_bin,
            dev_inspect_bin=mock_extractor_bin,
            rpc_url="https://mock.rpc",
            sender="0xsender",
            gas_budget=1000,
            gas_coin=None,
            gas_budget_ladder="",
            max_planning_calls=5,  # Allow multiple steps
            max_plan_attempts=1,
            baseline_max_candidates=0,
            max_heuristic_variants=0,
            plan_file=None,
            env_file=None,
            out_path=tmp_path / "result.json",
            resume=False,
            continue_on_error=False,
            max_errors=0,
            per_package_timeout_seconds=10.0,
            include_created_types=True,
            require_dry_run=True,
            simulation_mode="dry-run",
            log_dir=None,
            run_id="test_run",
        )

        # Verify 2 calls were made (or 3 if it triggered the force-plan fallback)
        assert len(agent.history) >= 2

        # Verify first prompt did NOT have helper_fn (entry_then_public mode)
        assert "helper_fn" not in agent.history[0][1]
