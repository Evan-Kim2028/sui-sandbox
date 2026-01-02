from __future__ import annotations

import httpx

from smi_bench.agents.real_agent import RealAgent, RealAgentConfig, load_real_agent_config


def test_load_real_agent_config_prefers_os_env_over_dotenv(monkeypatch) -> None:
    monkeypatch.setenv("SMI_API_KEY", "from_os_env")
    cfg = load_real_agent_config(
        {
            "SMI_API_KEY": "from_dotenv",
            "SMI_API_BASE_URL": "https://example.invalid/v1",
            "SMI_MODEL": "glm-4.7",
        }
    )
    assert cfg.api_key == "from_os_env"


def test_load_real_agent_config_supports_fallback_keys_in_dotenv(monkeypatch) -> None:
    monkeypatch.delenv("SMI_API_KEY", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    cfg = load_real_agent_config(
        {
            "OPENAI_API_KEY": "from_dotenv_openai_key",
            "SMI_API_BASE_URL": "https://example.invalid/v1",
            "SMI_MODEL": "glm-4.7",
        }
    )
    assert cfg.api_key == "from_dotenv_openai_key"


def test_real_agent_openai_compatible_parses_json_array() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.method == "POST"
        assert request.url == httpx.URL("https://api.z.ai/v1/chat/completions")
        assert request.headers.get("authorization", "").startswith("Bearer ")
        return httpx.Response(
            200,
            json={
                "choices": [
                    {
                        "message": {
                            "content": '["0x1::m::S","0x2::n::T"]',
                        }
                    }
                ]
            },
        )

    transport = httpx.MockTransport(handler)
    client = httpx.Client(transport=transport)
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="test",
        base_url="https://api.z.ai/v1",
        model="glm-4.7",
        temperature=0.0,
        max_tokens=16,
        thinking=None,
        response_format=None,
        clear_thinking=None,
    )
    agent = RealAgent(cfg, client=client)
    out = agent.complete_type_list("return []")
    assert out == {"0x1::m::S", "0x2::n::T"}


def test_real_agent_sends_response_format_json_object() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        body = request.read().decode("utf-8")
        assert '"response_format"' in body
        assert '"json_object"' in body
        return httpx.Response(
            200,
            json={
                "choices": [
                    {
                        "message": {
                            "content": '{"key_types": ["0x1::m::S"]}',
                        }
                    }
                ]
            },
        )

    transport = httpx.MockTransport(handler)
    client = httpx.Client(transport=transport)
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="test",
        base_url="https://api.z.ai/api/coding/paas/v4",
        model="glm-4.7",
        temperature=0.0,
        max_tokens=16,
        thinking="enabled",
        response_format="json_object",
        clear_thinking=True,
    )
    agent = RealAgent(cfg, client=client)
    out = agent.complete_type_list('return {"key_types": []}')
    assert out == {"0x1::m::S"}


def test_real_agent_retries_on_429() -> None:
    calls = {"n": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        if calls["n"] == 1:
            return httpx.Response(429, headers={"retry-after": "0"})
        return httpx.Response(
            200,
            json={
                "choices": [
                    {
                        "message": {
                            "content": "[]",
                        }
                    }
                ]
            },
        )

    transport = httpx.MockTransport(handler)
    client = httpx.Client(transport=transport)
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="test",
        base_url="https://api.z.ai/api/paas/v4",
        model="glm-4.7",
        temperature=0.0,
        max_tokens=16,
        thinking=None,
        response_format=None,
        clear_thinking=None,
    )
    agent = RealAgent(cfg, client=client)
    out = agent.complete_type_list("return []")
    assert out == set()
    assert calls["n"] >= 2


def test_real_agent_complete_json_parses_object() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.method == "POST"
        return httpx.Response(
            200,
            json={
                "choices": [
                    {
                        "message": {
                            "content": (
                                '{"calls": [{"target": "0x2::tx_context::sender", "type_args": [], "args": []}]}'
                            ),
                        }
                    }
                ]
            },
        )

    transport = httpx.MockTransport(handler)
    client = httpx.Client(transport=transport)
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="test",
        base_url="https://api.z.ai/api/coding/paas/v4",
        model="glm-4.7",
        temperature=0.0,
        max_tokens=16,
        thinking=None,
        response_format="json_object",
        clear_thinking=None,
    )
    agent = RealAgent(cfg, client=client)
    out = agent.complete_json("return {}")
    assert isinstance(out, dict)
    assert "calls" in out


def test_real_agent_complete_json_retries_on_429() -> None:
    calls = {"n": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        if calls["n"] == 1:
            return httpx.Response(429, headers={"retry-after": "0"})
        return httpx.Response(
            200,
            json={"choices": [{"message": {"content": '{"calls": []}'}}]},
        )

    transport = httpx.MockTransport(handler)
    client = httpx.Client(transport=transport)
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="test",
        base_url="https://api.z.ai/api/coding/paas/v4",
        model="glm-4.7",
        temperature=0.0,
        max_tokens=16,
        thinking=None,
        response_format="json_object",
        clear_thinking=None,
    )
    agent = RealAgent(cfg, client=client)
    out = agent.complete_json("return {}")
    assert out == {"calls": []}
    assert calls["n"] >= 2


def test_real_agent_complete_json_requires_object() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={"choices": [{"message": {"content": "[]"}}]},
        )

    transport = httpx.MockTransport(handler)
    client = httpx.Client(transport=transport)
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="test",
        base_url="https://api.z.ai/api/coding/paas/v4",
        model="glm-4.7",
        temperature=0.0,
        max_tokens=16,
        thinking=None,
        response_format=None,
        clear_thinking=None,
    )
    agent = RealAgent(cfg, client=client)
    try:
        _ = agent.complete_json("return []")
        assert False, "expected ValueError"
    except ValueError as e:
        assert "JSON object" in str(e)
