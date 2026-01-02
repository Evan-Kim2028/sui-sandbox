from __future__ import annotations

import pytest

from smi_bench.inhabit_runner import _gas_budgets_to_try, _parse_gas_budget_ladder


def test_parse_gas_budget_ladder_empty() -> None:
    assert _parse_gas_budget_ladder("") == []
    assert _parse_gas_budget_ladder("   ") == []


def test_parse_gas_budget_ladder_parses_and_dedups() -> None:
    assert _parse_gas_budget_ladder("200, 500,200") == [200, 500]


def test_parse_gas_budget_ladder_rejects_invalid() -> None:
    with pytest.raises(ValueError):
        _parse_gas_budget_ladder("abc")


def test_gas_budgets_to_try_keeps_order_and_uniques() -> None:
    assert _gas_budgets_to_try(base=10, ladder=[20, 10, 30]) == [10, 20, 30]
