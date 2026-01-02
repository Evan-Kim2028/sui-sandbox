from __future__ import annotations

import os
from dataclasses import dataclass

import httpx

from smi_bench.env import load_dotenv


@dataclass(frozen=True)
class OpenRouterModel:
    id: str
    name: str | None
    context_length: int | None
    pricing_prompt: float | None
    pricing_completion: float | None


def _extract_price(v: object) -> float | None:
    if v is None:
        return None
    if isinstance(v, bool):
        return None
    if isinstance(v, (int, float)):
        return float(v)
    if isinstance(v, str):
        try:
            return float(v)
        except ValueError:
            return None
    return None


def _get_api_key(env_overrides: dict[str, str] | None = None) -> str:
    env_overrides = env_overrides or {}
    key = os.environ.get("SMI_API_KEY") or os.environ.get("OPENROUTER_API_KEY")
    if key:
        return key
    key = env_overrides.get("SMI_API_KEY") or env_overrides.get("OPENROUTER_API_KEY")
    if key:
        return key
    raise ValueError("missing OpenRouter API key (set OPENROUTER_API_KEY or SMI_API_KEY)")


def fetch_openrouter_models(
    *,
    base_url: str = "https://openrouter.ai/api/v1",
    api_key: str,
) -> list[OpenRouterModel]:
    url = f"{base_url.rstrip('/')}/models"
    r = httpx.get(url, headers={"Authorization": f"Bearer {api_key}"}, timeout=30)
    r.raise_for_status()
    payload = r.json()
    data = payload.get("data")
    if not isinstance(data, list):
        raise ValueError("unexpected OpenRouter /models response")

    out: list[OpenRouterModel] = []
    for item in data:
        if not isinstance(item, dict):
            continue
        model_id = item.get("id")
        if not isinstance(model_id, str) or not model_id:
            continue
        name = item.get("name")
        if not isinstance(name, str):
            name = None

        context_length = item.get("context_length")
        if not isinstance(context_length, int):
            context_length = None

        pricing = item.get("pricing")
        pricing_prompt = None
        pricing_completion = None
        if isinstance(pricing, dict):
            pricing_prompt = _extract_price(pricing.get("prompt"))
            pricing_completion = _extract_price(pricing.get("completion"))

        out.append(
            OpenRouterModel(
                id=model_id,
                name=name,
                context_length=context_length,
                pricing_prompt=pricing_prompt,
                pricing_completion=pricing_completion,
            )
        )
    return out


def write_model_ids(path: str, model_ids: list[str]) -> None:
    lines = [m.strip() for m in model_ids if m.strip()]
    lines = sorted(set(lines))
    with open(path, "w", encoding="utf-8") as f:
        for line in lines:
            f.write(f"{line}\n")


def main(argv: list[str] | None = None) -> int:
    import argparse

    p = argparse.ArgumentParser(description="Fetch OpenRouter model ids and write to a text file")
    p.add_argument("--env-file", type=str, default=None, help="Path to .env file (e.g. benchmark/.env)")
    p.add_argument(
        "--out",
        type=str,
        default="benchmark/manifests/models/openrouter_models.txt",
        help="Output file path",
    )
    p.add_argument(
        "--base-url",
        type=str,
        default=None,
        help="Override OpenRouter base URL (defaults to SMI_API_BASE_URL/OPENROUTER_BASE_URL or https://openrouter.ai/api/v1)",
    )
    p.add_argument(
        "--contains",
        type=str,
        default=None,
        help="Only include model ids containing this substring (case-insensitive)",
    )
    p.add_argument(
        "--name-contains",
        type=str,
        default=None,
        help="Only include models whose display name contains this substring (case-insensitive)",
    )
    args = p.parse_args(argv)

    env_overrides: dict[str, str] = {}
    if args.env_file:
        env_overrides = load_dotenv(__import__("pathlib").Path(args.env_file))

    base_url = (
        args.base_url
        or os.environ.get("SMI_API_BASE_URL")
        or os.environ.get("OPENROUTER_BASE_URL")
        or env_overrides.get("SMI_API_BASE_URL")
        or env_overrides.get("OPENROUTER_BASE_URL")
        or "https://openrouter.ai/api/v1"
    )
    api_key = _get_api_key(env_overrides)

    models = fetch_openrouter_models(base_url=base_url, api_key=api_key)

    if args.contains:
        needle = args.contains.lower()
        models = [m for m in models if needle in m.id.lower()]
    if args.name_contains:
        needle = args.name_contains.lower()
        models = [m for m in models if (m.name or "").lower().find(needle) != -1]

    write_model_ids(args.out, [m.id for m in models])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
