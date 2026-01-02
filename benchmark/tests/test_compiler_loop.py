from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

from smi_bench.inhabit_runner import _build_real_agent_retry_prompt, run


def test_build_real_agent_retry_prompt_includes_harness_error() -> None:
    package_id = "0x123"
    target_key_types = {"0x1::m::S"}
    last_failure = {"harness_error": "missing field calls"}

    prompt = _build_real_agent_retry_prompt(
        package_id=package_id, target_key_types=target_key_types, last_failure=last_failure
    )

    assert "The harness failed to parse your JSON" in prompt
    assert "missing field calls" in prompt
    # Ensure payload contains the error
    assert '"harness_error": "missing field calls"' in prompt


def test_build_real_agent_retry_prompt_includes_dry_run_error() -> None:
    package_id = "0x123"
    target_key_types = {"0x1::m::S"}
    last_failure = {"dry_run_effects_error": "InsufficientGas"}

    prompt = _build_real_agent_retry_prompt(
        package_id=package_id, target_key_types=target_key_types, last_failure=last_failure
    )

    assert "failed on-chain simulation: InsufficientGas" in prompt


@patch("pathlib.Path.exists")
@patch("smi_bench.inhabit_runner.RealAgent")
@patch("smi_bench.inhabit_runner._run_rust_emit_bytecode_json")
@patch("smi_bench.inhabit_runner.collect_packages")
@patch("smi_bench.inhabit_runner._run_tx_sim_with_fallback")
def test_run_loop_retries_on_harness_error(
    mock_sim, mock_collect, mock_emit, mock_agent_class, mock_exists, tmp_path: Path, monkeypatch
) -> None:
    # Setup mocks
    monkeypatch.setenv("SMI_API_KEY", "test_key")
    mock_exists.return_value = True
    mock_pkg = MagicMock()
    mock_pkg.package_id = "0x123"
    mock_pkg.package_dir = "/tmp/pkg"
    mock_collect.return_value = [mock_pkg]

    mock_emit.return_value = {"modules": {}}

    # Mock Agent behavior
    mock_agent = mock_agent_class.return_value
    # First call fails with a "harness error" (e.g. invalid JSON from the agent's perspective)
    # Second call succeeds
    mock_agent.complete_json.side_effect = [ValueError("missing field calls"), {"calls": []}]

    # Mock simulation success for the second attempt
    mock_sim.return_value = (
        {"effects": {"status": {"status": "success"}}},  # tx_out
        set(),  # created
        set(),  # static
        "dry-run",  # mode
        False,  # fell_back
        True,  # rpc_ok
        None,  # dry_run_err
    )

    # Run the benchmark
    out_path = tmp_path / "results.json"
    run(
        corpus_root=Path("/tmp/corpus"),
        samples=1,
        seed=0,
        package_ids_file=None,
        agent_name="real-openai-compatible",
        rust_bin=Path("fake_rust"),
        dev_inspect_bin=Path("fake_sim"),
        rpc_url="https://fake_rpc",
        sender="0x1",
        gas_budget=1000,
        gas_coin=None,
        gas_budget_ladder="",
        max_plan_attempts=2,
        baseline_max_candidates=5,
        max_heuristic_variants=1,
        plan_file=None,
        env_file=None,
        out_path=out_path,
        resume=False,
        continue_on_error=True,
        max_errors=1,
        checkpoint_every=0,
        per_package_timeout_seconds=10,
        include_created_types=False,
        require_dry_run=False,
        simulation_mode="dry-run",
        log_dir=None,
        run_id=None,
    )

    # Verify results
    data = json.loads(out_path.read_text())
    pkg_res = data["packages"][0]

    assert pkg_res["package_id"] == "0x123"
    # SUCCESS: It should have taken 2 attempts
    assert pkg_res["plan_attempts"] == 2
    assert pkg_res["dry_run_ok"] is True
    assert pkg_res["error"] is None

    # Verify retry prompt was built correctly for the second attempt
    # The first call to complete_json was for attempt 1 (normal prompt)
    # The second call to complete_json was for attempt 2 (retry prompt)
    assert mock_agent.complete_json.call_count == 2
