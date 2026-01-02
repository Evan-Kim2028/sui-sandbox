from __future__ import annotations

from smi_bench.inhabit.score import canonical_base_type, extract_created_object_types, score_inhabitation


def test_base_type_strips_type_args() -> None:
    assert canonical_base_type("0x2::coin::Coin<0x2::sui::SUI>") == (
        "0x0000000000000000000000000000000000000000000000000000000000000002::coin::Coin"
    )
    assert canonical_base_type("0x2::object::UID") == (
        "0x0000000000000000000000000000000000000000000000000000000000000002::object::UID"
    )


def test_extract_created_object_types_from_object_changes() -> None:
    dev = {
        "effects": {
            "objectChanges": [
                {
                    "type": "created",
                    "objectType": "0x2::coin::Coin<0x2::sui::SUI>",
                },
                {
                    "type": "mutated",
                    "objectType": "0x2::object::UID",
                },
            ]
        }
    }
    out = extract_created_object_types(dev)
    assert out == {"0x2::coin::Coin<0x2::sui::SUI>"}


def test_extract_created_object_types_from_dry_run_shape() -> None:
    dev = {
        "objectChanges": [
            {
                "type": "created",
                "objectType": "0x2::coin::Coin<0x2::sui::SUI>",
            },
            {
                "type": "mutated",
                "objectType": "0x2::object::UID",
            },
        ]
    }
    out = extract_created_object_types(dev)
    assert out == {"0x2::coin::Coin<0x2::sui::SUI>"}


def test_score_inhabitation_matches_on_base_types() -> None:
    targets = {"0x2::coin::Coin", "0x2::object::UID"}
    created = {"0x2::coin::Coin<0x2::sui::SUI>", "0x2::random::Random"}
    s = score_inhabitation(target_key_types=targets, created_object_types=created)
    assert s.targets == 2
    assert s.created_distinct == 2  # Random + Coin (base)
    assert s.created_hits == 1
    assert s.missing == 1
