from __future__ import annotations

import os
from dataclasses import dataclass

import httpx

from smi_bench.json_extract import extract_type_list


@dataclass(frozen=True)
class RealAgentConfig:
    provider: str
    api_key: str
    base_url: str
    model: str
    temperature: float
    max_tokens: int


def _env_get(*keys: str) -> str | None:
    for k in keys:
        v = os.environ.get(k)
        if v:
            return v
    return None


def load_real_agent_config(env_overrides: dict[str, str] | None = None) -> RealAgentConfig:
    env_overrides = env_overrides or {}

    def get(k: str, *fallbacks: str) -> str | None:
        if k in env_overrides and env_overrides[k]:
            return env_overrides[k]
        return _env_get(k, *fallbacks)

    provider = get("SMI_PROVIDER") or "openai_compatible"

    api_key = get("SMI_API_KEY", "OPENAI_API_KEY", "ZAI_API_KEY", "ZHIPUAI_API_KEY")
    if not api_key:
        raise ValueError("missing API key (set SMI_API_KEY or OPENAI_API_KEY)")

    base_url = (
        get("SMI_API_BASE_URL", "OPENAI_BASE_URL", "OPENAI_API_BASE")
        or "https://api.openai.com/v1"
    )
    model = get("SMI_MODEL", "OPENAI_MODEL") or "gpt-4o-mini"

    temperature_s = get("SMI_TEMPERATURE") or "0"
    max_tokens_s = get("SMI_MAX_TOKENS") or "800"

    try:
        temperature = float(temperature_s)
    except Exception as e:
        raise ValueError(f"invalid SMI_TEMPERATURE={temperature_s}") from e
    try:
        max_tokens = int(max_tokens_s)
    except Exception as e:
        raise ValueError(f"invalid SMI_MAX_TOKENS={max_tokens_s}") from e

    return RealAgentConfig(
        provider=provider,
        api_key=api_key,
        base_url=base_url.rstrip("/"),
        model=model,
        temperature=temperature,
        max_tokens=max_tokens,
    )


class RealAgent:
    """
    Real LLM agent.

    Currently supports OpenAI-compatible chat completions via:
      POST {base_url}/chat/completions
    """

    def __init__(self, cfg: RealAgentConfig, client: httpx.Client | None = None) -> None:
        self.cfg = cfg
        self._client = client or httpx.Client(timeout=60)

        if cfg.provider != "openai_compatible":
            raise ValueError(f"unsupported provider: {cfg.provider}")

    def smoke(self) -> set[str]:
        """
        Minimal connectivity + parsing check. Returns a set (likely empty).
        """
        prompt = (
            "Return a JSON array of strings. For smoke testing, return an empty array: []"
        )
        return self.complete_type_list(prompt)

    def complete_type_list(self, prompt: str) -> set[str]:
        url = f"{self.cfg.base_url}/chat/completions"
        headers = {"Authorization": f"Bearer {self.cfg.api_key}"}
        payload = {
            "model": self.cfg.model,
            "temperature": self.cfg.temperature,
            "max_tokens": self.cfg.max_tokens,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a careful assistant. Output only valid JSON.",
                },
                {"role": "user", "content": prompt},
            ],
        }

        r = self._client.post(url, headers=headers, json=payload)
        r.raise_for_status()
        data = r.json()

        content = None
        try:
            content = data["choices"][0]["message"]["content"]
        except Exception:
            try:
                content = data["choices"][0]["text"]
            except Exception:
                raise ValueError("unexpected response shape (missing choices[0].message.content)")

        if not isinstance(content, str):
            raise ValueError("unexpected response content type")

        return extract_type_list(content)

