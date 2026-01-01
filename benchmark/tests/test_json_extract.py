from __future__ import annotations

import pytest

from smi_bench.json_extract import JsonExtractError, extract_type_list


def test_extract_type_list_plain_array() -> None:
    s = '["0x1::m::S","0x2::n::T"]'
    out = extract_type_list(s)
    assert "0x1::m::S" in out and "0x2::n::T" in out


def test_extract_type_list_code_fence() -> None:
    s = "```json\n[\"0x1::m::S\"]\n```"
    out = extract_type_list(s)
    assert out == {"0x1::m::S"}


def test_extract_type_list_object_form() -> None:
    s = '{"key_types":["0x1::m::S"]}'
    out = extract_type_list(s)
    assert out == {"0x1::m::S"}


def test_extract_type_list_rejects_no_json() -> None:
    with pytest.raises(JsonExtractError):
        extract_type_list("hello world")

