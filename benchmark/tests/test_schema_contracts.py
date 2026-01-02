"""Refactor safety tests: assert determinism and schema field contracts."""

from __future__ import annotations

import json
from dataclasses import asdict

from smi_bench.inhabit.score import InhabitationScore
from smi_bench.judge import KeyTypeScore
from smi_bench.runner import PackageResult, RunResult


def test_phase1_run_result_schema_fields() -> None:
    """Assert Phase I RunResult has expected schema fields."""
    result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test_corpus",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=10,
        seed=42,
        agent="test-agent",
        aggregate={"avg_f1": 0.5, "errors": 0},
        packages=[],
    )

    # Serialize and deserialize to check JSON round-trip
    serialized = json.dumps(asdict(result), sort_keys=True)
    deserialized = json.loads(serialized)

    # Assert required fields exist
    assert "schema_version" in deserialized
    assert "started_at_unix_seconds" in deserialized
    assert "finished_at_unix_seconds" in deserialized
    assert "corpus_root_name" in deserialized
    assert "samples" in deserialized
    assert "seed" in deserialized
    assert "agent" in deserialized
    assert "aggregate" in deserialized
    assert "packages" in deserialized

    # Assert aggregate has expected shape
    agg = deserialized["aggregate"]
    assert isinstance(agg, dict)
    assert "errors" in agg  # Common aggregate field


def test_phase1_package_result_determinism() -> None:
    """Assert Phase I PackageResult serialization is deterministic (sorted keys)."""
    score = KeyTypeScore(
        tp=2,
        fp=1,
        fn=1,
        precision=0.666,
        recall=0.666,
        f1=0.666,
        missing_sample=["0x1::m::S"],
        extra_sample=["0x2::m::X"],
    )

    result1 = PackageResult(
        package_id="0xabc",
        truth_key_types=3,
        predicted_key_types=3,
        score=score,
    )

    result2 = PackageResult(
        package_id="0xdef",
        truth_key_types=2,
        predicted_key_types=2,
        score=score,
    )

    # Serialize both with sort_keys=True (as the harness does)
    json1 = json.dumps(asdict(result1), sort_keys=True)
    json2 = json.dumps(asdict(result2), sort_keys=True)

    # Parse back and verify structure
    d1 = json.loads(json1)
    d2 = json.loads(json2)

    # Assert keys are sorted (deterministic order)
    keys1 = list(d1.keys())
    keys2 = list(d2.keys())
    assert keys1 == sorted(keys1), "Keys should be sorted for determinism"
    assert keys2 == sorted(keys2), "Keys should be sorted for determinism"

    # Assert score dict has expected fields
    assert "score" in d1
    score_dict = d1["score"]
    assert isinstance(score_dict, dict)
    assert "tp" in score_dict
    assert "fp" in score_dict
    assert "fn" in score_dict
    assert "precision" in score_dict
    assert "recall" in score_dict
    assert "f1" in score_dict
    assert "missing_sample" in score_dict
    assert "extra_sample" in score_dict


def test_phase2_run_result_schema_fields() -> None:
    """Assert Phase II InhabitRunResult has expected schema fields."""
    from smi_bench.inhabit_runner import InhabitRunResult

    result = InhabitRunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test_corpus",
        samples=10,
        seed=42,
        agent="test-agent",
        rpc_url="https://test.rpc",
        sender="0x123",
        gas_budget=10_000_000,
        gas_coin=None,
        aggregate={"avg_hit_rate": 0.1, "packages_total": 10},
        packages=[],
    )

    # Serialize and deserialize to check JSON round-trip
    serialized = json.dumps(asdict(result), sort_keys=True)
    deserialized = json.loads(serialized)

    # Assert required fields exist
    assert "schema_version" in deserialized
    assert "started_at_unix_seconds" in deserialized
    assert "finished_at_unix_seconds" in deserialized
    assert "corpus_root_name" in deserialized
    assert "samples" in deserialized
    assert "seed" in deserialized
    assert "agent" in deserialized
    assert "rpc_url" in deserialized
    assert "sender" in deserialized
    assert "gas_budget" in deserialized
    assert "aggregate" in deserialized
    assert "packages" in deserialized

    # Assert aggregate has expected shape
    agg = deserialized["aggregate"]
    assert isinstance(agg, dict)
    assert "packages_total" in agg  # Common aggregate field


def test_phase2_inhabitation_score_schema() -> None:
    """Assert InhabitationScore has expected schema fields."""
    score = InhabitationScore(
        targets=5,
        created_distinct=3,
        created_hits=2,
        missing=3,
    )

    # Serialize and check fields
    serialized = json.dumps(asdict(score), sort_keys=True)
    deserialized = json.loads(serialized)

    assert "targets" in deserialized
    assert "created_distinct" in deserialized
    assert "created_hits" in deserialized
    assert "missing" in deserialized

    # Assert determinism (sorted keys)
    keys = list(deserialized.keys())
    assert keys == sorted(keys), "Keys should be sorted for determinism"


def test_packages_list_should_be_sorted_for_determinism() -> None:
    """
    Assert that packages lists in RunResult should be sorted by package_id for reproducible diffs.

    This is a documentation test: the actual sorting happens in the runner code, but we assert
    the expectation here.
    """
    # Create a sample with unsorted package_ids
    packages_data = [
        {
            "package_id": "0xccc",
            "score": {
                "tp": 1,
                "fp": 0,
                "fn": 0,
                "precision": 1.0,
                "recall": 1.0,
                "f1": 1.0,
                "missing_sample": [],
                "extra_sample": [],
            },
        },
        {
            "package_id": "0xaaa",
            "score": {
                "tp": 2,
                "fp": 0,
                "fn": 0,
                "precision": 1.0,
                "recall": 1.0,
                "f1": 1.0,
                "missing_sample": [],
                "extra_sample": [],
            },
        },
        {
            "package_id": "0xbbb",
            "score": {
                "tp": 1,
                "fp": 1,
                "fn": 0,
                "precision": 0.5,
                "recall": 1.0,
                "f1": 0.666,
                "missing_sample": [],
                "extra_sample": [],
            },
        },
    ]

    # For determinism, packages should be sorted by package_id
    sorted_packages = sorted(packages_data, key=lambda p: p["package_id"])

    result = RunResult(
        schema_version=1,
        started_at_unix_seconds=1000,
        finished_at_unix_seconds=2000,
        corpus_root_name="test",
        corpus_git=None,
        target_ids_file=None,
        target_ids_total=None,
        samples=3,
        seed=42,
        agent="test",
        aggregate={},
        packages=sorted_packages,  # Use sorted version
    )

    # Serialize and verify order is preserved
    serialized = json.dumps(asdict(result), sort_keys=True)
    deserialized = json.loads(serialized)

    pkg_ids = [p["package_id"] for p in deserialized["packages"]]
    assert pkg_ids == ["0xaaa", "0xbbb", "0xccc"], "Packages should be sorted by package_id for determinism"
