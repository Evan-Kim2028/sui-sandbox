from __future__ import annotations

import os
import time
from dataclasses import dataclass

import httpx

from smi_bench.json_extract import extract_json_value, extract_type_list


@dataclass(frozen=True)
class RealAgentConfig:
    provider: str
    api_key: str
    base_url: str
    model: str
    temperature: float
    max_tokens: int | None
    thinking: str | None
    response_format: str | None
    clear_thinking: bool | None


def _env_get(*keys: str) -> str | None:
    for k in keys:
        v = os.environ.get(k)
        if v:
            return v
    return None


def _parse_bool(v: str) -> bool:
    s = v.strip().lower()
    if s in ("1", "true", "t", "yes", "y", "on"):
        return True
    if s in ("0", "false", "f", "no", "n", "off"):
        return False
    raise ValueError(f"invalid bool: {v!r}")


def load_real_agent_config(env_overrides: dict[str, str] | None = None) -> RealAgentConfig:
    env_overrides = env_overrides or {}

    def get(k: str, *fallbacks: str) -> str | None:
        # Precedence: real environment > .env file > fallbacks
        for kk in (k, *fallbacks):
            v = _env_get(kk)
            if v:
                return v
        for kk in (k, *fallbacks):
            v = env_overrides.get(kk)
            if v:
                return v
        return None

    provider = get("SMI_PROVIDER") or "openai_compatible"

    api_key = get("SMI_API_KEY", "OPENAI_API_KEY", "ZAI_API_KEY", "ZHIPUAI_API_KEY")
    if not api_key:
        raise ValueError("missing API key (set SMI_API_KEY or OPENAI_API_KEY)")

    base_url = get("SMI_API_BASE_URL", "OPENAI_BASE_URL", "OPENAI_API_BASE") or "https://api.openai.com/v1"
    model = get("SMI_MODEL", "OPENAI_MODEL") or "gpt-4o-mini"

    temperature_s = get("SMI_TEMPERATURE") or "0"
    max_tokens_s = get("SMI_MAX_TOKENS")
    thinking_s = get("SMI_THINKING")
    response_format_s = get("SMI_RESPONSE_FORMAT")
    clear_thinking_s = get("SMI_CLEAR_THINKING")

    try:
        temperature = float(temperature_s)
    except Exception as e:
        raise ValueError(f"invalid SMI_TEMPERATURE={temperature_s}") from e

    max_tokens = None
    if max_tokens_s:
        try:
            val = int(max_tokens_s)
            if val > 0:
                max_tokens = val
        except Exception as e:
            raise ValueError(f"invalid SMI_MAX_TOKENS={max_tokens_s}") from e

    try:
        clear_thinking = _parse_bool(clear_thinking_s) if clear_thinking_s else None
    except Exception as e:
        raise ValueError(f"invalid SMI_CLEAR_THINKING={clear_thinking_s!r}") from e

    return RealAgentConfig(
        provider=provider,
        api_key=api_key,
        base_url=base_url.rstrip("/"),
        model=model,
        temperature=temperature,
        max_tokens=max_tokens,
        thinking=thinking_s,
        response_format=response_format_s,
        clear_thinking=clear_thinking,
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
            'Return a JSON object with a single field "key_types" which is a JSON array of strings. '
            'For smoke testing, return: {"key_types": []}'
        )
        return self.complete_type_list(prompt)

    def complete_type_list(self, prompt: str, *, timeout_s: float | None = None) -> set[str]:
        url = f"{self.cfg.base_url}/chat/completions"
        headers = {"Authorization": f"Bearer {self.cfg.api_key}"}
        payload = {
            "model": self.cfg.model,
            "temperature": self.cfg.temperature,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a careful assistant. Output only valid JSON.",
                },
                {"role": "user", "content": prompt},
            ],
        }
        if self.cfg.max_tokens is not None:
            payload["max_tokens"] = self.cfg.max_tokens
        if self.cfg.thinking:
            payload["thinking"] = {"type": self.cfg.thinking}
            if self.cfg.clear_thinking is not None:
                payload["thinking"]["clear_thinking"] = self.cfg.clear_thinking
        if self.cfg.response_format:
            if self.cfg.response_format != "json_object":
                raise ValueError(f"unsupported SMI_RESPONSE_FORMAT={self.cfg.response_format!r}")
            payload["response_format"] = {"type": "json_object"}

        backoff_s = 1.0
        last_exc: Exception | None = None
        last_status: int | None = None
        last_body_prefix: str | None = None
        deadline = (time.monotonic() + timeout_s) if timeout_s is not None else None

        def body_prefix(r: httpx.Response) -> str:
            try:
                t = r.text
            except Exception:
                return "<unavailable>"
            t = t.replace("\n", " ").replace("\r", " ")
            return t[:400]

        def extract_api_error(r: httpx.Response) -> str | None:
            try:
                data = r.json()
            except Exception:
                return None
            if isinstance(data, dict):
                err = data.get("error")
                if isinstance(err, dict):
                    code = err.get("code")
                    msg = err.get("message")
                    if isinstance(code, str) and isinstance(msg, str):
                        return f"{code}: {msg}"
                    if isinstance(msg, str):
                        return msg
            return None

        for attempt in range(6):
            if deadline is not None and time.monotonic() >= deadline:
                raise TimeoutError("per-call timeout exceeded")
            try:
                req_timeout = None
                if deadline is not None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("per-call timeout exceeded")
                    req_timeout = max(1.0, remaining)
                r = self._client.post(url, headers=headers, json=payload, timeout=req_timeout)
                last_status = r.status_code
                last_body_prefix = body_prefix(r)

                if r.status_code == 404:
                    raise RuntimeError(f"endpoint not found (404): {url}")

                if r.status_code in (401, 403):
                    api_err = extract_api_error(r)
                    msg = api_err or last_body_prefix or "<no body>"
                    raise RuntimeError(f"auth failed ({r.status_code}): {msg}")

                if r.status_code in (429, 500, 502, 503, 504):
                    if r.status_code == 429:
                        api_err = extract_api_error(r)
                        # Some providers use 429 for non-rate-limit errors (e.g., quota/billing).
                        if api_err and (
                            "Insufficient balance" in api_err
                            or "no resource package" in api_err
                            or api_err.startswith("1113:")
                        ):
                            hint = ""
                            if "api.z.ai/api/paas/v4" in self.cfg.base_url:
                                hint = " (if you are on the Z.AI GLM Coding Plan, try base_url=https://api.z.ai/api/coding/paas/v4)"
                            raise RuntimeError(f"provider quota/billing error: {api_err}{hint}")

                    retry_after = r.headers.get("retry-after")
                    if retry_after:
                        try:
                            sleep_s = float(retry_after)
                        except Exception:
                            sleep_s = backoff_s
                    else:
                        sleep_s = backoff_s
                    if deadline is not None:
                        remaining = deadline - time.monotonic()
                        if remaining <= 0:
                            raise TimeoutError("per-call timeout exceeded")
                        sleep_s = min(sleep_s, max(0.0, remaining))
                    time.sleep(sleep_s)
                    backoff_s = min(backoff_s * 2, 8.0)
                    continue
                r.raise_for_status()
                data = r.json()
                break
            except Exception as e:
                last_exc = e
                sleep_s = backoff_s
                if deadline is not None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("per-call timeout exceeded") from last_exc
                    sleep_s = min(sleep_s, max(0.0, remaining))
                time.sleep(sleep_s)
                backoff_s = min(backoff_s * 2, 8.0)
        else:
            extra = ""
            if last_status is not None:
                extra = f" last_status={last_status}"
            if last_body_prefix:
                extra += f" body={last_body_prefix}"
            raise RuntimeError(f"request failed after retries.{extra}") from last_exc

        content = None
        try:
            content = data["choices"][0]["message"]["content"]
        except Exception:
            try:
                content = data["choices"][0]["text"]
            except Exception as e:
                raise ValueError(f"unexpected response shape: {data}") from e

        if not isinstance(content, str):
            raise ValueError("unexpected response content type")

        if content.strip() == "":
            finish_reason = None
            try:
                finish_reason = data["choices"][0].get("finish_reason")
            except Exception:
                pass
            hint = ""
            if finish_reason == "length":
                hint = " (model hit max_tokens; increase SMI_MAX_TOKENS or reduce prompt size)"
            raise ValueError(f"model returned empty content{hint}")

        return extract_type_list(content)

    def complete_json(self, prompt: str, *, timeout_s: float | None = None) -> dict:
        """
        Request an OpenAI-compatible chat completion and parse the assistant content as a JSON object.

        This is intentionally strict: it must parse, and it must be a JSON object (dict).
        """
        url = f"{self.cfg.base_url}/chat/completions"
        headers = {"Authorization": f"Bearer {self.cfg.api_key}"}
        payload = {
            "model": self.cfg.model,
            "temperature": self.cfg.temperature,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a careful assistant. Output only valid JSON.",
                },
                {"role": "user", "content": prompt},
            ],
        }
        if self.cfg.max_tokens is not None:
            payload["max_tokens"] = self.cfg.max_tokens
        if self.cfg.thinking:
            payload["thinking"] = {"type": self.cfg.thinking}
            if self.cfg.clear_thinking is not None:
                payload["thinking"]["clear_thinking"] = self.cfg.clear_thinking
        if self.cfg.response_format:
            if self.cfg.response_format != "json_object":
                raise ValueError(f"unsupported SMI_RESPONSE_FORMAT={self.cfg.response_format!r}")
            payload["response_format"] = {"type": "json_object"}

        backoff_s = 1.0
        last_exc: Exception | None = None
        last_status: int | None = None
        last_body_prefix: str | None = None
        deadline = (time.monotonic() + timeout_s) if timeout_s is not None else None

        def body_prefix(r: httpx.Response) -> str:
            try:
                t = r.text
            except Exception:
                return "<unavailable>"
            t = t.replace("\n", " ").replace("\r", " ")
            return t[:400]

        def extract_api_error(r: httpx.Response) -> str | None:
            try:
                data = r.json()
            except Exception:
                return None
            if isinstance(data, dict):
                err = data.get("error")
                if isinstance(err, dict):
                    code = err.get("code")
                    msg = err.get("message")
                    if isinstance(code, str) and isinstance(msg, str):
                        return f"{code}: {msg}"
                    if isinstance(msg, str):
                        return msg
            return None

        for _attempt in range(6):
            if deadline is not None and time.monotonic() >= deadline:
                raise TimeoutError("per-call timeout exceeded")
            try:
                req_timeout = None
                if deadline is not None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("per-call timeout exceeded")
                    req_timeout = max(1.0, remaining)
                r = self._client.post(url, headers=headers, json=payload, timeout=req_timeout)
                last_status = r.status_code
                last_body_prefix = body_prefix(r)

                if r.status_code == 404:
                    raise RuntimeError(f"endpoint not found (404): {url}")

                if r.status_code in (401, 403):
                    api_err = extract_api_error(r)
                    msg = api_err or last_body_prefix or "<no body>"
                    raise RuntimeError(f"auth failed ({r.status_code}): {msg}")

                if r.status_code in (429, 500, 502, 503, 504):
                    if r.status_code == 429:
                        api_err = extract_api_error(r)
                        if api_err and (
                            "Insufficient balance" in api_err
                            or "no resource package" in api_err
                            or api_err.startswith("1113:")
                        ):
                            hint = ""
                            if "api.z.ai/api/paas/v4" in self.cfg.base_url:
                                hint = " (if you are on the Z.AI GLM Coding Plan, try base_url=https://api.z.ai/api/coding/paas/v4)"
                            raise RuntimeError(f"provider quota/billing error: {api_err}{hint}")

                    retry_after = r.headers.get("retry-after")
                    if retry_after:
                        try:
                            sleep_s = float(retry_after)
                        except Exception:
                            sleep_s = backoff_s
                    else:
                        sleep_s = backoff_s
                    if deadline is not None:
                        remaining = deadline - time.monotonic()
                        if remaining <= 0:
                            raise TimeoutError("per-call timeout exceeded")
                        sleep_s = min(sleep_s, max(0.0, remaining))
                    time.sleep(sleep_s)
                    backoff_s = min(backoff_s * 2, 8.0)
                    continue

                r.raise_for_status()
                data = r.json()
                break
            except Exception as e:
                last_exc = e
                sleep_s = backoff_s
                if deadline is not None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("per-call timeout exceeded") from last_exc
                    sleep_s = min(sleep_s, max(0.0, remaining))
                time.sleep(sleep_s)
                backoff_s = min(backoff_s * 2, 8.0)
        else:
            extra = ""
            if last_status is not None:
                extra = f" last_status={last_status}"
            if last_body_prefix:
                extra += f" body={last_body_prefix}"
            raise RuntimeError(f"request failed after retries.{extra}") from last_exc

        content = None
        try:
            content = data["choices"][0]["message"]["content"]
        except Exception:
            try:
                content = data["choices"][0]["text"]
            except Exception as e:
                raise ValueError(f"unexpected response shape: {data}") from e

        if not isinstance(content, str):
            raise ValueError("unexpected response content type")
        if content.strip() == "":
            raise ValueError(f"model returned empty content. full_response={data}")

        try:
            parsed = extract_json_value(content)
        except Exception as e:
            raise ValueError(f"model output was not valid JSON: {content[:300]!r}") from e

        if not isinstance(parsed, dict):
            raise ValueError(f"model output must be a JSON object, got {type(parsed)}")

        return parsed
