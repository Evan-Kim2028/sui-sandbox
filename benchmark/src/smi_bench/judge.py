from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class KeyTypeScore:
    tp: int
    fp: int
    fn: int
    precision: float
    recall: float
    f1: float
    missing_sample: list[str]
    extra_sample: list[str]


def score_key_types(
    truth: set[str],
    predicted: set[str],
    *,
    max_samples: int = 20,
) -> KeyTypeScore:
    if not truth and not predicted:
        return KeyTypeScore(
            tp=0,
            fp=0,
            fn=0,
            precision=1.0,
            recall=1.0,
            f1=1.0,
            missing_sample=[],
            extra_sample=[],
        )

    tp_set = truth & predicted
    fp_set = predicted - truth
    fn_set = truth - predicted

    tp = len(tp_set)
    fp = len(fp_set)
    fn = len(fn_set)

    precision = (tp / (tp + fp)) if (tp + fp) > 0 else 0.0
    recall = (tp / (tp + fn)) if (tp + fn) > 0 else 0.0
    # Compute F1 from counts to minimize floating-point drift in invariants.
    denom = 2 * tp + fp + fn
    f1 = (2 * tp / denom) if denom > 0 else 0.0

    missing_sample = sorted(fn_set)[:max_samples]
    extra_sample = sorted(fp_set)[:max_samples]

    return KeyTypeScore(
        tp=tp,
        fp=fp,
        fn=fn,
        precision=precision,
        recall=recall,
        f1=f1,
        missing_sample=missing_sample,
        extra_sample=extra_sample,
    )
