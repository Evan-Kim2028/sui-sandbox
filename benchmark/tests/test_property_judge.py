"""Property-based tests for Phase I scoring logic using Hypothesis.

These tests verify invariants and mathematical properties of the scoring
algorithm that example-based tests might miss.
"""

from __future__ import annotations

from smi_bench.judge import score_key_types


def test_score_key_types_perfect_score() -> None:
    """Property: truth == predicted → F1=1.0."""
    from hypothesis import given
    from hypothesis import strategies as st
    from hypothesis.strategies import sets as st_sets

    @given(st_sets(st.just("0x1::m::S")))
    def check(truth_key_types):
        predicted_key_types = set(truth_key_types)
        score = score_key_types(truth_key_types, predicted_key_types)

        # Perfect prediction should have F1=1.0
        assert score.f1 == 1.0
        assert score.precision == 1.0
        assert score.recall == 1.0
        assert score.tp == len(truth_key_types)
        assert score.fp == 0
        assert score.fn == 0
        assert score.missing_sample == []
        assert score.extra_sample == []

    check()


def test_score_key_types_empty_prediction() -> None:
    """Property: empty prediction → F1=0.0."""
    from hypothesis import given
    from hypothesis import strategies as st
    from hypothesis.strategies import sets as st_sets

    @given(st_sets(st.just("0x1::m::S"), min_size=1))
    def check(truth_key_types):
        predicted_key_types: set[str] = set()
        score = score_key_types(truth_key_types, predicted_key_types)

        # Empty prediction should have F1=0.0
        assert score.f1 == 0.0
        assert score.precision == 0.0
        assert score.recall == 0.0
        assert score.tp == 0
        assert score.fp == 0
        assert score.fn == len(truth_key_types)
        # All truth types should be in missing
        assert len(score.missing_sample) == len(truth_key_types)

    check()


def test_score_key_types_symmetric_difference() -> None:
    """Property: symmetric difference |a-b| = fn + fp."""
    from hypothesis import given
    from hypothesis.strategies import sets as st_sets
    from hypothesis.strategies import text as st_text

    @given(
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
    )
    def check(truth_key_types, predicted_key_types):
        score = score_key_types(truth_key_types, predicted_key_types)

        # Symmetric difference property: |A-B| = (A-B) U (B-A)
        # |A-B| size should equal fn + fp
        expected_size = len(truth_key_types - predicted_key_types) + len(predicted_key_types - truth_key_types)
        actual_size = len(score.missing_sample) + len(score.extra_sample)

        assert actual_size == expected_size

    check()


def test_score_key_types_precision_formula() -> None:
    """Property: precision = tp / (tp + fp)."""
    from hypothesis import given
    from hypothesis.strategies import sets as st_sets
    from hypothesis.strategies import text as st_text

    @given(
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
    )
    def check(truth_key_types, predicted_key_types):
        score = score_key_types(truth_key_types, predicted_key_types)

        if score.tp + score.fp == 0:
            # Avoid division by zero
            return

        expected_precision = score.tp / (score.tp + score.fp)
        # Allow small floating point differences
        assert abs(score.precision - expected_precision) < 1e-9

    check()


def test_score_key_types_recall_formula() -> None:
    """Property: recall = tp / (tp + fn)."""
    from hypothesis import given
    from hypothesis.strategies import sets as st_sets
    from hypothesis.strategies import text as st_text

    @given(
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
    )
    def check(truth_key_types, predicted_key_types):
        score = score_key_types(truth_key_types, predicted_key_types)

        if score.tp + score.fn == 0:
            # Avoid division by zero
            return

        expected_recall = score.tp / (score.tp + score.fn)
        # Allow small floating point differences
        assert abs(score.recall - expected_recall) < 1e-9

    check()


def test_score_key_types_f1_formula() -> None:
    """Property: F1 = 2*pr*rc / (pr+rc)."""
    from hypothesis import given
    from hypothesis.strategies import sets as st_sets
    from hypothesis.strategies import text as st_text

    @given(
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
    )
    def check(truth_key_types, predicted_key_types):
        score = score_key_types(truth_key_types, predicted_key_types)

        if score.precision + score.recall == 0:
            # Avoid division by zero
            return

        expected_f1 = (2 * score.precision * score.recall) / (score.precision + score.recall)
        # Allow small floating point differences
        assert abs(score.f1 - expected_f1) < 1e-9

    check()


def test_score_key_types_deterministic() -> None:
    """Property: same inputs → same outputs."""
    from hypothesis import given
    from hypothesis.strategies import sets as st_sets
    from hypothesis.strategies import text as st_text

    @given(
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
    )
    def check(truth_key_types, predicted_key_types):
        score1 = score_key_types(truth_key_types, predicted_key_types)
        score2 = score_key_types(truth_key_types, predicted_key_types)

        # Same inputs should produce identical outputs
        assert score1.f1 == score2.f1
        assert score1.precision == score2.precision
        assert score1.recall == score2.recall
        assert score1.tp == score2.tp
        assert score1.fp == score2.fp
        assert score1.fn == score2.fn
        assert score1.missing_sample == score2.missing_sample
        assert score1.extra_sample == score2.extra_sample

    check()


def test_score_key_types_subset_properties() -> None:
    """Property: subset relationships between truth and prediction."""
    from hypothesis import given
    from hypothesis.strategies import sets as st_sets
    from hypothesis.strategies import text as st_text

    @given(
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
    )
    def check(truth_key_types, predicted_key_types):
        score = score_key_types(truth_key_types, predicted_key_types)

        # Property: predicted ⊆ truth → fp = 0, fn = |truth| - |predicted|
        if predicted_key_types.issubset(truth_key_types):
            assert score.fp == 0
            expected_fn = len(truth_key_types) - len(predicted_key_types)
            assert score.fn == expected_fn
            assert score.extra_sample == []

        # Property: truth ⊆ predicted → fn = 0, fp = |predicted| - |truth|
        if truth_key_types.issubset(predicted_key_types):
            assert score.fn == 0
            expected_fp = len(predicted_key_types) - len(truth_key_types)
            assert score.fp == expected_fp
            assert score.missing_sample == []

    check()


def test_score_key_types_bounds() -> None:
    """Property: F1, precision, recall ∈ [0, 1]."""
    from hypothesis import given
    from hypothesis.strategies import sets as st_sets
    from hypothesis.strategies import text as st_text

    @given(
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
    )
    def check(truth_key_types, predicted_key_types):
        score = score_key_types(truth_key_types, predicted_key_types)

        # All metrics should be in [0, 1]
        assert 0.0 <= score.f1 <= 1.0
        assert 0.0 <= score.precision <= 1.0
        assert 0.0 <= score.recall <= 1.0

    check()


def test_score_key_types_tp_fp_fn_properties() -> None:
    """Property: tp = |truth ∩ predicted|, fp = |predicted - truth|, fn = |truth - predicted|."""
    from hypothesis import given
    from hypothesis.strategies import sets as st_sets
    from hypothesis.strategies import text as st_text

    @given(
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
        st_sets(st_text(min_size=0), min_size=0, max_size=10),
    )
    def check(truth_key_types, predicted_key_types):
        score = score_key_types(truth_key_types, predicted_key_types)

        # Property: tp = |truth ∩ predicted|
        expected_tp = len(truth_key_types & predicted_key_types)
        assert score.tp == expected_tp

        # Property: fp = |predicted - truth|
        expected_fp = len(predicted_key_types - truth_key_types)
        assert score.fp == expected_fp

        # Property: fn = |truth - predicted|
        expected_fn = len(truth_key_types - predicted_key_types)
        assert score.fn == expected_fn

    check()
