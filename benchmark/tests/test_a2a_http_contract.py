"""HTTP contract tests for A2A protocol compliance.

These tests validate HTTP-level protocol compliance including:
- Response headers (A2A-Version, Content-Type)
- Status codes
- JSON-RPC 2.0 response structure
- Content-Type negotiation
"""

from __future__ import annotations

from starlette.testclient import TestClient


class TestA2AVersionHeaders:
    """Test A2A-Version header injection."""

    def test_agent_card_endpoint_has_version_header(self) -> None:
        """Agent card endpoint should return A2A-Version header."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        response = client.get("/.well-known/agent-card.json")

        assert response.status_code == 200
        assert "A2A-Version" in response.headers
        assert response.headers["A2A-Version"] == "0.3.0"
        assert response.headers["Content-Type"] == "application/json"

    def test_rpc_endpoint_has_version_header(self) -> None:
        """RPC endpoint should return A2A-Version header."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        # Send valid JSON-RPC request
        response = client.post(
            "/",
            json={
                "jsonrpc": "2.0",
                "method": "message/send",
                "params": {
                    "message": {
                        "role": "user",
                        "parts": [{"text": "test"}],
                    },
                },
                "id": 1,
            },
        )

        # JSON-RPC always returns 200, even for errors
        assert response.status_code == 200
        assert "A2A-Version" in response.headers
        assert response.headers["A2A-Version"] == "0.3.0"
        assert response.headers["Content-Type"] == "application/json"

    def test_purple_agent_has_version_header(self) -> None:
        """Purple agent should also return A2A-Version header."""
        from smi_bench.a2a_purple_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9998/")
        client = TestClient(app)

        response = client.get("/.well-known/agent-card.json")

        assert response.status_code == 200
        assert "A2A-Version" in response.headers
        assert response.headers["A2A-Version"] == "0.3.0"


class TestJSONRPCResponseStructure:
    """Test JSON-RPC 2.0 response structure compliance."""

    def test_success_response_structure(self) -> None:
        """Successful JSON-RPC response should have correct structure."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        # Send valid JSON-RPC request
        # Note: This will use the real executor, which may fail without proper config,
        # but we're only testing response structure, not execution success
        response = client.post(
            "/",
            json={
                "jsonrpc": "2.0",
                "method": "message/send",
                "params": {
                    "message": {
                        "role": "user",
                        "parts": [{"text": "test"}],
                    },
                },
                "id": 1,
            },
        )

        assert response.status_code == 200
        body = response.json()

        # JSON-RPC 2.0 structure
        assert "jsonrpc" in body
        assert body["jsonrpc"] == "2.0"
        assert "id" in body

        # Should have either result or error
        assert "result" in body or "error" in body

    def test_error_response_structure(self) -> None:
        """Error JSON-RPC response should have correct structure."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        # Send invalid JSON-RPC request (missing method)
        response = client.post(
            "/",
            json={
                "jsonrpc": "2.0",
                "id": 1,
                # Missing method
            },
        )

        assert response.status_code == 200  # JSON-RPC always 200
        body = response.json()

        # Error structure
        assert body["jsonrpc"] == "2.0"
        assert "error" in body
        assert "id" in body

        error = body["error"]
        assert "code" in error
        assert "message" in error
        assert isinstance(error["code"], int)
        assert isinstance(error["message"], str)

    def test_invalid_json_returns_error(self) -> None:
        """Invalid JSON should return JSON-RPC error."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        # Send invalid JSON
        response = client.post(
            "/",
            content="not json",
            headers={"Content-Type": "application/json"},
        )

        # Should still return 200 with JSON-RPC error
        assert response.status_code == 200
        body = response.json()
        assert "error" in body
        assert body["error"]["code"] == -32700  # Parse error


class TestContentTypeNegotiation:
    """Test Content-Type header handling."""

    def test_agent_card_content_type(self) -> None:
        """Agent card should return application/json."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        response = client.get("/.well-known/agent-card.json")

        assert response.headers["Content-Type"] == "application/json"

    def test_rpc_content_type(self) -> None:
        """RPC endpoint should accept and return application/json."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        response = client.post(
            "/",
            json={"jsonrpc": "2.0", "id": 1},
            headers={"Content-Type": "application/json"},
        )

        assert response.headers["Content-Type"] == "application/json"


class TestStatusCodes:
    """Test HTTP status code compliance."""

    def test_agent_card_returns_200(self) -> None:
        """Agent card endpoint should return 200."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        response = client.get("/.well-known/agent-card.json")
        assert response.status_code == 200

    def test_rpc_always_returns_200(self) -> None:
        """RPC endpoint should always return 200 (JSON-RPC spec)."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        # Valid request
        response = client.post("/", json={"jsonrpc": "2.0", "id": 1})
        assert response.status_code == 200

        # Invalid request (still 200, error in body)
        response = client.post("/", json={"invalid": "request"})
        assert response.status_code == 200

        # Malformed JSON (still 200, parse error in body)
        response = client.post("/", content="not json")
        assert response.status_code == 200

    def test_not_found_returns_404(self) -> None:
        """Non-existent endpoints should return 404."""
        from smi_bench.a2a_green_agent import build_app

        app = build_app(public_url="http://127.0.0.1:9999/")
        client = TestClient(app)

        response = client.get("/nonexistent")
        assert response.status_code == 404
