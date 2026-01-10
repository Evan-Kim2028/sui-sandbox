"""End-to-end integration tests for Rust helper (smi_tx_sim).

These tests run the actual compiled Rust binary against local Move fixtures
to verify static analysis correctness without mocking.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from smi_bench.inhabit.engine import run_tx_sim_via_helper

# Constants for the stress test fixture
# current file is repos/sui-move-interface-extractor/benchmark/tests/test_rust_integration.py
# parent 1: tests
# parent 2: benchmark
# parent 3: sui-move-interface-extractor (PROJECT_ROOT)
PROJECT_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_DIR = PROJECT_ROOT / "tests/fixture/build/fixture"
SIM_BIN = PROJECT_ROOT / "target/release/smi_tx_sim"


@pytest.mark.skipif(not SIM_BIN.exists(), reason="smi_tx_sim binary not found. Run cargo build --release first.")
def test_rust_static_analysis_integration() -> None:
    """Verify smi_tx_sim correctly identifies types via static analysis in build-only mode."""

    # Define a PTB spec that calls depth_0 in stress_tests
    # which eventually calls transfer::public_transfer(MyObj, ...) at depth 2.
    ptb_spec = {"calls": [{"target": "0x1::stress_tests::depth_0", "type_args": [], "args": []}]}

    tx_out, created, static_created, mode_used = run_tx_sim_via_helper(
        dev_inspect_bin=SIM_BIN,
        rpc_url="https://fullnode.mainnet.sui.io:443",  # Not used in build-only
        sender="0x123",
        mode="build-only",
        gas_budget=None,
        gas_coin=None,
        bytecode_package_dir=FIXTURE_DIR,
        ptb_spec=ptb_spec,
        timeout_s=30.0,
    )

    assert mode_used == "build_only"

    # Canonical padded address for 0x1
    pkg_padded = "0x0000000000000000000000000000000000000000000000000000000000000001"
    expected_type = f"{pkg_padded}::stress_tests::MyObj"

    assert expected_type in static_created
    assert expected_type in created  # run_tx_sim_via_helper merges static into created for display
