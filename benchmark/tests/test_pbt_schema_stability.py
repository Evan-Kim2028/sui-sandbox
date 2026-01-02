"""Property-based tests for Phase II schema stability.

Ensures that complex result objects can be serialized to JSON and
reloaded without data loss or schema violations.
"""

from __future__ import annotations

import json
from dataclasses import asdict

import hypothesis.strategies as st
from hypothesis import given, settings

from smi_bench.inhabit.score import InhabitationScore
from smi_bench.inhabit_runner import InhabitPackageResult, InhabitRunResult, _to_package_dict
from smi_bench.schema import validate_phase2_run_json

# --- Strategies ---


@st.composite
def inhabitation_score_strategy(draw):
    targets = draw(st.integers(min_value=0, max_value=100))
    hits = draw(st.integers(min_value=0, max_value=targets))
    distinct = draw(st.integers(min_value=hits, max_value=200))
    return InhabitationScore(targets=targets, created_hits=hits, created_distinct=distinct, missing=targets - hits)


@st.composite
def package_result_strategy(draw):
    return InhabitPackageResult(
        package_id=draw(st.text(min_size=1, max_size=66)),
        score=draw(inhabitation_score_strategy()),
        error=draw(st.one_of(st.none(), st.text())),
        elapsed_seconds=draw(st.one_of(st.none(), st.floats(min_value=0, max_value=3600))),
        timed_out=draw(st.one_of(st.none(), st.booleans())),
        created_object_types_list=draw(st.one_of(st.none(), st.lists(st.text()))),
        simulation_mode=draw(st.one_of(st.none(), st.sampled_from(["dry-run", "dev-inspect"]))),
        fell_back_to_dev_inspect=draw(st.one_of(st.none(), st.booleans())),
        ptb_parse_ok=draw(st.one_of(st.none(), st.booleans())),
        tx_build_ok=draw(st.one_of(st.none(), st.booleans())),
        dry_run_ok=draw(st.one_of(st.none(), st.booleans())),
        # ... and so on for all fields if needed, but let's focus on the critical ones
    )


@st.composite
def run_result_strategy(draw):
    packages = draw(st.lists(package_result_strategy(), min_size=0, max_size=10))
    return InhabitRunResult(
        schema_version=2,
        started_at_unix_seconds=draw(st.integers(min_value=0)),
        finished_at_unix_seconds=draw(st.integers(min_value=0)),
        corpus_root_name=draw(st.text()),
        samples=len(packages),
        seed=draw(st.integers()),
        agent=draw(st.text()),
        rpc_url=draw(st.text()),
        sender=draw(st.text()),
        gas_budget=draw(st.integers()),
        gas_coin=draw(st.one_of(st.none(), st.text())),
        aggregate={"avg_hit_rate": 0.0, "errors": 0},
        packages=[_to_package_dict(p) for p in packages],
    )


# --- Tests ---


@given(run_result_strategy())
@settings(deadline=None)
def test_schema_serialization_roundtrip(run_result: InhabitRunResult):
    """Invariant: Serializing a result and validating it must always succeed."""
    data = asdict(run_result)

    # 1. Must pass schema validation
    validate_phase2_run_json(data)

    # 2. Must be JSON serializable
    json_str = json.dumps(data)
    reloaded = json.loads(json_str)

    # 3. Structural integrity
    assert reloaded["schema_version"] == run_result.schema_version
    assert len(reloaded["packages"]) == len(run_result.packages)

    if len(run_result.packages) > 0:
        # Check first package key consistency
        pkg_data = reloaded["packages"][0]
        assert "package_id" in pkg_data
        assert "score" in pkg_data
        assert isinstance(pkg_data["score"], dict)


def test_to_package_dict_completeness():
    """Verify that _to_package_dict actually includes all InhabitPackageResult fields.

    This is a meta-test to catch when a developer adds a field to the dataclass
    but forgets to update the serialization helper.
    """
    import dataclasses

    from smi_bench.inhabit_runner import InhabitPackageResult

    # Get fields of the dataclass
    fields = {f.name for f in dataclasses.fields(InhabitPackageResult)}

    # Create a dummy result
    dummy = InhabitPackageResult(package_id="0x1", score=InhabitationScore(0, 0, 0, 0))

    # Get keys produced by helper
    serialized_keys = set(_to_package_dict(dummy).keys())

    # Logic: every field in the dataclass should be represented in the dictionary
    # (assuming we want 1:1 mapping for results)
    missing = fields - serialized_keys
    assert not missing, f"Fields missing from serialization: {missing}"
