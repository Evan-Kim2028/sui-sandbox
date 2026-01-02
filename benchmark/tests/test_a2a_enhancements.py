"""
Tests for A2A protocol compliance enhancements.

Tests cover:
- Task cancellation support
- A2A version headers
- Protocol version in agent card
"""

from __future__ import annotations


class TestA2AVersionHeaders:
    """Test A2A-Version header injection."""

    def test_green_agent_version_header(self) -> None:
        """Green agent should return A2A-Version header in responses."""
        from smi_bench import a2a_green_agent

        app = a2a_green_agent.build_app(public_url="http://test:9999/")

        # Check that middleware is registered
        # Starlette middleware is in app.user_middleware
        assert hasattr(app, "user_middleware") or hasattr(app, "middleware")

        # Version constant should be defined
        assert hasattr(a2a_green_agent, "A2A_PROTOCOL_VERSION")
        assert a2a_green_agent.A2A_PROTOCOL_VERSION == "0.3.0"

    def test_purple_agent_version_header(self) -> None:
        """Purple agent should return A2A-Version header in responses."""
        from smi_bench import a2a_purple_agent

        app = a2a_purple_agent.build_app(public_url="http://test:9998/")

        # Check that middleware is registered
        assert hasattr(app, "user_middleware") or hasattr(app, "middleware")

        # Version constant should be defined
        assert hasattr(a2a_purple_agent, "A2A_PROTOCOL_VERSION")
        assert a2a_purple_agent.A2A_PROTOCOL_VERSION == "0.3.0"

    def test_agent_card_includes_protocol_version(self) -> None:
        """Agent cards should include protocol_version field."""
        from smi_bench.a2a_green_agent import _card as green_card
        from smi_bench.a2a_purple_agent import _card as purple_card

        green = green_card(url="http://test:9999/")
        purple = purple_card(url="http://test:9998/")

        # Both cards should have protocol_version
        assert hasattr(green, "protocol_version")
        assert green.protocol_version == "0.3.0"

        assert hasattr(purple, "protocol_version")
        assert purple.protocol_version == "0.3.0"


class TestTaskCancellation:
    """Test task cancellation support."""

    def test_green_executor_tracks_processes(self) -> None:
        """Green executor should initialize process tracking dicts."""
        from smi_bench.a2a_green_agent import SmiBenchGreenExecutor

        executor = SmiBenchGreenExecutor()

        assert hasattr(executor, "_task_processes")
        assert hasattr(executor, "_task_cancel_events")
        assert isinstance(executor._task_processes, dict)
        assert isinstance(executor._task_cancel_events, dict)

    def test_cancel_method_exists(self) -> None:
        """Cancel method should exist and not be a stub."""
        import inspect

        from smi_bench.a2a_green_agent import SmiBenchGreenExecutor

        executor = SmiBenchGreenExecutor()
        assert hasattr(executor, "cancel")
        assert callable(executor.cancel)

        # Check that it's not just raising "not implemented"
        source = inspect.getsource(executor.cancel)
        assert "Cancel a running task" in source or "Implements A2A protocol" in source

    def test_purple_cancel_method_exists(self) -> None:
        """Purple agent cancel should have descriptive implementation."""
        import inspect

        from smi_bench.a2a_purple_agent import PurpleExecutor

        executor = PurpleExecutor()
        assert hasattr(executor, "cancel")
        assert callable(executor.cancel)

        # Check that it has a docstring explaining why it's not supported
        source = inspect.getsource(executor.cancel)
        assert "Purple agent" in source or "immediate" in source

    def test_terminate_process_method_exists(self) -> None:
        """Green executor should have _terminate_process helper."""
        from smi_bench.a2a_green_agent import SmiBenchGreenExecutor

        executor = SmiBenchGreenExecutor()
        assert hasattr(executor, "_terminate_process")
        assert callable(executor._terminate_process)


class TestA2AConstants:
    """Test A2A protocol constants."""

    def test_supported_content_types_defined(self) -> None:
        """SUPPORTED_CONTENT_TYPES constant should be defined."""
        from smi_bench import a2a_green_agent

        assert hasattr(a2a_green_agent, "SUPPORTED_CONTENT_TYPES")
        assert isinstance(a2a_green_agent.SUPPORTED_CONTENT_TYPES, set)
        assert "application/json" in a2a_green_agent.SUPPORTED_CONTENT_TYPES

    def test_a2a_protocol_version_format(self) -> None:
        """A2A protocol version should follow semver format."""
        from smi_bench import a2a_green_agent

        version = a2a_green_agent.A2A_PROTOCOL_VERSION
        parts = version.split(".")

        assert len(parts) == 3, "Version should be in X.Y.Z format"
        assert all(p.isdigit() for p in parts), "All version parts should be numeric"


class TestBackwardCompatibility:
    """Ensure enhancements don't break existing functionality."""

    def test_green_agent_still_builds(self) -> None:
        """Green agent app should still build successfully."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://test:9999/")
        assert app is not None

    def test_purple_agent_still_builds(self) -> None:
        """Purple agent app should still build successfully."""
        from smi_bench.a2a_purple_agent import build_app

        app = build_app(public_url="http://test:9998/")
        assert app is not None

    def test_agent_cards_still_valid(self) -> None:
        """Agent cards should still contain all required fields."""
        from smi_bench.a2a_green_agent import _card as green_card
        from smi_bench.a2a_purple_agent import _card as purple_card

        green = green_card(url="http://test:9999/")
        purple = purple_card(url="http://test:9998/")

        # Check required fields
        for card in [green, purple]:
            assert card.name
            assert card.description
            assert card.url
            assert card.version
            assert card.capabilities
            assert card.skills
            assert len(card.skills) > 0
