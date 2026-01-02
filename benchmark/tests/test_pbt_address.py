"""Property-based tests for address and type normalization.

Uses Hypothesis to verify internal invariants like idempotency and length stability.
"""

from __future__ import annotations

import hypothesis.strategies as st
from hypothesis import given

from smi_bench.inhabit.score import normalize_address, normalize_type_string


@st.composite
def sui_address(draw):
    """Generates a string that looks like a Sui address (0x followed by 1-64 hex chars)."""
    hex_chars = draw(st.text(alphabet="0123456789abcdefABCDEF", min_size=1, max_size=64))
    return f"0x{hex_chars}"


@given(sui_address())
def test_normalize_address_is_idempotent(addr: str) -> None:
    """Invariant: normalize(normalize(x)) == normalize(x)"""
    first = normalize_address(addr)
    second = normalize_address(first)
    assert first == second


@given(sui_address())
def test_normalize_address_length(addr: str) -> None:
    """Invariant: Normalized address must be 0x + 64 hex chars (66 total)."""
    norm = normalize_address(addr)
    assert len(norm) == 66
    assert norm.startswith("0x")


@given(st.text(alphabet="0123456789abcdefABCDEF", min_size=1, max_size=64))
def test_normalize_address_case_insensitivity(hex_part: str) -> None:
    """Invariant: normalize(0xABC) == normalize(0xabc)"""
    addr_upper = f"0x{hex_part.upper()}"
    addr_lower = f"0x{hex_part.lower()}"
    assert normalize_address(addr_upper) == normalize_address(addr_lower)


@given(st.text())
def test_normalize_address_preserves_non_hex_literals(text: str) -> None:
    """Invariant: normalize_address should return input if it doesn't start with 0x."""
    if not text.strip().lower().startswith("0x"):
        assert normalize_address(text) == text


@given(st.lists(sui_address(), min_size=1, max_size=5))
def test_normalize_type_string_preserves_structure(addresses: str) -> None:
    """Invariant: normalize_type_string pads all addresses in a complex type string."""
    # Construct a fake type string: 0x1::m::S<0x2::m::S>
    type_str = "::".join(addresses) + "<" + ",".join(addresses) + ">"
    norm = normalize_type_string(type_str)

    # Every 0x... in the output should be padded to 66 chars
    import re

    all_addr = re.findall(r"0x[0-9a-fA-F]+", norm)
    for a in all_addr:
        assert len(a) == 66
