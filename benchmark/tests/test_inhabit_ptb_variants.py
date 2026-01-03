from __future__ import annotations

import json

from smi_bench.inhabit.engine import ptb_variants


def test_ptb_variants_deterministic_and_bounded() -> None:
    base = {
        "calls": [
            {
                "target": "0x1::m::f",
                "type_args": [],
                "args": [
                    {"address": "0x2"},
                    {"u64": 1},
                    {"vector_address": ["0x3", "0x4"]},
                ],
            }
        ]
    }

    v1 = ptb_variants(base, sender="0xabc", max_variants=10)
    v2 = ptb_variants(base, sender="0xabc", max_variants=10)

    assert [n for n, _ in v1] == [n for n, _ in v2]
    assert [json.dumps(s, sort_keys=True) for _, s in v1] == [json.dumps(s, sort_keys=True) for _, s in v2]

    # base plan must not be mutated
    assert base["calls"][0]["args"][0]["address"] == "0x2"
    assert base["calls"][0]["args"][1]["u64"] == 1
    assert base["calls"][0]["args"][2]["vector_address"] == ["0x3", "0x4"]

    names = [n for n, _ in v1]
    assert names[0] == "base"
    assert "addr_sender" in names
    assert "ints_0" in names
    assert "ints_2" in names
    assert "ints_10" in names

    addr_sender = dict(v1)["addr_sender"]
    assert addr_sender["calls"][0]["args"][0]["address"] == "0xabc"
    assert addr_sender["calls"][0]["args"][2]["vector_address"] == ["0xabc", "0xabc"]

    ints_0 = dict(v1)["ints_0"]
    assert ints_0["calls"][0]["args"][1]["u64"] == 0

    limited = ptb_variants(base, sender="0xabc", max_variants=2)
    assert len(limited) == 2
