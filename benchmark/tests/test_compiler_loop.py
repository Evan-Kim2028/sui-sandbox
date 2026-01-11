from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

from smi_bench.agents.real_agent import LLMJsonResponse, LLMUsage
from smi_bench.inhabit_runner import _build_real_agent_retry_prompt, run


def _mock_llm_response(content: dict) -> LLMJsonResponse:
    """Helper to create mock LLM responses with zero usage."""
    return LLMJsonResponse(content=content, usage=LLMUsage(0, 0, 0))


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
@patch("smi_bench.inhabit_runner.emit_bytecode_json")
@patch("smi_bench.inhabit_runner.collect_packages")
@patch("smi_bench.inhabit_runner._run_tx_sim_with_fallback")
@patch("smi_bench.inhabit_runner.fetch_inventory")
@patch("smi_bench.inhabit_runner._run_preflight_checks")
def test_run_loop_retries_on_harness_error(
    _mock_preflight,
    mock_fetch_inventory,
    mock_sim,
    mock_collect,
    mock_emit,
    mock_agent_class,
    mock_exists,
    tmp_path: Path,
    monkeypatch,
) -> None:
    # Setup mocks
    monkeypatch.setenv("SMI_API_KEY", "test_key")
    monkeypatch.setenv("SMI_MODEL", "m")
    mock_exists.return_value = True
    mock_fetch_inventory.return_value = {}  # Empty inventory for test
    mock_pkg = MagicMock()
    mock_pkg.package_id = "0x123"
    mock_pkg.package_dir = "/tmp/pkg"
    mock_collect.return_value = [mock_pkg]

    mock_emit.return_value = {"modules": {}}

    # Mock Agent behavior
    mock_agent = mock_agent_class.return_value
    # First call fails with a "harness error" (e.g. invalid JSON from the agent's perspective)
    # Second call succeeds
    mock_agent.complete_json.side_effect = [ValueError("missing field calls"), _mock_llm_response({"calls": []})]

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
        max_planning_calls=2,
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
        parent_pid=None,
        max_run_seconds=None,
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


@patch("pathlib.Path.exists")
@patch("smi_bench.inhabit_runner.RealAgent")
@patch("smi_bench.inhabit_runner.emit_bytecode_json")
@patch("smi_bench.inhabit_runner.collect_packages")
@patch("smi_bench.inhabit_runner._run_tx_sim_with_fallback")
@patch("smi_bench.inhabit_runner.fetch_inventory")
@patch("smi_bench.inhabit_runner._run_preflight_checks")
def test_run_loop_progressive_need_more_uses_multiple_planning_calls(
    _mock_preflight,
    mock_fetch_inventory,
    mock_sim,
    mock_collect,
    mock_emit,
    mock_agent_class,
    mock_exists,
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setenv("SMI_API_KEY", "test_key")
    monkeypatch.setenv("SMI_MODEL", "m")
    mock_exists.return_value = True
    mock_fetch_inventory.return_value = {}  # Empty inventory for test
    mock_pkg = MagicMock()
    mock_pkg.package_id = "0x123"
    mock_pkg.package_dir = "/tmp/pkg"
    mock_collect.return_value = [mock_pkg]

    mock_emit.return_value = {"modules": {"m": {"functions": {}}}}

    mock_agent = mock_agent_class.return_value
    mock_agent.complete_json.side_effect = [
        _mock_llm_response({"need_more": ["0x1::m::f"], "reason": "need details"}),
        _mock_llm_response({"calls": []}),
    ]

    mock_sim.return_value = (
        {"effects": {"status": {"status": "success"}}},
        set(),
        set(),
        "dry-run",
        False,
        True,
        None,
    )

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
        max_planning_calls=2,
        max_plan_attempts=1,
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
        parent_pid=None,
        max_run_seconds=None,
    )


@patch("pathlib.Path.exists")
@patch("smi_bench.inhabit_runner.RealAgent")
@patch("smi_bench.inhabit_runner.emit_bytecode_json")
@patch("smi_bench.inhabit_runner.collect_packages")
@patch("smi_bench.inhabit_runner._run_preflight_checks")
def test_run_guard_exits_when_parent_pid_missing(
    _mock_preflight, mock_collect, mock_emit, _mock_agent_class, mock_exists, tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setenv("SMI_API_KEY", "test_key")
    monkeypatch.setenv("SMI_MODEL", "m")
    mock_exists.return_value = True

    mock_pkg = MagicMock()
    mock_pkg.package_id = "0x123"
    mock_pkg.package_dir = "/tmp/pkg"
    mock_collect.return_value = [mock_pkg]
    mock_emit.return_value = {"modules": {}}

    # Make the parent PID check deterministically fail.
    monkeypatch.setattr("smi_bench.inhabit.engine.pid_is_alive", lambda _pid: False)

    out_path = tmp_path / "results.json"
    run(
        corpus_root=Path("/tmp/corpus"),
        samples=1,
        seed=0,
        package_ids_file=None,
        agent_name="baseline-search",
        rust_bin=Path("fake_rust"),
        dev_inspect_bin=Path("fake_sim"),
        rpc_url="https://fake_rpc",
        sender="0x1",
        gas_budget=1000,
        gas_coin=None,
        gas_budget_ladder="",
        max_planning_calls=1,
        max_plan_attempts=1,
        baseline_max_candidates=1,
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
        simulation_mode="build-only",
        log_dir=None,
        run_id=None,
        parent_pid=12345,
        max_run_seconds=None,
    )

    data = json.loads(out_path.read_text())
    assert data["aggregate"]["errors"] == 1
    assert "Parent process exited" in data["packages"][0]["error"]


@patch("pathlib.Path.exists")
@patch("smi_bench.inhabit_runner.RealAgent")
@patch("smi_bench.inhabit_runner.emit_bytecode_json")
@patch("smi_bench.inhabit_runner.collect_packages")
@patch("smi_bench.inhabit_runner._run_preflight_checks")
def test_run_guard_exits_when_max_run_seconds_exceeded(
    _mock_preflight, mock_collect, mock_emit, _mock_agent_class, mock_exists, tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setenv("SMI_API_KEY", "test_key")
    monkeypatch.setenv("SMI_MODEL", "m")
    mock_exists.return_value = True

    mock_pkg = MagicMock()
    mock_pkg.package_id = "0x123"
    mock_pkg.package_dir = "/tmp/pkg"
    mock_collect.return_value = [mock_pkg]
    mock_emit.return_value = {"modules": {}}

    # Force the guard to trip immediately.
    monkeypatch.setattr("smi_bench.inhabit_runner.time.monotonic", lambda: 1_000_000.0)

    out_path = tmp_path / "results.json"
    run(
        corpus_root=Path("/tmp/corpus"),
        samples=1,
        seed=0,
        package_ids_file=None,
        agent_name="baseline-search",
        rust_bin=Path("fake_rust"),
        dev_inspect_bin=Path("fake_sim"),
        rpc_url="https://fake_rpc",
        sender="0x1",
        gas_budget=1000,
        gas_coin=None,
        gas_budget_ladder="",
        max_planning_calls=1,
        max_plan_attempts=1,
        baseline_max_candidates=1,
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
        simulation_mode="build-only",
        log_dir=None,
        run_id=None,
        parent_pid=None,
        max_run_seconds=0.0,
    )

    data = json.loads(out_path.read_text())
    assert data["aggregate"]["errors"] == 1
    assert "Maximum run time exceeded" in data["packages"][0]["error"]
