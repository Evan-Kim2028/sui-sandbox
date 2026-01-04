"""
Tests for new API endpoints: /validate, /schema, /info.

These tests verify the robustness improvements to the Docker API.
"""

import pytest
from starlette.testclient import TestClient

from smi_bench.a2a_green_agent import build_app


@pytest.fixture
def client():
    """Create test client for the A2A app."""
    app = build_app(public_url="http://localhost:9999/")
    return TestClient(app)


class TestValidateEndpoint:
    """Tests for POST /validate endpoint."""

    def test_validate_valid_config(self, client):
        """Valid config returns 200 with valid=true."""
        response = client.post(
            "/validate",
            json={
                "config": {
                    "corpus_root": "/app/corpus",
                    "package_ids_file": "/app/manifest.txt",
                }
            },
        )
        assert response.status_code == 200
        data = response.json()
        assert data["valid"] is True
        assert "config" in data
        assert data["config"]["corpus_root"] == "/app/corpus"
        assert data["warnings"] == []

    def test_validate_invalid_config_missing_required(self, client):
        """Missing required field returns 400 with valid=false."""
        response = client.post(
            "/validate",
            json={
                "config": {
                    "corpus_root": "/app/corpus",
                    # package_ids_file missing
                }
            },
        )
        assert response.status_code == 400
        data = response.json()
        assert data["valid"] is False
        assert "error" in data
        assert "package_ids_file" in data["error"]

    def test_validate_invalid_config_empty_required(self, client):
        """Empty required field returns 400."""
        response = client.post(
            "/validate",
            json={
                "config": {
                    "corpus_root": "",
                    "package_ids_file": "/app/manifest.txt",
                }
            },
        )
        assert response.status_code == 400
        data = response.json()
        assert data["valid"] is False
        assert "corpus_root" in data["error"]

    def test_validate_unknown_fields_produce_warnings(self, client):
        """Unknown fields are reported as warnings but config is still valid."""
        response = client.post(
            "/validate",
            json={
                "config": {
                    "corpus_root": "/app/corpus",
                    "package_ids_file": "/app/manifest.txt",
                    "typo_field": 123,
                    "another_unknown": "value",
                }
            },
        )
        assert response.status_code == 200
        data = response.json()
        assert data["valid"] is True
        assert len(data["warnings"]) > 0
        assert "typo_field" in data["warnings"][0]

    def test_validate_cross_field_validation(self, client):
        """Cross-field validation errors are caught."""
        response = client.post(
            "/validate",
            json={
                "config": {
                    "corpus_root": "/app/corpus",
                    "package_ids_file": "/app/manifest.txt",
                    "require_dry_run": True,
                    "simulation_mode": "dev-inspect",
                    "sender": "0x123",
                }
            },
        )
        assert response.status_code == 400
        data = response.json()
        assert data["valid"] is False
        assert "require_dry_run" in data["error"]

    def test_validate_missing_config_key(self, client):
        """Missing 'config' key in body returns 400."""
        response = client.post("/validate", json={"not_config": {"foo": "bar"}})
        assert response.status_code == 400
        data = response.json()
        assert "Missing 'config' field" in data["error"]

    def test_validate_invalid_json(self, client):
        """Invalid JSON body returns 400."""
        response = client.post("/validate", content="not json", headers={"Content-Type": "application/json"})
        assert response.status_code == 400

    def test_validate_returns_normalized_config(self, client):
        """Validation returns normalized config with defaults filled in."""
        response = client.post(
            "/validate",
            json={
                "config": {
                    "corpus_root": "/app/corpus",
                    "package_ids_file": "/app/manifest.txt",
                }
            },
        )
        assert response.status_code == 200
        data = response.json()
        config = data["config"]

        # Check defaults are filled in
        assert config["seed"] == 0
        assert config["gas_budget"] == 10_000_000
        assert config["max_errors"] == 25
        assert config["max_planning_calls"] == 50
        assert config["checkpoint_every"] == 10

    def test_validate_all_p0_fields(self, client):
        """Validate accepts all P0 fields."""
        response = client.post(
            "/validate",
            json={
                "config": {
                    "corpus_root": "/app/corpus",
                    "package_ids_file": "/app/manifest.txt",
                    "seed": 42,
                    "sender": "0xabc",
                    "gas_budget": 50_000_000,
                    "gas_coin": "0xgas",
                    "gas_budget_ladder": "10000000,20000000",
                    "max_errors": 10,
                    "max_run_seconds": 3600,
                }
            },
        )
        assert response.status_code == 200
        data = response.json()
        assert data["valid"] is True
        assert data["config"]["seed"] == 42
        assert data["config"]["gas_budget"] == 50_000_000

    def test_validate_all_p1_fields(self, client):
        """Validate accepts all P1 fields."""
        response = client.post(
            "/validate",
            json={
                "config": {
                    "corpus_root": "/app/corpus",
                    "package_ids_file": "/app/manifest.txt",
                    "max_planning_calls": 10,
                    "checkpoint_every": 5,
                    "max_heuristic_variants": 8,
                    "baseline_max_candidates": 100,
                    "include_created_types": True,
                    "require_dry_run": True,
                    "simulation_mode": "dry-run",
                }
            },
        )
        assert response.status_code == 200
        data = response.json()
        assert data["valid"] is True


class TestSchemaEndpoint:
    """Tests for GET /schema endpoint."""

    def test_schema_returns_valid_json_schema(self, client):
        """Schema endpoint returns valid JSON Schema."""
        response = client.get("/schema")
        assert response.status_code == 200
        schema = response.json()

        assert "$schema" in schema
        assert schema["type"] == "object"
        assert "properties" in schema
        assert "required" in schema

    def test_schema_has_required_fields(self, client):
        """Schema correctly marks required fields."""
        response = client.get("/schema")
        schema = response.json()

        assert "corpus_root" in schema["required"]
        assert "package_ids_file" in schema["required"]

    def test_schema_has_all_properties(self, client):
        """Schema includes all config properties."""
        response = client.get("/schema")
        props = response.json()["properties"]

        # Core fields
        assert "corpus_root" in props
        assert "package_ids_file" in props
        assert "samples" in props
        assert "agent" in props

        # P0 fields
        assert "seed" in props
        assert "sender" in props
        assert "gas_budget" in props
        assert "gas_coin" in props
        assert "gas_budget_ladder" in props
        assert "max_errors" in props
        assert "max_run_seconds" in props

        # P1 fields
        assert "max_planning_calls" in props
        assert "checkpoint_every" in props
        assert "max_heuristic_variants" in props
        assert "baseline_max_candidates" in props
        assert "include_created_types" in props
        assert "require_dry_run" in props

    def test_schema_property_types(self, client):
        """Schema properties have correct types."""
        response = client.get("/schema")
        props = response.json()["properties"]

        assert props["seed"]["type"] == "integer"
        assert props["gas_budget"]["type"] == "integer"
        assert props["max_run_seconds"]["type"] == ["number", "null"]
        assert props["include_created_types"]["type"] == "boolean"
        assert props["corpus_root"]["type"] == "string"

    def test_schema_has_defaults(self, client):
        """Schema properties include default values."""
        response = client.get("/schema")
        props = response.json()["properties"]

        assert props["seed"]["default"] == 0
        assert props["gas_budget"]["default"] == 10_000_000
        assert props["max_errors"]["default"] == 25

    def test_schema_disallows_additional_properties(self, client):
        """Schema enforces strict mode."""
        response = client.get("/schema")
        schema = response.json()

        assert schema["additionalProperties"] is False


class TestInfoEndpoint:
    """Tests for GET /info endpoint."""

    def test_info_returns_version(self, client):
        """Info endpoint returns version info."""
        response = client.get("/info")
        assert response.status_code == 200
        data = response.json()

        assert "version" in data
        assert "a2a_protocol_version" in data

    def test_info_returns_capabilities(self, client):
        """Info endpoint returns capability flags."""
        response = client.get("/info")
        data = response.json()

        assert "capabilities" in data
        caps = data["capabilities"]
        assert caps["streaming"] is True
        assert caps["cancellation"] is True
        assert caps["config_validation"] is True
        assert caps["schema_endpoint"] is True

    def test_info_returns_limits(self, client):
        """Info endpoint returns server limits."""
        response = client.get("/info")
        data = response.json()

        assert "limits" in data
        assert "max_concurrent_tasks" in data["limits"]
        assert isinstance(data["limits"]["max_concurrent_tasks"], int)

    def test_info_returns_endpoints(self, client):
        """Info endpoint lists available endpoints."""
        response = client.get("/info")
        data = response.json()

        assert "endpoints" in data
        endpoints = data["endpoints"]
        assert "health" in endpoints
        assert "validate" in endpoints
        assert "schema" in endpoints
        assert "info" in endpoints


class TestHealthEndpoint:
    """Tests for GET /health endpoint."""

    def test_health_returns_status(self, client):
        """Health endpoint returns status object."""
        response = client.get("/health")
        # May be 200 or 503 depending on binary availability
        assert response.status_code in [200, 503]
        data = response.json()

        assert "status" in data
        assert data["status"] in ["ok", "degraded"]

    def test_health_includes_binary_status(self, client):
        """Health endpoint includes binary availability."""
        response = client.get("/health")
        data = response.json()

        assert "binaries" in data
        assert "extractor" in data["binaries"]
        assert "simulator" in data["binaries"]

    def test_health_includes_executor_status(self, client):
        """Health endpoint includes executor status."""
        response = client.get("/health")
        data = response.json()

        assert "executor" in data
        assert "active_tasks" in data["executor"]


class TestA2AVersionHeader:
    """Test A2A-Version header is present on all responses."""

    def test_health_has_a2a_version(self, client):
        response = client.get("/health")
        assert "A2A-Version" in response.headers

    def test_validate_has_a2a_version(self, client):
        response = client.post("/validate", json={"config": {}})
        assert "A2A-Version" in response.headers

    def test_schema_has_a2a_version(self, client):
        response = client.get("/schema")
        assert "A2A-Version" in response.headers

    def test_info_has_a2a_version(self, client):
        response = client.get("/info")
        assert "A2A-Version" in response.headers
