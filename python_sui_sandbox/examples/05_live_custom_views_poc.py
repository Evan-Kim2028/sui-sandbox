#!/usr/bin/env python3
"""Live custom-view PoC: compose DeepBook + Sui framework views.

This script demonstrates a practical pattern:
1. Read current object versions from a live fullnode (JSON-RPC).
2. Fetch BCS object payloads via sui-sandbox.
3. Execute existing Move view functions locally in the sandbox VM.
4. Compose deterministic "virtual views" from multiple contracts.

Example output fields:
- pool_liquidity_snapshot: two-pool utilization + freshness snapshot
- pool_health_snapshot: per-pool safety limits + aggregate utilization health
- margin_state_snapshot: account + policy constants + clock-aligned metadata
- manager_compact_snapshot: compact manager/account state from existing views
- transformation_trace: step-by-step input/output mapping
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import struct
import urllib.error
import urllib.request
from typing import Any

import sui_sandbox

FULLNODE_RPC = os.getenv("SUI_FULLNODE_RPC", "https://fullnode.mainnet.sui.io:443")

MARGIN_PKG = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b"
SUI_TYPE = "0x2::sui::SUI"
USDC_TYPE = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC"

OBJECT_IDS = {
    "clock": "0x6",
    "margin_manager": "0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80",
    "base_margin_pool": "0x53041c6f86c4782aabbfc1d4fe234a6d37160310c7ee740c915f0a01b7127344",
    "quote_margin_pool": "0xba473d9ae278f10af75c50a8fa341e9c6a1c087dc91a3f23e8048baf67d0754f",
}

RATIO_SCALE = 1_000_000_000
U64_MAX = (1 << 64) - 1


def _rpc_call(method: str, params: list[Any]) -> Any:
    payload = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
    ).encode("utf-8")
    req = urllib.request.Request(
        FULLNODE_RPC,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            body = json.loads(resp.read().decode("utf-8"))
    except (urllib.error.URLError, TimeoutError) as exc:
        raise RuntimeError(f"JSON-RPC call failed for {method}: {exc}") from exc

    if body.get("error"):
        raise RuntimeError(f"JSON-RPC error for {method}: {body['error']}")
    return body.get("result")


def _latest_checkpoint_from_fullnode() -> int:
    result = _rpc_call("sui_getLatestCheckpointSequenceNumber", [])
    return int(result)


def _fullnode_object_version(object_id: str) -> int | None:
    try:
        result = _rpc_call(
            "sui_getObject",
            [
                object_id,
                {
                    "showType": False,
                    "showOwner": False,
                    "showPreviousTransaction": False,
                    "showDisplay": False,
                    "showContent": False,
                    "showBcs": False,
                    "showStorageRebate": False,
                },
            ],
        )
        version = result.get("data", {}).get("version")
        return int(version) if version is not None else None
    except RuntimeError:
        return None


def _decode_u64(b64: str) -> int:
    return struct.unpack("<Q", base64.b64decode(b64))[0]


def _decode_bool(b64: str) -> bool:
    raw = base64.b64decode(b64)
    return raw != b"\x00"


def _decode_hex32(b64: str) -> str:
    return "0x" + base64.b64decode(b64).hex()


def _decode_uleb128(raw: bytes, start: int = 0) -> tuple[int, int]:
    value = 0
    shift = 0
    i = start
    while i < len(raw):
        byte = raw[i]
        value |= (byte & 0x7F) << shift
        i += 1
        if byte & 0x80 == 0:
            return value, i
        shift += 7
        if shift > 63:
            raise ValueError("ULEB128 too large")
    raise ValueError("Invalid ULEB128 payload")


def _decode_option_hex32(b64: str) -> str | None:
    raw = base64.b64decode(b64)
    if not raw:
        raise ValueError("Empty option payload")
    tag = raw[0]
    if tag == 0:
        return None
    if tag != 1 or len(raw) < 33:
        raise ValueError(f"Invalid option<address> payload length={len(raw)} tag={tag}")
    return "0x" + raw[1:33].hex()


def _decode_vec_u64(b64: str) -> list[int]:
    raw = base64.b64decode(b64)
    length, offset = _decode_uleb128(raw, 0)
    values: list[int] = []
    for _ in range(length):
        end = offset + 8
        if end > len(raw):
            raise ValueError("vector<u64> payload truncated")
        values.append(struct.unpack("<Q", raw[offset:end])[0])
        offset = end
    if offset != len(raw):
        raise ValueError("vector<u64> payload has trailing bytes")
    return values


def _obj_input(object_meta: dict[str, Any]) -> dict[str, Any]:
    return {
        "object_id": object_meta["object_id"],
        "type_tag": object_meta["type_tag"],
        "bcs_bytes": base64.b64decode(object_meta["bcs_base64"]),
        "is_shared": bool(object_meta.get("is_shared", False)),
    }


def _summarize_object_inputs(object_inputs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for obj in object_inputs:
        bcs = obj.get("bcs_bytes", b"")
        out.append(
            {
                "object_id": obj.get("object_id"),
                "type_tag": obj.get("type_tag"),
                "is_shared": bool(obj.get("is_shared", False)),
                "bcs_size_bytes": len(bcs),
                "bcs_sha256": hashlib.sha256(bcs).hexdigest(),
            }
        )
    return out


def _append_trace_decoded(
    trace: list[dict[str, Any]] | None, decoded_output: dict[str, Any]
) -> None:
    if trace is None or not trace:
        return
    trace[-1]["decoded_output"] = decoded_output


def _run_view(
    package_id: str,
    module: str,
    function: str,
    *,
    type_args: list[str] | None = None,
    object_inputs: list[dict[str, Any]] | None = None,
    pure_inputs: list[bytes] | None = None,
    package_bytecodes: dict[str, Any] | None = None,
    trace: list[dict[str, Any]] | None = None,
    trace_label: str | None = None,
) -> dict[str, Any]:
    type_args = type_args or []
    object_inputs = object_inputs or []
    pure_inputs = pure_inputs or []
    out = sui_sandbox.call_view_function(
        package_id=package_id,
        module=module,
        function=function,
        type_args=type_args,
        object_inputs=object_inputs,
        pure_inputs=pure_inputs,
        package_bytecodes=package_bytecodes,
    )
    if not out.get("success"):
        raise RuntimeError(
            f"{package_id}::{module}::{function} failed: {out.get('error')}"
        )
    if trace is not None:
        trace.append(
            {
                "label": trace_label or f"{module}.{function}",
                "call": f"{package_id}::{module}::{function}",
                "inputs": {
                    "type_args": type_args,
                    "object_inputs": _summarize_object_inputs(object_inputs),
                    "pure_inputs_base64": [
                        base64.b64encode(blob).decode("ascii") for blob in pure_inputs
                    ],
                },
                "raw_output": {
                    "return_type_tags": out["return_type_tags"][0]
                    if out.get("return_type_tags")
                    else [],
                    "return_values_base64": out["return_values"][0]
                    if out.get("return_values")
                    else [],
                    "gas_used": out.get("gas_used"),
                },
            }
        )
    return out


def _first_ret_u64(out: dict[str, Any]) -> int:
    return _decode_u64(out["return_values"][0][0])


def _first_ret_bool(out: dict[str, Any]) -> bool:
    return _decode_bool(out["return_values"][0][0])


def _first_ret_hex32(out: dict[str, Any]) -> str:
    return _decode_hex32(out["return_values"][0][0])


def _first_ret_option_hex32(out: dict[str, Any]) -> str | None:
    return _decode_option_hex32(out["return_values"][0][0])


def _first_ret_vec_u64(out: dict[str, Any]) -> list[int]:
    return _decode_vec_u64(out["return_values"][0][0])


def _run_first_u64(
    package_id: str,
    module: str,
    function: str,
    *,
    type_args: list[str] | None = None,
    object_inputs: list[dict[str, Any]] | None = None,
    pure_inputs: list[bytes] | None = None,
    package_bytecodes: dict[str, Any] | None = None,
    trace: list[dict[str, Any]] | None = None,
    trace_label: str | None = None,
) -> int:
    out = _run_view(
        package_id,
        module,
        function,
        type_args=type_args,
        object_inputs=object_inputs,
        pure_inputs=pure_inputs,
        package_bytecodes=package_bytecodes,
        trace=trace,
        trace_label=trace_label,
    )
    value = _first_ret_u64(out)
    _append_trace_decoded(trace, {"first_return_u64": value})
    return value


def _run_first_bool(
    package_id: str,
    module: str,
    function: str,
    *,
    type_args: list[str] | None = None,
    object_inputs: list[dict[str, Any]] | None = None,
    pure_inputs: list[bytes] | None = None,
    package_bytecodes: dict[str, Any] | None = None,
    trace: list[dict[str, Any]] | None = None,
    trace_label: str | None = None,
) -> bool:
    out = _run_view(
        package_id,
        module,
        function,
        type_args=type_args,
        object_inputs=object_inputs,
        pure_inputs=pure_inputs,
        package_bytecodes=package_bytecodes,
        trace=trace,
        trace_label=trace_label,
    )
    value = _first_ret_bool(out)
    _append_trace_decoded(trace, {"first_return_bool": value})
    return value


def _run_first_hex32(
    package_id: str,
    module: str,
    function: str,
    *,
    type_args: list[str] | None = None,
    object_inputs: list[dict[str, Any]] | None = None,
    pure_inputs: list[bytes] | None = None,
    package_bytecodes: dict[str, Any] | None = None,
    trace: list[dict[str, Any]] | None = None,
    trace_label: str | None = None,
) -> str:
    out = _run_view(
        package_id,
        module,
        function,
        type_args=type_args,
        object_inputs=object_inputs,
        pure_inputs=pure_inputs,
        package_bytecodes=package_bytecodes,
        trace=trace,
        trace_label=trace_label,
    )
    value = _first_ret_hex32(out)
    _append_trace_decoded(trace, {"first_return_hex32": value})
    return value


def _run_first_option_hex32(
    package_id: str,
    module: str,
    function: str,
    *,
    type_args: list[str] | None = None,
    object_inputs: list[dict[str, Any]] | None = None,
    pure_inputs: list[bytes] | None = None,
    package_bytecodes: dict[str, Any] | None = None,
    trace: list[dict[str, Any]] | None = None,
    trace_label: str | None = None,
) -> str | None:
    out = _run_view(
        package_id,
        module,
        function,
        type_args=type_args,
        object_inputs=object_inputs,
        pure_inputs=pure_inputs,
        package_bytecodes=package_bytecodes,
        trace=trace,
        trace_label=trace_label,
    )
    value = _first_ret_option_hex32(out)
    _append_trace_decoded(trace, {"first_return_option_hex32": value})
    return value


def _run_first_vec_u64(
    package_id: str,
    module: str,
    function: str,
    *,
    type_args: list[str] | None = None,
    object_inputs: list[dict[str, Any]] | None = None,
    pure_inputs: list[bytes] | None = None,
    package_bytecodes: dict[str, Any] | None = None,
    trace: list[dict[str, Any]] | None = None,
    trace_label: str | None = None,
) -> list[int]:
    out = _run_view(
        package_id,
        module,
        function,
        type_args=type_args,
        object_inputs=object_inputs,
        pure_inputs=pure_inputs,
        package_bytecodes=package_bytecodes,
        trace=trace,
        trace_label=trace_label,
    )
    values = _first_ret_vec_u64(out)
    _append_trace_decoded(
        trace,
        {
            "first_return_vec_u64_count": len(values),
            "first_return_vec_u64": values,
        },
    )
    return values


def _pool_weather(
    *,
    label: str,
    pool_object_input: dict[str, Any],
    pool_type: str,
    package_payload: dict[str, Any],
    now_ms: int,
    trace: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    total_supply = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "total_supply",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.total_supply",
    )
    total_borrow = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "total_borrow",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.total_borrow",
    )
    interest_rate = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "interest_rate",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.interest_rate",
    )
    true_interest_rate = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "true_interest_rate",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.true_interest_rate",
    )
    last_update_ms = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "last_update_timestamp",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.last_update_timestamp",
    )
    borrow_ratio = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "borrow_ratio",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.borrow_ratio",
    )
    supply_ratio = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "supply_ratio",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.supply_ratio",
    )
    protocol_spread = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "protocol_spread",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.protocol_spread",
    )
    max_utilization_rate = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "max_utilization_rate",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.max_utilization_rate",
    )
    is_rate_limit_enabled = _run_first_bool(
        MARGIN_PKG,
        "margin_pool",
        "is_rate_limit_enabled",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.is_rate_limit_enabled",
    )
    rate_limit_capacity = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "rate_limit_capacity",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.rate_limit_capacity",
    )
    rate_limit_refill_rate_per_ms = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "rate_limit_refill_rate_per_ms",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.rate_limit_refill_rate_per_ms",
    )
    min_borrow = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "min_borrow",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.min_borrow",
    )
    supply_cap = _run_first_u64(
        MARGIN_PKG,
        "margin_pool",
        "supply_cap",
        type_args=[pool_type],
        object_inputs=[pool_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label=f"{label}.supply_cap",
    )

    utilization_pct = (100.0 * total_borrow / total_supply) if total_supply else 0.0
    freshness_seconds = max(0.0, (now_ms - last_update_ms) / 1000.0)
    available_liquidity = total_supply - total_borrow if total_supply > total_borrow else 0
    max_utilization_borrow_limit = (total_supply * max_utilization_rate) // RATIO_SCALE
    liquidity_buffer = (
        max_utilization_borrow_limit - total_borrow
        if max_utilization_borrow_limit > total_borrow
        else 0
    )
    utilization_limit_pct = 100.0 * max_utilization_rate / RATIO_SCALE
    is_near_utilization_limit = (
        utilization_pct >= (utilization_limit_pct * 0.95)
        if utilization_limit_pct > 0.0
        else False
    )

    # Non-protocol composite metric for demo purposes.
    pulse_score = round(utilization_pct * 0.7 + min(freshness_seconds, 60.0) * 0.3, 4)

    return {
        "label": label,
        "pool_object_id": pool_object_input["object_id"],
        "total_supply_raw": total_supply,
        "total_borrow_raw": total_borrow,
        "available_liquidity_raw": available_liquidity,
        "utilization_pct": round(utilization_pct, 6),
        "utilization_limit_pct": round(utilization_limit_pct, 6),
        "is_near_utilization_limit": is_near_utilization_limit,
        "max_utilization_borrow_limit_raw": max_utilization_borrow_limit,
        "liquidity_buffer_raw": liquidity_buffer,
        "interest_rate_raw": interest_rate,
        "true_interest_rate_raw": true_interest_rate,
        "borrow_ratio_raw": borrow_ratio,
        "supply_ratio_raw": supply_ratio,
        "protocol_spread_raw": protocol_spread,
        "max_utilization_rate_raw": max_utilization_rate,
        "is_rate_limit_enabled": is_rate_limit_enabled,
        "rate_limit_capacity_raw": rate_limit_capacity,
        "rate_limit_refill_rate_per_ms_raw": rate_limit_refill_rate_per_ms,
        "min_borrow_raw": min_borrow,
        "supply_cap_raw": supply_cap,
        "last_update_ms": last_update_ms,
        "freshness_seconds": round(freshness_seconds, 3),
        "pulse_score": pulse_score,
    }


def _health_band(*, utilization_pct: float, is_near_limit: bool, freshness_seconds: float) -> str:
    if is_near_limit or utilization_pct >= 90.0 or freshness_seconds > 1800.0:
        return "red"
    if utilization_pct >= 70.0 or freshness_seconds > 600.0:
        return "yellow"
    return "green"


def _pool_health_snapshot(
    pools: list[dict[str, Any]], *, now_ms: int
) -> dict[str, Any]:
    pool_rows: list[dict[str, Any]] = []
    band_counts = {"green": 0, "yellow": 0, "red": 0}
    for pool in pools:
        band = _health_band(
            utilization_pct=float(pool["utilization_pct"]),
            is_near_limit=bool(pool["is_near_utilization_limit"]),
            freshness_seconds=float(pool["freshness_seconds"]),
        )
        band_counts[band] += 1
        pool_rows.append(
            {
                "label": pool["label"],
                "pool_object_id": pool["pool_object_id"],
                "band": band,
                "utilization_pct": pool["utilization_pct"],
                "utilization_limit_pct": pool["utilization_limit_pct"],
                "is_near_utilization_limit": pool["is_near_utilization_limit"],
                "available_liquidity_raw": pool["available_liquidity_raw"],
                "liquidity_buffer_raw": pool["liquidity_buffer_raw"],
                "freshness_seconds": pool["freshness_seconds"],
                "is_rate_limit_enabled": pool["is_rate_limit_enabled"],
            }
        )

    total_supply = sum(int(pool["total_supply_raw"]) for pool in pools)
    total_borrow = sum(int(pool["total_borrow_raw"]) for pool in pools)
    combined_utilization_pct = (100.0 * total_borrow / total_supply) if total_supply else 0.0
    aggregate_band = "green"
    if band_counts["red"] > 0:
        aggregate_band = "red"
    elif band_counts["yellow"] > 0:
        aggregate_band = "yellow"

    return {
        "as_of_ms": now_ms,
        "aggregate_band": aggregate_band,
        "band_counts": band_counts,
        "combined": {
            "total_supply_raw": total_supply,
            "total_borrow_raw": total_borrow,
            "combined_utilization_pct": round(combined_utilization_pct, 6),
            "total_available_liquidity_raw": max(0, total_supply - total_borrow),
        },
        "pools": pool_rows,
    }


def _manager_compact_snapshot(
    *,
    manager_object_input: dict[str, Any],
    package_payload: dict[str, Any],
    trace: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    manager_owner = _run_first_hex32(
        MARGIN_PKG,
        "margin_manager",
        "owner",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.owner",
    )
    manager_pool_id = _run_first_hex32(
        MARGIN_PKG,
        "margin_manager",
        "deepbook_pool",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.deepbook_pool",
    )
    manager_id = _run_first_hex32(
        MARGIN_PKG,
        "margin_manager",
        "id",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.id",
    )
    isolated_margin_pool_id = _run_first_option_hex32(
        MARGIN_PKG,
        "margin_manager",
        "margin_pool_id",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.margin_pool_id",
    )
    manager_has_base_debt = _run_first_bool(
        MARGIN_PKG,
        "margin_manager",
        "has_base_debt",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.has_base_debt",
    )
    manager_base_balance = _run_first_u64(
        MARGIN_PKG,
        "margin_manager",
        "base_balance",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.base_balance",
    )
    manager_quote_balance = _run_first_u64(
        MARGIN_PKG,
        "margin_manager",
        "quote_balance",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.quote_balance",
    )
    borrowed_base_shares = _run_first_u64(
        MARGIN_PKG,
        "margin_manager",
        "borrowed_base_shares",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.borrowed_base_shares",
    )
    borrowed_quote_shares = _run_first_u64(
        MARGIN_PKG,
        "margin_manager",
        "borrowed_quote_shares",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.borrowed_quote_shares",
    )
    deep_balance = _run_first_u64(
        MARGIN_PKG,
        "margin_manager",
        "deep_balance",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.deep_balance",
    )
    highest_trigger_below_price = _run_first_u64(
        MARGIN_PKG,
        "margin_manager",
        "highest_trigger_below_price",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.highest_trigger_below_price",
    )
    lowest_trigger_above_price = _run_first_u64(
        MARGIN_PKG,
        "margin_manager",
        "lowest_trigger_above_price",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.lowest_trigger_above_price",
    )
    conditional_order_ids = _run_first_vec_u64(
        MARGIN_PKG,
        "margin_manager",
        "conditional_order_ids",
        type_args=[SUI_TYPE, USDC_TYPE],
        object_inputs=[manager_object_input],
        package_bytecodes=package_payload,
        trace=trace,
        trace_label="margin_manager.conditional_order_ids",
    )

    has_any_debt = (
        manager_has_base_debt or borrowed_base_shares > 0 or borrowed_quote_shares > 0
    )
    trigger_window_initialized = (
        highest_trigger_below_price > 0 or lowest_trigger_above_price < U64_MAX
    )
    is_idle = (
        manager_base_balance == 0
        and manager_quote_balance == 0
        and deep_balance == 0
        and not has_any_debt
        and len(conditional_order_ids) == 0
    )

    return {
        "manager_id": manager_id,
        "owner": manager_owner,
        "deepbook_pool_id": manager_pool_id,
        "isolated_margin_pool_id": isolated_margin_pool_id,
        "balances_raw": {
            "base": manager_base_balance,
            "quote": manager_quote_balance,
            "deep": deep_balance,
        },
        "borrowed_shares_raw": {
            "base": borrowed_base_shares,
            "quote": borrowed_quote_shares,
        },
        "triggers_raw": {
            "highest_trigger_below_price": highest_trigger_below_price,
            "lowest_trigger_above_price": lowest_trigger_above_price,
        },
        "conditional_order_ids": conditional_order_ids,
        "conditional_order_count": len(conditional_order_ids),
        "flags": {
            "has_base_debt": manager_has_base_debt,
            "has_any_debt": has_any_debt,
            "trigger_window_initialized": trigger_window_initialized,
            "is_idle": is_idle,
        },
    }


def main() -> None:
    latest_checkpoint = _latest_checkpoint_from_fullnode()
    transformation_trace: list[dict[str, Any]] = []

    package_payload = sui_sandbox.fetch_historical_package_bytecodes(
        [MARGIN_PKG],
        type_refs=[SUI_TYPE, USDC_TYPE],
    )

    live_objects: dict[str, dict[str, Any]] = {}
    input_objects: dict[str, dict[str, Any]] = {}
    for name, object_id in OBJECT_IDS.items():
        meta = sui_sandbox.fetch_object_bcs(object_id)
        obj_input = _obj_input(meta)
        live_objects[name] = {"meta": meta, "input": obj_input}
        input_objects[name] = {
            "object_id": object_id,
            "type_tag": meta.get("type_tag"),
            "sandbox_object_version": int(meta.get("version")),
            "is_shared": bool(meta.get("is_shared", False)),
            "hydration_endpoint": meta.get("endpoint_used"),
            "bcs_size_bytes": len(obj_input["bcs_bytes"]),
            "bcs_sha256": hashlib.sha256(obj_input["bcs_bytes"]).hexdigest(),
        }

    alignment: dict[str, dict[str, Any]] = {}
    for name, object_id in OBJECT_IDS.items():
        sandbox_version = int(live_objects[name]["meta"]["version"])
        fullnode_version = _fullnode_object_version(object_id)
        gap = None
        if fullnode_version is not None:
            gap = fullnode_version - sandbox_version
        alignment[name] = {
            "object_id": object_id,
            "fullnode_version": fullnode_version,
            "sandbox_object_version": sandbox_version,
            "version_gap_fullnode_minus_sandbox": gap,
        }

    now_ms = _run_first_u64(
        "0x2",
        "clock",
        "timestamp_ms",
        object_inputs=[live_objects["clock"]["input"]],
        package_bytecodes=package_payload,
        trace=transformation_trace,
        trace_label="clock.timestamp_ms",
    )

    max_risk_ratio = _run_first_u64(
        MARGIN_PKG,
        "margin_constants",
        "max_risk_ratio",
        package_bytecodes=package_payload,
        trace=transformation_trace,
        trace_label="margin_constants.max_risk_ratio",
    )
    max_leverage = _run_first_u64(
        MARGIN_PKG,
        "margin_constants",
        "max_leverage",
        package_bytecodes=package_payload,
        trace=transformation_trace,
        trace_label="margin_constants.max_leverage",
    )

    manager_compact_snapshot = _manager_compact_snapshot(
        manager_object_input=live_objects["margin_manager"]["input"],
        package_payload=package_payload,
        trace=transformation_trace,
    )

    pool_liquidity_snapshot = [
        _pool_weather(
            label="SUI Margin Pool",
            pool_object_input=live_objects["base_margin_pool"]["input"],
            pool_type=SUI_TYPE,
            package_payload=package_payload,
            now_ms=now_ms,
            trace=transformation_trace,
        ),
        _pool_weather(
            label="USDC Margin Pool",
            pool_object_input=live_objects["quote_margin_pool"]["input"],
            pool_type=USDC_TYPE,
            package_payload=package_payload,
            now_ms=now_ms,
            trace=transformation_trace,
        ),
    ]
    pool_health_snapshot = _pool_health_snapshot(pool_liquidity_snapshot, now_ms=now_ms)

    margin_state_snapshot = {
        "manager_owner": manager_compact_snapshot["owner"],
        "deepbook_pool_id": manager_compact_snapshot["deepbook_pool_id"],
        "base_balance_raw": manager_compact_snapshot["balances_raw"]["base"],
        "quote_balance_raw": manager_compact_snapshot["balances_raw"]["quote"],
        "has_base_debt": manager_compact_snapshot["flags"]["has_base_debt"],
        "clock_timestamp_ms": now_ms,
        "policy_caps": {
            "max_risk_ratio_raw": max_risk_ratio,
            "max_leverage_raw": max_leverage,
        },
    }

    report = {
        "proof": "live_fullnode_plus_local_vm_views",
        "fullnode_rpc": FULLNODE_RPC,
        "fullnode_latest_checkpoint": latest_checkpoint,
        "sandbox_hydration_endpoint": live_objects["clock"]["meta"].get("endpoint_used"),
        "package_count_loaded": package_payload.get("count"),
        "input_objects": input_objects,
        "object_version_alignment": alignment,
        "composed_views": {
            "pool_liquidity_snapshot": pool_liquidity_snapshot,
            "pool_health_snapshot": pool_health_snapshot,
            "margin_state_snapshot": margin_state_snapshot,
            "manager_compact_snapshot": manager_compact_snapshot,
        },
        "transformation_trace": transformation_trace,
        "transformation_examples": [
            {
                "field": "utilization_pct",
                "formula": "100 * total_borrow_raw / total_supply_raw",
            },
            {
                "field": "available_liquidity_raw",
                "formula": "max(total_supply_raw - total_borrow_raw, 0)",
            },
            {
                "field": "liquidity_buffer_raw",
                "formula": "max((total_supply_raw * max_utilization_rate_raw / 1e9) - total_borrow_raw, 0)",
            },
            {
                "field": "freshness_seconds",
                "formula": "(clock_timestamp_ms - last_update_ms) / 1000",
            },
            {
                "field": "pulse_score",
                "formula": "0.7 * utilization_pct + 0.3 * min(freshness_seconds, 60)",
            },
            {
                "field": "manager_compact_snapshot.flags.is_idle",
                "formula": "balances==0 and borrowed_shares==0 and conditional_order_count==0",
            },
        ],
        "notes": [
            "All view execution above used existing on-chain contracts (no custom package publish).",
            "Composed views are deterministic transformations over decoded Move return values.",
        ],
    }

    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
