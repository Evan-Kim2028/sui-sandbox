from __future__ import annotations

from smi_bench.inhabit.metrics import compute_phase2_metrics


def test_compute_phase2_metrics_basic() -> None:
    rows = [
        {"dry_run_ok": True, "score": {"created_hits": 1, "targets": 2, "created_distinct": 3}},
        {"dry_run_ok": False, "score": {"created_hits": 0, "targets": 1, "created_distinct": 0}},
    ]
    m = compute_phase2_metrics(rows=rows, aggregate={})
    assert m.packages == 2
    assert m.dry_run_ok == 1
    assert m.any_hit == 1
    assert m.hits == 1
    assert m.targets == 3
    assert m.created_distinct_sum == 3
