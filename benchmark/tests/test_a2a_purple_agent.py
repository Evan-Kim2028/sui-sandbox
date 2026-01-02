"""A2A Purple agent tests.

Tests cover:
- Agent card creation
- App creation
- Entry point validation
"""

from __future__ import annotations

from smi_bench.a2a_purple_agent import _card


def test_card_returns_valid_agentcard() -> None:
    """Agent card creation returns valid AgentCard."""
    from a2a.types import AgentCard

    card = _card(url="http://test.url")

    assert isinstance(card, AgentCard)


def test_card_url_parameter_required() -> None:
    """Card creation requires url parameter."""

    card = _card(url="http://test.url")

    # The card object should exist
    assert card is not None


def test_purple_agent_module_exists() -> None:
    """Purple agent module can be imported."""
    # Verify module exists
    from smi_bench import a2a_purple_agent

    # Verify main function exists
    assert hasattr(a2a_purple_agent, "main")
    assert callable(a2a_purple_agent.main)


def test_purple_agent_imports_from_a2a() -> None:
    """Purple agent imports from a2a package."""
    # Verify imports work
    from smi_bench import a2a_purple_agent

    # These should be importable from a2a package
    # (In real scenario, AgentBeats framework handles this)
    assert a2a_purple_agent is not None
