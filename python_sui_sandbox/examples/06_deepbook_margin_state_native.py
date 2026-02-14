#!/usr/bin/env python3
"""Example 6: DeepBook margin_state via native Python bindings (no CLI pass-through).

This mirrors the Rust DeepBook margin example flow:
1) Load object versions from JSON snapshot
2) Fetch historical object BCS via gRPC
3) Fetch historical package bytecodes + dependencies
4) Execute margin_manager::manager_state in local Move VM
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import struct
from pathlib import Path
from typing import Any

import sui_sandbox

# DeepBook / Margin constants (mainnet)
DEEPBOOK_PACKAGE = (
    "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497"
)
MARGIN_PACKAGE = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b"
MARGIN_REGISTRY = (
    "0x0e40998b359a9ccbab22a98ed21bd4346abf19158bc7980c8291908086b3a742"
)
CLOCK = "0x6"
TARGET_MARGIN_MANAGER = (
    "0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80"
)
DEEPBOOK_POOL = "0xe05dafb5133bcffb8d59f4e12465dc0e9faeaa05e3e342a08fe135800e3e4407"
BASE_MARGIN_POOL = "0x53041c6f86c4782aabbfc1d4fe234a6d37160310c7ee740c915f0a01b7127344"
QUOTE_MARGIN_POOL = "0xba473d9ae278f10af75c50a8fa341e9c6a1c087dc91a3f23e8048baf67d0754f"
SUI_PYTH_PRICE_INFO = (
    "0x801dbc2f0053d34734814b2d6df491ce7807a725fe9a01ad74a07e9c51396c37"
)
USDC_PYTH_PRICE_INFO = (
    "0x5dec622733a204ca27f5a90d8c2fad453cc6665186fd5dff13a83d0b6c9027ab"
)

SUI_TYPE = "0x2::sui::SUI"
USDC_TYPE = (
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC"
)

DEFAULT_VERSIONS_FILE = (
    "examples/advanced/deepbook_margin_state/data/deepbook_versions_240733000.json"
)

OBJECT_IDS = [
    TARGET_MARGIN_MANAGER,
    MARGIN_REGISTRY,
    SUI_PYTH_PRICE_INFO,
    USDC_PYTH_PRICE_INFO,
    DEEPBOOK_POOL,
    BASE_MARGIN_POOL,
    QUOTE_MARGIN_POOL,
    CLOCK,
]


def decode_u64_le(raw: bytes) -> int:
    if len(raw) < 8:
        return 0
    return struct.unpack("<Q", raw[:8])[0]


def load_versions(path: Path) -> tuple[int, dict[str, int]]:
    data = json.loads(path.read_text())
    checkpoint = int(data["checkpoint"])
    versions = {
        str(object_id): int(meta["version"])
        for object_id, meta in data.get("objects", {}).items()
        if isinstance(meta, dict) and "version" in meta
    }
    return checkpoint, versions


def fetch_object_inputs(
    versions: dict[str, int],
    endpoint: str | None,
    api_key: str | None,
) -> tuple[list[dict[str, Any]], dict[str, int]]:
    object_inputs: list[dict[str, Any]] = []

    # Keep all known version hints (not just PTB inputs) for child fetcher lookups.
    historical_versions = dict(versions)

    for object_id in OBJECT_IDS:
        version = versions.get(object_id)
        if version is None:
            raise RuntimeError(f"Missing version for required object: {object_id}")

        payload = sui_sandbox.fetch_object_bcs(
            object_id,
            version=version,
            endpoint=endpoint,
            api_key=api_key,
        )
        bcs_bytes = base64.b64decode(payload["bcs_base64"])
        type_tag = payload["type_tag"]

        object_inputs.append(
            {
                "object_id": object_id,
                "bcs_bytes": bcs_bytes,
                "type_tag": type_tag,
                "is_shared": True,
                "mutable": False,
            }
        )
        print(f"  fetched {object_id[:18]}... v{payload['version']}")

    return object_inputs, historical_versions


def fetch_package_bytecodes(
    checkpoint: int,
    endpoint: str | None,
    api_key: str | None,
) -> dict[str, Any]:
    return sui_sandbox.fetch_historical_package_bytecodes(
        [MARGIN_PACKAGE, DEEPBOOK_PACKAGE],
        type_refs=[SUI_TYPE, USDC_TYPE],
        checkpoint=checkpoint,
        endpoint=endpoint,
        api_key=api_key,
    )


def decode_manager_state(result: dict[str, Any]) -> dict[str, float] | None:
    if not result.get("success"):
        return None
    return_values = result.get("return_values", [])
    if not return_values or not return_values[0]:
        return None

    cmd_returns: list[str] = return_values[0]
    if len(cmd_returns) < 12:
        return None

    decoded = [base64.b64decode(v) for v in cmd_returns]
    risk_ratio = decode_u64_le(decoded[2])
    base_asset = decode_u64_le(decoded[3])
    quote_asset = decode_u64_le(decoded[4])
    base_debt = decode_u64_le(decoded[5])
    quote_debt = decode_u64_le(decoded[6])
    current_price = decode_u64_le(decoded[11])

    return {
        "risk_ratio_pct": risk_ratio / 1e9 * 100.0,
        "base_asset_sui": base_asset / 1e9,
        "quote_asset_usdc": quote_asset / 1e6,
        "base_debt_sui": base_debt / 1e9,
        "quote_debt_usdc": quote_debt / 1e6,
        "current_price": current_price / 1e6,
    }


def main() -> None:
    parser = argparse.ArgumentParser(
        description="DeepBook margin_state using native Python bindings"
    )
    parser.add_argument(
        "--versions-file",
        default=DEFAULT_VERSIONS_FILE,
        help=f"Versions JSON snapshot (default: {DEFAULT_VERSIONS_FILE})",
    )
    parser.add_argument(
        "--grpc-endpoint",
        default=None,
        help="Optional gRPC endpoint override (defaults to env/auto discovery)",
    )
    parser.add_argument(
        "--grpc-api-key",
        default=None,
        help="Optional gRPC API key override",
    )
    args = parser.parse_args()

    versions_path = Path(args.versions_file)
    checkpoint, versions = load_versions(versions_path)

    grpc_endpoint = args.grpc_endpoint or os.getenv("SUI_GRPC_ENDPOINT")
    grpc_api_key = args.grpc_api_key or os.getenv("SUI_GRPC_API_KEY")

    print("=== DeepBook manager_state (Native Python) ===")
    print(f"versions_file: {versions_path}")
    print(f"checkpoint:    {checkpoint}")
    if grpc_endpoint:
        print(f"grpc_endpoint: {grpc_endpoint}")

    print("\n[1/3] Fetching historical objects...")
    object_inputs, historical_versions = fetch_object_inputs(
        versions,
        grpc_endpoint,
        grpc_api_key,
    )

    print("\n[2/3] Fetching historical package bytecodes + deps...")
    package_bytecodes = fetch_package_bytecodes(
        checkpoint,
        grpc_endpoint,
        grpc_api_key,
    )
    print(f"  loaded package bytecode sets: {len(package_bytecodes.get('packages', {}))}")

    print("\n[3/3] Executing margin_manager::manager_state...")
    result = sui_sandbox.call_view_function(
        package_id=MARGIN_PACKAGE,
        module="margin_manager",
        function="manager_state",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=object_inputs,
        historical_versions=historical_versions,
        fetch_child_objects=True,
        grpc_endpoint=grpc_endpoint,
        grpc_api_key=grpc_api_key,
        package_bytecodes=package_bytecodes,
        # Historical package payload already includes dependency closure.
        fetch_deps=False,
    )

    print(f"success: {result.get('success')}")
    print(f"gas_used: {result.get('gas_used')}")
    if result.get("error"):
        err = str(result["error"])
        print(f"error: {err}")
        endpoint_for_hint = grpc_endpoint or "https://archive.mainnet.sui.io:443"
        looks_like_archive_gap = (
            ("ContractAbort" in err and "abort_code" in err)
            or ("major_status: ABORTED" in err and "dynamic_field" in err)
        )
        if looks_like_archive_gap and (
            "archive.mainnet.sui.io" in endpoint_for_hint
            or "fullnode.mainnet.sui.io" in endpoint_for_hint
        ):
            print(
                "hint: likely archive runtime-object gap; retry with "
                "SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443"
            )
        return

    decoded = decode_manager_state(result)
    if decoded:
        print("\nDecoded margin state:")
        for key, value in decoded.items():
            print(f"  {key}: {value:.6f}")
    else:
        print("No decodable return payload found.")


if __name__ == "__main__":
    main()
