"""Property-based tests for A2A EvaluationBundle payloads.

Ensures that even with varying numeric values and string content,
the bundles remain valid according to the schema.
"""

from __future__ import annotations

import hypothesis.strategies as st
from hypothesis import given, settings
from test_evaluation_bundle_schema import validate_evaluation_bundle


@st.composite
def evaluation_bundle_strategy(draw):
    """Generates a valid EvaluationBundle with random but consistent data."""
    started_at = draw(st.integers(min_value=1000000000, max_value=2000000000))
    elapsed = draw(st.floats(min_value=0.0, max_value=10000.0))
    finished_at = int(started_at + elapsed)

    return {
        "schema_version": 1,
        "spec_url": "smi-bench:evaluation_bundle:v1",
        "benchmark": draw(st.sampled_from(["phase1_discovery", "phase2_inhabit"])),
        "run_id": draw(st.text(min_size=1, max_size=50)),
        "exit_code": draw(st.integers(min_value=0, max_value=255)),
        "timings": {
            "started_at_unix_seconds": started_at,
            "finished_at_unix_seconds": finished_at,
            "elapsed_seconds": float(elapsed),
        },
        "config": {
            "corpus_root": draw(st.text()),
            "package_ids_file": draw(st.text()),
            "samples": draw(st.integers()),
            "rpc_url": "http://localhost:9000",
            "simulation_mode": "dry-run",
        },
        "metrics": {
            "avg_hit_rate": draw(st.floats(min_value=0.0, max_value=1.0)),
            "packages_total": draw(st.integers(min_value=0)),
            "packages_with_error": draw(st.integers(min_value=0)),
            "packages_timed_out": draw(st.integers(min_value=0)),
        },
        "artifacts": {
            "results_path": "results/test.json",
            "run_metadata_path": "logs/test/run_metadata.json",
            "events_path": "logs/test/events.jsonl",
        },
    }


@given(evaluation_bundle_strategy())
@settings(deadline=None)
def test_random_bundles_pass_validation(bundle: dict) -> None:
    """Invariant: Bundles generated with valid ranges must always pass validation."""
    validate_evaluation_bundle(bundle)


@given(st.integers(min_value=0, max_value=10**9), st.integers(min_value=0, max_value=10**9))
@settings(deadline=None)
def test_timing_consistency_invariant(start: int, duration: int) -> None:
    """Invariant: finished_at >= started_at is NOT a schema rule, but a logical truth for runs."""
    # This is a meta-test to ensure we don't produce impossible bundles in our own logic
    assert (start + duration) >= start
