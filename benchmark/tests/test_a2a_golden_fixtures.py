"""Golden fixture tests for A2A protocol responses.

These tests ensure that A2A responses match expected formats exactly,
providing a regression test suite for protocol compliance.
"""

from __future__ import annotations

import json
from pathlib import Path

# Path to golden fixtures
FIXTURES_DIR = Path(__file__).parent / "fixtures" / "a2a"


class TestAgentCardGoldenFixtures:
    """Test agent cards match golden fixtures."""

    def test_green_agent_card_matches_fixture(self) -> None:
        """Green agent card should match expected structure."""
        from smi_bench.a2a_green_agent import _card as green_card

        card = green_card(url="http://127.0.0.1:9999/")

        # Load golden fixture
        fixture_path = FIXTURES_DIR / "agent_card_green.json"
        expected = json.loads(fixture_path.read_text())

        # Convert card to dict for comparison
        card_dict = {
            "name": card.name,
            "description": card.description,
            "url": card.url,
            "version": card.version,
            "protocol_version": card.protocol_version,
            "provider": {
                "organization": card.provider.organization,
                "url": card.provider.url,
            },
            "default_input_modes": list(card.default_input_modes),
            "default_output_modes": list(card.default_output_modes),
            "capabilities": {
                "streaming": card.capabilities.streaming,
                "push_notifications": card.capabilities.push_notifications,
                "state_transition_history": card.capabilities.state_transition_history,
            },
            "skills": [
                {
                    "id": skill.id,
                    "name": skill.name,
                    "description": skill.description,
                    "tags": list(skill.tags),
                    "examples": list(skill.examples) if skill.examples else [],
                    "input_modes": list(skill.input_modes) if skill.input_modes else [],
                    "output_modes": list(skill.output_modes) if skill.output_modes else [],
                }
                for skill in card.skills
            ],
        }

        # Compare key fields (allowing URL to vary)
        assert card_dict["name"] == expected["name"]
        assert card_dict["description"] == expected["description"]
        assert card_dict["version"] == expected["version"]
        assert card_dict["protocol_version"] == expected["protocol_version"]
        assert card_dict["capabilities"] == expected["capabilities"]
        assert len(card_dict["skills"]) == len(expected["skills"])
        assert card_dict["skills"][0]["id"] == expected["skills"][0]["id"]

    def test_purple_agent_card_matches_fixture(self) -> None:
        """Purple agent card should match expected structure."""
        from smi_bench.a2a_purple_agent import _card as purple_card

        card = purple_card(url="http://127.0.0.1:9998/")

        # Load golden fixture
        fixture_path = FIXTURES_DIR / "agent_card_purple.json"
        expected = json.loads(fixture_path.read_text())

        # Convert card to dict for comparison
        card_dict = {
            "name": card.name,
            "description": card.description,
            "url": card.url,
            "version": card.version,
            "protocol_version": card.protocol_version,
            "provider": {
                "organization": card.provider.organization,
                "url": card.provider.url,
            },
            "default_input_modes": list(card.default_input_modes),
            "default_output_modes": list(card.default_output_modes),
            "capabilities": {
                "streaming": card.capabilities.streaming,
                "push_notifications": card.capabilities.push_notifications,
                "state_transition_history": card.capabilities.state_transition_history,
            },
            "skills": [
                {
                    "id": skill.id,
                    "name": skill.name,
                    "description": skill.description,
                    "tags": list(skill.tags),
                    "examples": list(skill.examples) if skill.examples else [],
                    "input_modes": list(skill.input_modes) if skill.input_modes else [],
                    "output_modes": list(skill.output_modes) if skill.output_modes else [],
                }
                for skill in card.skills
            ],
        }

        # Compare key fields
        assert card_dict["name"] == expected["name"]
        assert card_dict["description"] == expected["description"]
        assert card_dict["version"] == expected["version"]
        assert card_dict["protocol_version"] == expected["protocol_version"]
        assert card_dict["capabilities"] == expected["capabilities"]


class TestEvaluationBundleGoldenFixtures:
    """Test evaluation bundles match golden fixtures."""

    def test_minimal_bundle_structure(self) -> None:
        """Minimal bundle should have all required fields."""
        fixture_path = FIXTURES_DIR / "evaluation_bundle_minimal.json"
        bundle = json.loads(fixture_path.read_text())

        # Required fields per schema
        required_fields = [
            "schema_version",
            "spec_url",
            "benchmark",
            "run_id",
            "exit_code",
            "timings",
            "config",
            "metrics",
            "errors",
            "artifacts",
        ]

        for field in required_fields:
            assert field in bundle, f"Missing required field: {field}"

        # Validate schema version
        assert bundle["schema_version"] == 1
        assert bundle["spec_url"] == "smi-bench:evaluation_bundle:v1"

        # Validate timings structure
        timings = bundle["timings"]
        assert "started_at_unix_seconds" in timings
        assert "finished_at_unix_seconds" in timings
        assert "elapsed_seconds" in timings

        # Validate artifacts structure
        artifacts = bundle["artifacts"]
        assert "results_path" in artifacts
        assert "run_metadata_path" in artifacts
        assert "events_path" in artifacts

    def test_full_bundle_structure(self) -> None:
        """Full bundle should have all optional fields populated."""
        fixture_path = FIXTURES_DIR / "evaluation_bundle_full.json"
        bundle = json.loads(fixture_path.read_text())

        # Should have all required fields
        assert "schema_version" in bundle
        assert "config" in bundle
        assert "metrics" in bundle

        # Config should have optional fields
        config = bundle["config"]
        assert "per_package_timeout_seconds" in config
        assert "max_plan_attempts" in config
        assert "continue_on_error" in config
        assert "resume" in config

        # Metrics should have aggregate metrics
        metrics = bundle["metrics"]
        assert "avg_hit_rate" in metrics
        assert "packages_total" in metrics
        assert "errors" in metrics


class TestErrorResponseGoldenFixtures:
    """Test error responses match expected formats."""

    def test_invalid_request_error_format(self) -> None:
        """Invalid request error should match JSON-RPC 2.0 format."""
        fixture_path = FIXTURES_DIR / "error_response_invalid_request.json"
        error_response = json.loads(fixture_path.read_text())

        # JSON-RPC 2.0 error structure
        assert error_response["jsonrpc"] == "2.0"
        assert "error" in error_response
        assert "id" in error_response

        error = error_response["error"]
        assert "code" in error
        assert "message" in error
        assert error["code"] == -32600  # Invalid Request
        assert "Invalid Request" in error["message"]
