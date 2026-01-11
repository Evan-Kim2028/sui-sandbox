"""Unit tests for A2A Green and Purple agent internal logic.

These tests focus on configuration loading, card generation, and
utility functions within the agents.
"""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock

import pytest

from smi_bench.a2a_green_agent import _card as green_card
from smi_bench.a2a_green_agent import _extract_payload, _load_cfg
from smi_bench.a2a_purple_agent import _card as purple_card


def test_green_agent_card_generation():
    """Verify green agent card structure."""
    card = green_card(url="http://green:9999")
    assert card.name == "smi-bench-green"
    assert "run_phase2" in [s.id for s in card.skills]


def test_purple_agent_card_generation():
    """Verify purple agent card structure."""
    card = purple_card(url="http://purple:9998")
    assert card.name == "smi-bench-purple"


def test_green_agent_config_defaults(tmp_path: Path):
    """Verify config loading handles defaults correctly."""
    # Create a temporary manifest file
    manifest = tmp_path / "manifest.txt"
    manifest.write_text("0x1\n")

    raw = {"corpus_root": "/tmp/corpus", "package_ids_file": str(manifest)}
    cfg = _load_cfg(raw)
    assert cfg.rpc_url == "https://fullnode.mainnet.sui.io:443"
    assert cfg.max_plan_attempts == 2
    assert cfg.continue_on_error is True


def test_green_agent_config_validation():
    """Verify config loading raises on missing required fields."""
    from smi_bench.a2a_errors import InvalidConfigError

    with pytest.raises(InvalidConfigError, match="corpus_root - missing or empty"):
        _load_cfg({"samples": 1})


def test_payload_extraction_from_user_input():
    """Verify payload extraction from user_input (JSON string)."""
    context = MagicMock()
    context.get_user_input.return_value = '{"config": {"corpus_root": "xyz"}}'

    payload = _extract_payload(context)
    assert payload["config"]["corpus_root"] == "xyz"


def test_payload_extraction_from_metadata():
    """Verify payload extraction from metadata fallback."""
    context = MagicMock()
    context.get_user_input.return_value = None
    # Mocking the _params.metadata attribute
    context._params = MagicMock()
    context._params.metadata = {"config": {"corpus_root": "abc"}}

    payload = _extract_payload(context)
    assert payload["config"]["corpus_root"] == "abc"


def test_green_agent_summarization_handles_missing_file(tmp_path: Path):
    """Verify summarization handles non-existent output files."""
    from smi_bench.a2a_green_agent import _summarize_phase2_results

    metrics, errors = _summarize_phase2_results(tmp_path / "nonexistent.json")
    assert metrics == {}
    assert errors == []
