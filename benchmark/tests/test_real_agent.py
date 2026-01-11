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


def test_load_real_agent_config_env_overrides_win_over_process_env(monkeypatch) -> None:
    monkeypatch.setenv("SMI_API_BASE_URL", "https://api.z.ai/api/coding/paas/v4")
    cfg = load_real_agent_config(
        {"SMI_API_BASE_URL": "https://openrouter.ai/api/v1", "SMI_API_KEY": "k", "SMI_MODEL": "m"}
    )
    assert cfg.base_url == "https://api.z.ai/api/coding/paas/v4"


def test_real_agent_timeout_is_passed_to_httpx(monkeypatch) -> None:
    # Ensure we don't enforce an earlier internal deadline than the passed timeout.
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="test",
        base_url="https://openrouter.ai/api/v1",
        model="openai/gpt-4o-mini",
        temperature=0.0,
        max_tokens=None,
        thinking=None,
        response_format=None,
        clear_thinking=None,
        min_request_timeout_s=None,
        max_request_retries=1,
    )
    agent = RealAgent(cfg)

    class DummyResp:
        status_code = 200
        headers = {}

        def json(self):
            return {"choices": [{"message": {"content": '{"calls": []}'}}]}

        @property
        def text(self):
            return "ok"

        def raise_for_status(self):
            return None

    seen = {}

    def fake_post(url, headers=None, json=None, timeout=None):
        seen["timeout"] = timeout
        return DummyResp()

    monkeypatch.setattr(agent._client, "post", fake_post)
    agent.complete_json("{}", timeout_s=300)
    assert seen["timeout"] is not None


def test_real_agent_min_request_timeout_is_applied(monkeypatch) -> None:
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="test",
        base_url="https://openrouter.ai/api/v1",
        model="openai/gpt-4o-mini",
        temperature=0.0,
        max_tokens=None,
        thinking=None,
        response_format=None,
        clear_thinking=None,
        min_request_timeout_s=30.0,
        max_request_retries=1,
    )
    agent = RealAgent(cfg)

    class DummyResp:
        status_code = 200
        headers = {}

        def json(self):
            return {"choices": [{"message": {"content": '{"calls": []}'}}]}

        @property
        def text(self):
            return "ok"

        def raise_for_status(self):
            return None

    seen = {}

    def fake_post(url, headers=None, json=None, timeout=None):
        seen["timeout"] = timeout
        return DummyResp()

    monkeypatch.setattr(agent._client, "post", fake_post)
    agent.complete_json("{}", timeout_s=1)
    assert seen["timeout"] == 30.0


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
        min_request_timeout_s=None,
        max_request_retries=None,
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
        min_request_timeout_s=None,
        max_request_retries=None,
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
        min_request_timeout_s=None,
        max_request_retries=None,
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
        min_request_timeout_s=None,
        max_request_retries=None,
    )
    agent = RealAgent(cfg, client=client)
    resp = agent.complete_json("return {}")
    assert isinstance(resp.content, dict)
    assert "calls" in resp.content
    # Verify usage is captured (even if zeros when not in response)
    assert resp.usage.prompt_tokens >= 0


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
        min_request_timeout_s=None,
        max_request_retries=None,
    )
    agent = RealAgent(cfg, client=client)
    resp = agent.complete_json("return {}")
    assert resp.content == {"calls": []}
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
        min_request_timeout_s=None,
        max_request_retries=None,
    )
    agent = RealAgent(cfg, client=client)
    try:
        _ = agent.complete_json("return []")
        assert False, "expected ValueError"
    except ValueError as e:
        assert "JSON object" in str(e)


def test_load_real_agent_config_supports_openrouter_api_key(monkeypatch) -> None:
    """Test that OPENROUTER_API_KEY is recognized as a fallback."""
    monkeypatch.delenv("SMI_API_KEY", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    cfg = load_real_agent_config(
        {
            "OPENROUTER_API_KEY": "sk-or-v1-test-key",
            "SMI_API_BASE_URL": "https://openrouter.ai/api/v1",
            "SMI_MODEL": "anthropic/claude-sonnet-4",
        }
    )
    assert cfg.api_key == "sk-or-v1-test-key"
    assert cfg.model == "anthropic/claude-sonnet-4"


def test_real_agent_detects_openrouter() -> None:
    """Test that OpenRouter detection works correctly."""
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="sk-or-v1-test",
        base_url="https://openrouter.ai/api/v1",
        model="anthropic/claude-sonnet-4",
        temperature=0.0,
        max_tokens=None,
        thinking=None,
        response_format=None,
        clear_thinking=None,
        min_request_timeout_s=None,
        max_request_retries=None,
    )
    agent = RealAgent(cfg)
    assert agent.is_openrouter is True
    assert "HTTP-Referer" in agent._client.headers
    assert "X-Title" in agent._client.headers


def test_real_agent_detects_reasoning_models() -> None:
    """Test that reasoning model detection works for various providers."""
    test_cases = [
        ("deepseek/deepseek-v3", True),
        ("openai/o1-preview", True),
        ("openai/o3-mini", True),
        ("z-ai/glm-4.7", True),
        ("qwen/qwen-2.5", True),
        ("anthropic/claude-sonnet-4", False),
        ("openai/gpt-4o", False),
    ]

    for model_name, expected_reasoning in test_cases:
        cfg = RealAgentConfig(
            provider="openai_compatible",
            api_key="test",
            base_url="https://openrouter.ai/api/v1",
            model=model_name,
            temperature=0.0,
            max_tokens=None,
            thinking=None,
            response_format=None,
            clear_thinking=None,
            min_request_timeout_s=None,
            max_request_retries=None,
        )
        agent = RealAgent(cfg)
        assert agent.is_reasoning_model == expected_reasoning, f"Failed for model: {model_name}"


def test_real_agent_openrouter_sends_custom_headers() -> None:
    """Test that OpenRouter-specific headers are sent."""

    def handler(request: httpx.Request) -> httpx.Response:
        # Verify OpenRouter headers are present
        assert "HTTP-Referer" in request.headers
        assert "X-Title" in request.headers
        assert "sui-move-interface-extractor" in request.headers["HTTP-Referer"].lower()
        return httpx.Response(
            200,
            json={"choices": [{"message": {"content": '{"key_types": []}'}}]},
        )

    transport = httpx.MockTransport(handler)
    client = httpx.Client(transport=transport)
    cfg = RealAgentConfig(
        provider="openai_compatible",
        api_key="sk-or-v1-test",
        base_url="https://openrouter.ai/api/v1",
        model="anthropic/claude-sonnet-4",
        temperature=0.0,
        max_tokens=None,
        thinking=None,
        response_format=None,
        clear_thinking=None,
        min_request_timeout_s=None,
        max_request_retries=None,
    )
    agent = RealAgent(cfg, client=client)
    out = agent.complete_type_list("test prompt")
    assert out == set()
