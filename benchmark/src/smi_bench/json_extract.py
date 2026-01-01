from __future__ import annotations

import json
import re


class JsonExtractError(ValueError):
    pass


_FENCE_RE = re.compile(r"```(?:json)?\s*(.*?)\s*```", re.DOTALL | re.IGNORECASE)


def _strip_code_fences(text: str) -> str:
    m = _FENCE_RE.search(text)
    if m:
        return m.group(1).strip()
    return text.strip()


def extract_json_value(text: str) -> object:
    """
    Best-effort JSON extraction from a model response.
    Accepts:
      - raw JSON
      - JSON wrapped in ```json fences
      - extra prose around a JSON blob (extract first {...} or [...] span)
    """
    s = _strip_code_fences(text)
    try:
        return json.loads(s)
    except Exception:
        pass

    # Try to find the first top-level JSON array/object substring.
    for opener, closer in (("[", "]"), ("{", "}")):
        start = s.find(opener)
        if start == -1:
            continue
        end = s.rfind(closer)
        if end == -1 or end <= start:
            continue
        candidate = s[start : end + 1]
        try:
            return json.loads(candidate)
        except Exception:
            continue

    raise JsonExtractError("no JSON found in model output")


def extract_type_list(text: str) -> set[str]:
    """
    Accepts either:
      - JSON array of strings: ["0x..::m::S", ...]
      - JSON object with key_types: {"key_types":[...]}
    """
    v = extract_json_value(text)
    if isinstance(v, list):
        return {x for x in v if isinstance(x, str)}
    if isinstance(v, dict):
        key_types = v.get("key_types")
        if isinstance(key_types, list):
            return {x for x in key_types if isinstance(x, str)}
    raise JsonExtractError("unexpected JSON shape (expected array or {key_types:[...]})")

