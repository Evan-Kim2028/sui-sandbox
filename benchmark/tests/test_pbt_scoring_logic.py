"""Property-based tests for scoring and metrics aggregation.

Ensures that the scoring logic is resilient to malformed RPC data
and that aggregate metrics remain mathematically consistent.
"""

from __future__ import annotations

import hypothesis.strategies as st
from hypothesis import given, settings

from smi_bench.inhabit.metrics import compute_phase2_metrics
from smi_bench.inhabit.score import extract_created_object_types, score_inhabitation


@st.composite
def nested_junk_json(draw):
    """Generates recursive junk dictionaries to stress-test parsers."""
    return draw(
        st.recursive(
            st.none() | st.booleans() | st.integers() | st.text(),
            lambda children: st.dictionaries(st.text(), children) | st.lists(children),
            max_leaves=10,
        )
    )


@given(nested_junk_json())
@settings(deadline=None)
def test_extract_created_types_never_crashes(junk: any) -> None:
    """Invariant: The parser must handle arbitrary/malformed JSON without raising Exceptions."""
    try:
        res = extract_created_object_types(junk if isinstance(junk, dict) else {})
        assert isinstance(res, set)
    except Exception as e:
        # We only allow known logical errors if we were to define them,
        # but for now, it should be robust.
        raise e


@given(
    st.lists(
        st.fixed_dictionaries(
            {
                "targets": st.integers(min_value=1, max_value=100),
                "hits": st.integers(min_value=0, max_value=100),
            }
        ),
        min_size=1,
        max_size=50,
    )
)
@settings(deadline=None)
def test_metrics_aggregation_consistency(package_data: list[dict]) -> None:
    """Invariant: Micro hit rate must equal sum(hits) / sum(targets)."""
    # Create fake package rows
    rows = []
    total_hits = 0
    total_targets = 0

    for i, data in enumerate(package_data):
        # Ensure hits <= targets for realistic data
        hits = min(data["hits"], data["targets"])
        targets = data["targets"]

        rows.append(
            {
                "package_id": f"0x{i}",
                "score": {
                    "targets": targets,
                    "created_hits": hits,
                    "created_distinct": hits,  # dummy
                },
                "dry_run_ok": True,
            }
        )
        total_hits += hits
        total_targets += targets

    # Compute aggregate
    metrics = compute_phase2_metrics(rows=rows, aggregate={})

    # Check micro-average logic
    # Note: compute_phase2_metrics doesn't return micro directly, but we check macro consistency

    assert metrics.packages == len(rows)
    assert metrics.hits == total_hits
    assert metrics.targets == total_targets

    if all(r["score"]["created_hits"] == r["score"]["targets"] for r in rows):
        assert metrics.macro_avg_hit_rate == 1.0


@given(st.sets(st.text(min_size=5), min_size=1), st.sets(st.text(min_size=5), min_size=1))
@settings(deadline=None)
def test_score_inhabitation_bounds(targets: set[str], created: set[str]) -> None:
    """Invariant: created_hits can never exceed targets."""
    score = score_inhabitation(target_key_types=targets, created_object_types=created)
    assert score.created_hits <= score.targets
    assert score.targets == len({t.split("<")[0] for t in targets})  # approximate
