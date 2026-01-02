from __future__ import annotations

from pathlib import Path

from smi_bench.inhabit_runner import _load_ids_file_ordered, _load_plan_file, _resolve_sender_and_gas_coin


def test_load_ids_file_ignores_comments_and_blanks(tmp_path: Path) -> None:
    p = tmp_path / "ids.txt"
    p.write_text(
        "\n".join(
            [
                "# comment",
                "",
                "0x1",
                "  0x2  ",
            ]
        )
        + "\n"
    )
    out = _load_ids_file_ordered(p)
    assert out == ["0x1", "0x2"]


def test_load_plan_file_parses_mapping(tmp_path: Path) -> None:
    p = tmp_path / "plans.json"
    p.write_text('{"0x1": {"calls": []}, "0x2": {"calls": [{"target": "0x2::m::f"}]}}\n')
    out = _load_plan_file(p)
    assert set(out.keys()) == {"0x1", "0x2"}
    assert out["0x1"]["calls"] == []


def test_resolve_sender_and_gas_coin_prefers_cli_over_env() -> None:
    sender, gas_coin = _resolve_sender_and_gas_coin(
        sender="0x1",
        gas_coin="0x2",
        env_overrides={"SMI_SENDER": "0xenv", "SMI_GAS_COIN": "0xenvcoin"},
    )
    assert sender == "0x1"
    assert gas_coin == "0x2"


def test_resolve_sender_and_gas_coin_falls_back_to_env() -> None:
    sender, gas_coin = _resolve_sender_and_gas_coin(
        sender=None,
        gas_coin=None,
        env_overrides={"SMI_SENDER": "0xenv", "SMI_GAS_COIN": "0xenvcoin"},
    )
    assert sender == "0xenv"
    assert gas_coin == "0xenvcoin"
