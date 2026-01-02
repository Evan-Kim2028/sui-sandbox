from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Phase2Metrics:
    packages: int
    dry_run_ok: int
    any_hit: int
    hits: int
    targets: int
    created_distinct_sum: int
    macro_avg_hit_rate: float
    # New planning-focused metrics
    planning_only_packages: int = 0
    planning_only_hit_rate: float = 0.0
    formatting_only_failures: int = 0
    causality_valid_count: int = 0
    causality_success_rate: float = 0.0


def compute_phase2_metrics(*, rows: list[dict], aggregate: dict | None = None) -> Phase2Metrics:
    """
    Compute Phase II aggregate metrics from a run JSON's `packages[]` rows.

    Includes planning-only metrics that exclude pure formatting failures.
    """
    n = 0
    dry_run_ok = 0
    any_hit = 0
    hits = 0
    targets = 0
    created_distinct_sum = 0
    macro_sum = 0.0

    # Planning-only metrics
    planning_only_packages = 0
    planning_only_macro_sum = 0.0
    formatting_only_failures = 0
    causality_valid_count = 0

    for r in rows:
        if not isinstance(r, dict):
            continue
        score = r.get("score")
        if not isinstance(score, dict):
            continue
        n += 1
        if r.get("dry_run_ok") is True:
            dry_run_ok += 1
        h = int(score.get("created_hits", 0) or 0)
        t = int(score.get("targets", 0) or 0)
        cd = int(score.get("created_distinct", 0) or 0)
        hits += h
        targets += t
        created_distinct_sum += cd
        if h > 0:
            any_hit += 1
        pkg_hit_rate = (h / t) if t else 0.0
        macro_sum += pkg_hit_rate

        # Check for pure formatting failure (schema violations but no semantic failures)
        schema_violations = int(r.get("schema_violation_count", 0) or 0)
        semantic_failures = int(r.get("semantic_failure_count", 0) or 0)

        is_formatting_only_failure = schema_violations > 0 and semantic_failures == 0 and h == 0

        if is_formatting_only_failure:
            formatting_only_failures += 1
        else:
            # Include in planning-only metrics
            planning_only_packages += 1
            planning_only_macro_sum += pkg_hit_rate

        # Check causality validity
        causality_valid = r.get("causality_valid")
        if causality_valid is True:
            causality_valid_count += 1

    macro = (macro_sum / n) if n else 0.0
    planning_only_hit_rate = (planning_only_macro_sum / planning_only_packages) if planning_only_packages else 0.0
    causality_success_rate = (causality_valid_count / n) if n else 0.0

    # Prefer recorded macro if present (should match our computed macro).
    if isinstance(aggregate, dict):
        m = aggregate.get("avg_hit_rate")
        if isinstance(m, (int, float)):
            macro = float(m)

    return Phase2Metrics(
        packages=n,
        dry_run_ok=dry_run_ok,
        any_hit=any_hit,
        hits=hits,
        targets=targets,
        created_distinct_sum=created_distinct_sum,
        macro_avg_hit_rate=macro,
        planning_only_packages=planning_only_packages,
        planning_only_hit_rate=planning_only_hit_rate,
        formatting_only_failures=formatting_only_failures,
        causality_valid_count=causality_valid_count,
        causality_success_rate=causality_success_rate,
    )
