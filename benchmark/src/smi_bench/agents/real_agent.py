from __future__ import annotations

import json
import logging
import os
import time
from dataclasses import dataclass
from typing import Any, cast

import httpx

from smi_bench.json_extract import JsonExtractError, extract_json_value, extract_type_list
from smi_bench.logging import JsonlLogger
from smi_bench.utils import safe_parse_float, safe_parse_int

logger = logging.getLogger(__name__)


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
    min_request_timeout_s: float | None = None
    max_request_retries: int | None = None


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
        """
        Resolve config values across multiple possible env var keys.

        Semantics:
        - We have an ordered list of keys (k + fallbacks) in *priority order*.
        - We pick the *first key* that exists in either process env or env_overrides.
        - For that chosen key, process env wins over env_overrides (so operators can override
          a stable .env without editing files).
        """
        # Keys that should be overrideable only via process env (operator override).
        # This helps scripts/CI force values without editing .env files.
        operator_env_wins = {"SMI_API_KEY", "SMI_API_BASE_URL"}

        for kk in (k, *fallbacks):
            v_env = _env_get(kk)
            v_override = env_overrides.get(kk)
            if not (v_env or v_override):
                continue
            if kk in operator_env_wins:
                return v_env or v_override
            return v_override or v_env
        return None

    def get_api_key() -> str | None:
        """
        Resolve API keys with a policy that avoids ambient-provider leakage during tests.

        Rules:
        - If SMI_API_KEY is set in process env, it wins (operator override).
        - Otherwise, if SMI_API_KEY is set in env_overrides, use it.
        - For other provider keys, explicit env_overrides wins over process env so that a passed
          dotenv/config dict can deterministically select a provider even if the environment has
          another provider key present (common in dev shells / CI).
        """
        # Highest-priority "global" key
        v = _env_get("SMI_API_KEY")
        if v:
            return v
        v = env_overrides.get("SMI_API_KEY")
        if v:
            return v

        # Provider-specific fallbacks (prefer overrides over env for determinism)
        for k in ("OPENAI_API_KEY", "OPENROUTER_API_KEY", "ZAI_API_KEY", "ZHIPUAI_API_KEY"):
            v = env_overrides.get(k)
            if v:
                return v
            v = _env_get(k)
            if v:
                return v
        return None

    provider = get("SMI_PROVIDER") or "openai_compatible"

    api_key = get_api_key()
    if not api_key:
        raise ValueError("missing API key (set SMI_API_KEY, OPENROUTER_API_KEY, or OPENAI_API_KEY)")

    base_url = (
        get("SMI_API_BASE_URL", "OPENROUTER_BASE_URL", "OPENAI_BASE_URL", "OPENAI_API_BASE")
        or "https://api.openai.com/v1"
    )
    model = get("SMI_MODEL", "OPENAI_MODEL")
    if not model:
        raise ValueError("missing model (set SMI_MODEL or OPENAI_MODEL)")

    temperature_s = get("SMI_TEMPERATURE")
    max_tokens_s = get("SMI_MAX_TOKENS")
    thinking_s = get("SMI_THINKING")
    response_format_s = get("SMI_RESPONSE_FORMAT")
    clear_thinking_s = get("SMI_CLEAR_THINKING")
    min_request_timeout_s = get("SMI_MIN_REQUEST_TIMEOUT_SECONDS", "SMI_MIN_REQUEST_TIMEOUT_S")
    max_request_retries_s = get("SMI_MAX_REQUEST_RETRIES")

    temperature = safe_parse_float(temperature_s, 0.0, min_val=0.0, max_val=2.0, name="SMI_TEMPERATURE")

    max_tokens = None
    if max_tokens_s:
        max_tokens = safe_parse_int(max_tokens_s, 4096, min_val=1, max_val=100000, name="SMI_MAX_TOKENS")

    try:
        clear_thinking = _parse_bool(clear_thinking_s) if clear_thinking_s else None
    except ValueError as e:
        raise ValueError(
            f"invalid SMI_CLEAR_THINKING={clear_thinking_s!r} (expected 'true', 'false', '1', '0', 'yes', 'no')"
        ) from e

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
        min_request_timeout_s=(
            safe_parse_float(min_request_timeout_s, 60.0, min_val=1.0, max_val=3600.0, name="SMI_MIN_REQUEST_TIMEOUT")
            if min_request_timeout_s
            else None
        ),
        max_request_retries=(
            safe_parse_int(max_request_retries_s, 6, min_val=0, max_val=20, name="SMI_MAX_REQUEST_RETRIES")
            if max_request_retries_s
            else None
        ),
    )


class RealAgent:
    """
    Real LLM agent.

    Currently supports OpenAI-compatible chat completions via:
      POST {base_url}/chat/completions

    This includes OpenRouter (unified API for 150+ models), OpenAI, and other
    OpenAI-compatible providers.
    """

    def __init__(self, cfg: RealAgentConfig, client: httpx.Client | None = None) -> None:
        self.cfg = cfg
        self._client = client or httpx.Client(timeout=60)

        if cfg.provider != "openai_compatible":
            raise ValueError(f"unsupported provider: {cfg.provider}")

        # Detect OpenRouter for optimizations
        self.is_openrouter = "openrouter.ai" in cfg.base_url.lower()

        if self.is_openrouter:
            self._client.headers.update(self._openrouter_headers())

        # Auto-detect reasoning models that need more tokens
        self.is_reasoning_model = any(
            x in cfg.model.lower() for x in ["deepseek", "o1", "o3", "glm", "qwen", "thinking"]
        )

    def _request_retries(self) -> int:
        if self.cfg.max_request_retries is None:
            return 6
        return max(0, int(self.cfg.max_request_retries))

    def _request_timeout(self, *, remaining_s: float | None) -> float | None:
        if remaining_s is None:
            return None
        timeout = max(1.0, float(remaining_s))
        if self.cfg.min_request_timeout_s is not None:
            timeout = max(timeout, float(self.cfg.min_request_timeout_s))
        return timeout

    def _openrouter_headers(self) -> dict[str, str]:
        return {
            "HTTP-Referer": "https://github.com/MystenLabs/sui-move-interface-extractor",
            "X-Title": "Sui Move Interface Extractor Benchmark",
        }

    def smoke(self) -> set[str]:
        """
        Minimal connectivity + parsing check. Returns a set (likely empty).
        """
        prompt = (
            'Return a JSON object with a single field "key_types" which is a JSON array of strings. '
            'For smoke testing, return: {"key_types": []}'
        )
        return self.complete_type_list(prompt)

    def debug_effective_config(self) -> dict[str, str]:
        def redact(v: str) -> str:
            suffix = v[-6:] if len(v) >= 6 else v
            return f"len={len(v)} suffix={suffix}"

        return {
            "provider": self.cfg.provider,
            "base_url": self.cfg.base_url,
            "model": self.cfg.model,
            "api_key": redact(self.cfg.api_key),
        }

    def complete_type_list(
        self,
        prompt: str,
        *,
        timeout_s: float | None = None,
        logger: JsonlLogger | None = None,
        log_context: dict[str, object] | None = None,
    ) -> set[str]:
        # Use jsonl_logger for event() calls, module logger for debug/info/error
        jsonl_logger = logger
        url = f"{self.cfg.base_url}/chat/completions"
        headers = {"Authorization": f"Bearer {self.cfg.api_key}"}
        if self.is_openrouter:
            headers.update(self._openrouter_headers())
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
        # timeout_s is provided by the runner's per-package time budget. Treat it as the primary limiter.
        # Do not add extra internal deadlines beyond the per-request timeout we pass to httpx.
        deadline = (time.monotonic() + timeout_s) if timeout_s is not None else None

        def body_prefix(r: httpx.Response) -> str:
            try:
                t = r.text
            except httpx.StreamError as e:
                logging.getLogger(__name__).debug("Failed to read response text: %s", e)
                return "<unavailable>"
            t = t.replace("\n", " ").replace("\r", " ")
            return t[:400]

        def extract_api_error(r: httpx.Response) -> str | None:
            try:
                data = r.json()
            except (json.JSONDecodeError, httpx.ResponseNotRead) as e:
                logging.getLogger(__name__).debug("Failed to parse API error response as JSON: %s", e)
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

        for attempt in range(self._request_retries()):
            try:
                req_timeout = None
                if deadline is not None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("per-call timeout exceeded")
                    req_timeout = self._request_timeout(remaining_s=remaining)
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
                            logging.getLogger(__name__).info(
                                f"Rate-limited by API (status={r.status_code}), retry-after={retry_after}s, backing off"
                            )
                        except (ValueError, TypeError) as e:
                            logging.getLogger(__name__).warning(
                                f"Failed to parse retry-after header '{retry_after}': {e}, "
                                f"using default backoff={backoff_s}s"
                            )
                            sleep_s = backoff_s
                    else:
                        logging.getLogger(__name__).info(
                            f"Rate-limited by API (status={r.status_code}), "
                            f"no retry-after header, using backoff={backoff_s}s"
                        )
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
                if jsonl_logger is not None:
                    ctx = dict(log_context or {})
                    ctx.update(
                        {
                            "attempt": attempt + 1,
                            "endpoint": url,
                            "model": self.cfg.model,
                            "base_url": self.cfg.base_url,
                            "timeout_s": timeout_s,
                            "req_timeout_s": req_timeout,
                            "last_status": last_status,
                            "last_body_prefix": last_body_prefix,
                            "exc_type": type(e).__name__,
                            "exc": str(e),
                        }
                    )
                    jsonl_logger.event("llm_request_error", **ctx)
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
        except (KeyError, IndexError, TypeError) as e:
            logging.getLogger(__name__).debug(f"Response missing choices[0].message.content: {e}, trying legacy format")
            try:
                content = data["choices"][0]["text"]
                logging.getLogger(__name__).debug("Successfully extracted content from legacy 'text' field")
            except (KeyError, IndexError, TypeError) as e2:
                data_summary = list(data.keys()) if isinstance(data, dict) else type(data)
                if jsonl_logger is not None:
                    jsonl_logger.event(
                        "llm_content_extraction_error",
                        error=str(e2),
                        data_keys=data_summary,
                    )
                raise ValueError(f"unexpected response shape: {data}") from e2

        if not isinstance(content, str):
            raise ValueError("unexpected response content type")

        if content.strip() == "":
            finish_reason = None
            try:
                finish_reason = data["choices"][0].get("finish_reason")
            except (KeyError, IndexError, TypeError):
                logging.getLogger(__name__).debug("Could not extract finish_reason from response")
            hint = ""
            if finish_reason == "length":
                hint = " (model hit max_tokens; increase SMI_MAX_TOKENS or reduce prompt size)"
            raise ValueError(f"model returned empty content{hint}")

        return extract_type_list(content)

    def complete_json(
        self,
        prompt: str,
        *,
        timeout_s: float | None = None,
        logger: JsonlLogger | None = None,
        log_context: dict[str, object] | None = None,
    ) -> dict[str, Any]:
        """
        Request an OpenAI-compatible chat completion and parse the assistant content as a JSON object.

        Returns (parsed_dict, usage_dict).
        """
        jsonl_logger = logger
        url = f"{self.cfg.base_url}/chat/completions"
        headers = {"Authorization": f"Bearer {self.cfg.api_key}"}
        if self.is_openrouter:
            headers.update(self._openrouter_headers())
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
        # timeout_s is provided by the runner's per-package time budget. Treat it as the primary limiter.
        # Do not add extra internal deadlines beyond the per-request timeout we pass to httpx.
        deadline = (time.monotonic() + timeout_s) if timeout_s is not None else None

        def body_prefix(r: httpx.Response) -> str:
            try:
                t = r.text
            except httpx.StreamError as e:
                logging.getLogger(__name__).debug("Failed to read response text: %s", e)
                return "<unavailable>"
            t = t.replace("\n", " ").replace("\r", " ")
            return t[:400]

        def extract_api_error(r: httpx.Response) -> str | None:
            try:
                data = r.json()
            except (json.JSONDecodeError, httpx.ResponseNotRead) as e:
                logging.getLogger(__name__).debug("Failed to parse API error response as JSON: %s", e)
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

        for _attempt in range(self._request_retries()):
            try:
                req_timeout = None
                if deadline is not None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("per-call timeout exceeded")
                    req_timeout = self._request_timeout(remaining_s=remaining)
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
                            logging.getLogger(__name__).info(
                                f"Rate-limited by API (status={r.status_code}), retry-after={retry_after}s, backing off"
                            )
                        except (ValueError, TypeError) as e:
                            logging.getLogger(__name__).warning(
                                f"Failed to parse retry-after header '{retry_after}': {e}, "
                                f"using default backoff={backoff_s}s"
                            )
                            sleep_s = backoff_s
                    else:
                        logging.getLogger(__name__).info(
                            f"Rate-limited by API (status={r.status_code}), "
                            f"no retry-after header, using backoff={backoff_s}s"
                        )
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
                if logger is not None:
                    ctx = dict(log_context or {})
                    ctx.update(
                        {
                            "attempt": _attempt + 1,
                            "endpoint": url,
                            "model": self.cfg.model,
                            "base_url": self.cfg.base_url,
                            "timeout_s": timeout_s,
                            "req_timeout_s": req_timeout,
                            "last_status": last_status,
                            "last_body_prefix": last_body_prefix,
                            "exc_type": type(e).__name__,
                            "exc": str(e),
                        }
                    )
                    logger.event("llm_request_error", **ctx)
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
        except (KeyError, IndexError, TypeError) as e:
            logging.getLogger(__name__).debug(f"Response missing choices[0].message.content: {e}, trying legacy format")
            try:
                content = data["choices"][0]["text"]
                logging.getLogger(__name__).debug("Successfully extracted content from legacy 'text' field")
            except (KeyError, IndexError, TypeError) as e2:
                data_summary = list(data.keys()) if isinstance(data, dict) else type(data)
                if jsonl_logger is not None:
                    jsonl_logger.event(
                        "llm_content_extraction_error",
                        error=str(e2),
                        data_keys=data_summary,
                    )
                raise ValueError(f"unexpected response shape: {data}") from e2

        if (log := logger) is not None:
            ctx = dict(log_context or {})
            ctx.update(
                {
                    "endpoint": url,
                    "model": self.cfg.model,
                    "base_url": self.cfg.base_url,
                    "timeout_s": timeout_s,
                    "content": content,
                }
            )
            log.event("llm_response", **ctx)

        if not isinstance(content, str):
            raise ValueError("unexpected response content type")
        if content.strip() == "":
            finish_reason = None
            try:
                finish_reason = data["choices"][0].get("finish_reason")
            except (KeyError, IndexError, TypeError):
                logging.getLogger(__name__).debug("Could not extract finish_reason from response")
            hint = ""
            if finish_reason == "length":
                hint = " (model hit max_tokens; increase SMI_MAX_TOKENS or reduce prompt size)"
            raise ValueError(f"model returned empty content{hint}")

        try:
            parsed = json.loads(content)
        except Exception as e:
            # For Phase II planning, some models return JSON wrapped in prose or ```json fences.
            # Prefer scoring intelligence over strict formatting while keeping observability.
            try:
                parsed = extract_json_value(content)
                if jsonl_logger is not None:
                    ctx = dict(log_context or {})
                    ctx.update(
                        {
                            "endpoint": url,
                            "model": self.cfg.model,
                            "base_url": self.cfg.base_url,
                            "timeout_s": timeout_s,
                            "exc_type": type(e).__name__,
                            "exc": str(e),
                        }
                    )
                    jsonl_logger.event("llm_json_extracted", **ctx)
            except JsonExtractError:
                if (log := jsonl_logger) is not None:
                    ctx = dict(log_context or {})
                    ctx.update(
                        {
                            "endpoint": url,
                            "model": self.cfg.model,
                            "base_url": self.cfg.base_url,
                            "timeout_s": timeout_s,
                            "exc_type": type(e).__name__,
                            "exc": str(e),
                            "content": content,
                        }
                    )
                    log.event("llm_json_parse_error", **ctx)
                raise

        if (log := logger) is not None:
            ctx = dict(log_context or {})
            ctx.update(
                {
                    "endpoint": url,
                    "model": self.cfg.model,
                    "base_url": self.cfg.base_url,
                    "timeout_s": timeout_s,
                    "parsed": parsed,
                }
            )
            log.event("llm_json_parsed", **ctx)

        if not isinstance(parsed, dict):
            raise ValueError("expected a JSON object")
        return cast(dict[str, Any], parsed)
