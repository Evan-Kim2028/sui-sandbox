from __future__ import annotations

from smi_bench.judge import score_key_types


def test_score_key_types_perfect() -> None:
    truth = {"0x1::m::S", "0x1::m::T"}
    pred = {"0x1::m::S", "0x1::m::T"}
    s = score_key_types(truth, pred)
    assert s.tp == 2 and s.fp == 0 and s.fn == 0
    assert s.precision == 1.0 and s.recall == 1.0 and s.f1 == 1.0


def test_score_key_types_empty_pred() -> None:
    truth = {"0x1::m::S"}
    pred = set()
    s = score_key_types(truth, pred)
    assert s.tp == 0 and s.fp == 0 and s.fn == 1
    assert s.precision == 0.0 and s.recall == 0.0 and s.f1 == 0.0


def test_score_key_types_extra_pred() -> None:
    truth = {"0x1::m::S"}
    pred = {"0x1::m::S", "0x2::m::X"}
    s = score_key_types(truth, pred)
    assert s.tp == 1 and s.fp == 1 and s.fn == 0
    assert s.precision == 0.5 and s.recall == 1.0
