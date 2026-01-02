"""Additional Property-based tests for Phase I scoring logic.

Focuses on edge cases and mathematical boundaries.
"""

from __future__ import annotations

import hypothesis.strategies as st
from hypothesis import given

from smi_bench.judge import score_key_types


@given(
    st.sets(st.text(min_size=1), min_size=0, max_size=50),
    st.sets(st.text(min_size=1), min_size=0, max_size=50),
)
def test_f1_score_is_always_between_precision_and_recall(truth, predicted):
    """Invariant: F1 score must lie between precision and recall (or be zero)."""
    score = score_key_types(truth, predicted)

    if score.precision == 0 or score.recall == 0:
        assert score.f1 == 0
    else:
        # F1 is the harmonic mean, which is always between the two values
        lower = min(score.precision, score.recall)
        upper = max(score.precision, score.recall)
        assert lower <= score.f1 <= upper


@given(
    st.sets(st.text(min_size=1), min_size=1, max_size=20),
)
def test_all_truth_captured_gives_perfect_recall(truth):
    """Invariant: If predicted is a superset of truth, recall must be 1.0."""
    # Add some "noise" to the prediction to make it a proper superset
    predicted = truth | {"0x999::noise::Type"}
    score = score_key_types(truth, predicted)
    assert score.recall == 1.0


@given(
    st.sets(st.text(min_size=1), min_size=1, max_size=20),
)
def test_all_predictions_correct_gives_perfect_precision(predicted):
    """Invariant: If truth is a superset of predicted, precision must be 1.0."""
    # truth includes everything predicted plus some extra
    truth = predicted | {"0x888::extra::Truth"}
    score = score_key_types(truth, predicted)
    assert score.precision == 1.0


@given(
    st.sets(st.text(min_size=1), min_size=1, max_size=10),
    st.sets(st.text(min_size=1), min_size=1, max_size=10),
    st.integers(min_value=1, max_value=5),
)
def test_max_samples_clipping(truth, predicted, max_s):
    """Invariant: samples lists must not exceed max_samples."""
    score = score_key_types(truth, predicted, max_samples=max_s)
    assert len(score.missing_sample) <= max_s
    assert len(score.extra_sample) <= max_s
