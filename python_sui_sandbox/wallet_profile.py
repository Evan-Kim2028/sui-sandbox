#!/usr/bin/env python3
"""Generate a comprehensive wallet profile from pre-fetched Snowflake data + local Move VM.

Data is pre-fetched via igloo-mcp and saved to wallet_data/ as JSON files.
This script reads those files and calls view functions via Rust PyO3 bindings.

Usage:
  python wallet_profile.py [--data-dir wallet_data]
"""

from __future__ import annotations

import argparse
import base64
import json
import re
import struct
import sys
import time
import urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

import sui_sandbox

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

WALLET = "0x1e19b697bb7a332d8e23651cb3fb8247e536eb0846b28602c801e991cb4dbad0"
SUILEND_PKG = "0xf95b06141ed4a174f239417323bde3f209b972f5930d8521ea38a52aff3a6ddf"
ALPHALEND_MARKET_PKG = "0xd631cd66138909636fc3f73ed75820d0c5b76332d1644608ed1c85ea2b8219b4"
ALPHALEND_POSITIONS_TABLE = "0x9923cec7b613e58cc3feec1e8651096ad7970c0b4ef28b805c7d97fe58ff91ba"
SUI_RPC = "https://fullnode.mainnet.sui.io:443"
CLOCK_ID = "0x0000000000000000000000000000000000000000000000000000000000000006"

TOKEN_DECIMALS: dict[str, int] = {
    "SUI": 9, "USDC": 6, "USDT": 6, "WETH": 8, "WBTC": 8,
    "stSUI": 9, "STSUI": 9, "haSUI": 9, "afSUI": 9, "vSUI": 9, "sSUI": 9,
    "NAVX": 9, "DEEP": 6, "BLUE": 9, "CETUS": 9, "SCA": 9,
    "BUCK": 9, "FUD": 5, "TURBOS": 9, "BLUB": 2, "NS": 6,
    "ALKIMI": 9, "BOYSS": 9, "HBD": 9, "ySUI": 9, "ALPHA": 9,
}

# Suilend Decimal: value / 10^18 = real number
SUILEND_DECIMAL_SCALE = 10**18


# ---------------------------------------------------------------------------
# Data loading (from pre-fetched JSON files)
# ---------------------------------------------------------------------------

def load_json(path: Path) -> list[dict]:
    with open(path) as f:
        return json.load(f)


def load_json_optional(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return load_json(path)


# ---------------------------------------------------------------------------
# Type parsing helpers
# ---------------------------------------------------------------------------

def parse_coin_type(full_type: str) -> str | None:
    m = re.match(r"0x[0-9a-f]+::coin::Coin<(.+)>$", full_type)
    return m.group(1) if m else None


def token_symbol(coin_inner_type: str) -> str:
    parts = coin_inner_type.split("::")
    return parts[-1] if parts else coin_inner_type


def format_balance(raw: int, symbol: str) -> str:
    decimals = TOKEN_DECIMALS.get(symbol, 9)
    value = raw / (10 ** decimals)
    if value >= 1_000_000:
        return f"{value:,.2f}"
    elif value >= 1:
        return f"{value:,.4f}"
    else:
        return f"{value:.8f}"


def parse_pool_type_arg(cap_type: str) -> str | None:
    m = re.search(r"ObligationOwnerCap<(.+)>$", cap_type)
    return m.group(1) if m else None


def pool_name(pool_type: str) -> str:
    parts = pool_type.split("::")
    return parts[-1] if parts else pool_type


def suilend_decimal(value_str: str) -> float:
    """Convert Suilend Decimal string to float (value / 10^18)."""
    return int(value_str) / SUILEND_DECIMAL_SCALE


# ---------------------------------------------------------------------------
# Sui RPC helpers
# ---------------------------------------------------------------------------

def sui_rpc(method: str, params: list) -> dict:
    """Call Sui JSON-RPC endpoint."""
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(SUI_RPC, data=payload, headers={"Content-Type": "application/json"})
    resp = urllib.request.urlopen(req, timeout=15)
    return json.loads(resp.read())


def fetch_dynamic_object_field_bcs(parent_id: str, key_id: str) -> tuple[bytes, dict]:
    """Fetch a dynamic object field's inner BCS bytes and JSON content.

    Returns (inner_bcs_bytes, content_fields) where inner_bcs_bytes has the
    Field<ID, V> wrapper stripped (skip first 64 bytes).
    """
    # Step 1: Get wrapper object ID via getDynamicFieldObject
    data = sui_rpc("suix_getDynamicFieldObject", [parent_id, {"type": "0x2::object::ID", "value": key_id}])
    obj = data["result"]["data"]
    wrapper_id = obj["objectId"]
    content = obj.get("content", {})
    value_fields = content.get("fields", {}).get("value", {}).get("fields", {})

    # Step 2: Get BCS of wrapper
    data2 = sui_rpc("sui_getObject", [wrapper_id, {"showBcs": True}])
    bcs_b64 = data2["result"]["data"]["bcs"]["bcsBytes"]
    field_bcs = base64.b64decode(bcs_b64)

    # Strip Field<ID, Position> header: 32 bytes UID + 32 bytes name(ID)
    inner_bcs = field_bcs[64:]
    return inner_bcs, value_fields


def fetch_object_bcs(object_id: str) -> bytes:
    """Fetch a top-level object's BCS bytes."""
    data = sui_rpc("sui_getObject", [object_id, {"showBcs": True}])
    return base64.b64decode(data["result"]["data"]["bcs"]["bcsBytes"])


# ---------------------------------------------------------------------------
# BCS decoding
# ---------------------------------------------------------------------------

def decode_u64(bcs_bytes: bytes) -> int:
    return struct.unpack("<Q", bcs_bytes[:8])[0]


def decode_bool(bcs_bytes: bytes) -> bool:
    return bcs_bytes[0] != 0


# ---------------------------------------------------------------------------
# Package bytecode cache
# ---------------------------------------------------------------------------

_pkg_cache: dict[str, dict[str, list[bytes]]] = {}


def get_package_bytecodes(package_id: str, resolve_deps: bool = True) -> dict[str, list[bytes]]:
    if package_id in _pkg_cache:
        return _pkg_cache[package_id]

    result = sui_sandbox.fetch_package_bytecodes(
        package_id, resolve_deps=resolve_deps
    )
    decoded: dict[str, list[bytes]] = {}
    for pkg_id, b64_modules in result["packages"].items():
        decoded[pkg_id] = [base64.b64decode(b) for b in b64_modules]

    _pkg_cache[package_id] = decoded
    return decoded


# ---------------------------------------------------------------------------
# View function calling
# ---------------------------------------------------------------------------

def call_coin_value(
    object_id: str,
    coin_type: str,
    inner_type: str,
    object_json: str,
    pkg_bytecodes: dict[str, list[bytes]],
) -> int | None:
    all_modules = []
    for modules in pkg_bytecodes.values():
        all_modules.extend(modules)

    try:
        bcs_bytes = sui_sandbox.json_to_bcs(coin_type, object_json, all_modules)
    except Exception as e:
        print(f"    json_to_bcs error for {object_id[:16]}...: {e}", file=sys.stderr)
        return None

    try:
        result = sui_sandbox.call_view_function(
            "0x2", "coin", "value",
            type_args=[inner_type],
            object_inputs=[{
                "object_id": object_id,
                "bcs_bytes": bcs_bytes,
                "type_tag": coin_type,
                "is_shared": False,
            }],
            package_bytecodes={
                pkg_id: modules for pkg_id, modules in pkg_bytecodes.items()
            },
            fetch_deps=False,  # bytecodes pre-fetched; framework (0x2) always bundled
        )

        if result.get("success") and result.get("return_values"):
            raw_bytes = base64.b64decode(result["return_values"][0])
            return decode_u64(raw_bytes)
        else:
            err = result.get("error", "unknown")
            print(f"    VM error for {object_id[:16]}...: {err}", file=sys.stderr)
            return None
    except Exception as e:
        print(f"    call error for {object_id[:16]}...: {e}", file=sys.stderr)
        return None


def call_suilend_view(
    fn_name: str,
    obligation_id: str,
    obligation_type: str,
    obligation_json: str,
    pool_type_arg: str,
    pkg_bytecodes: dict[str, list[bytes]],
) -> dict | None:
    all_modules = []
    for modules in pkg_bytecodes.values():
        all_modules.extend(modules)

    try:
        bcs_bytes = sui_sandbox.json_to_bcs(
            obligation_type, obligation_json, all_modules
        )
    except Exception as e:
        print(f"    json_to_bcs error for obligation: {e}", file=sys.stderr)
        return None

    try:
        result = sui_sandbox.call_view_function(
            SUILEND_PKG, "obligation", fn_name,
            type_args=[pool_type_arg],
            object_inputs=[{
                "object_id": obligation_id,
                "bcs_bytes": bcs_bytes,
                "type_tag": obligation_type,
                "is_shared": True,
            }],
            package_bytecodes={
                pkg_id: modules for pkg_id, modules in pkg_bytecodes.items()
            },
            fetch_deps=True,
        )
        return result
    except Exception as e:
        print(f"    call error for {fn_name}: {e}", file=sys.stderr)
        return None


# ---------------------------------------------------------------------------
# Object reference extraction
# ---------------------------------------------------------------------------

def extract_object_refs(json_obj: dict) -> list[str]:
    refs = []
    for key, value in json_obj.items():
        if key == "id":
            continue
        if isinstance(value, str) and re.match(r"^0x[a-f0-9]{64}$", value):
            refs.append(value)
        elif isinstance(value, dict):
            refs.extend(extract_object_refs(value))
        elif isinstance(value, list):
            for item in value:
                if isinstance(item, dict):
                    refs.extend(extract_object_refs(item))
                elif isinstance(item, str) and re.match(r"^0x[a-f0-9]{64}$", item):
                    refs.append(item)
    return refs


# ---------------------------------------------------------------------------
# DeFi Position Parsers
# ---------------------------------------------------------------------------

def parse_suilend_position(obl_obj: dict, cap_info: dict, suilend_bytecodes: dict) -> dict | None:
    """Parse a Suilend obligation into a position dict. Returns None if zero-value."""
    obl_json = json.loads(obl_obj["OBJECT_JSON"])
    deposits = obl_json.get("deposits", [])
    borrows = obl_json.get("borrows", [])

    # Filter out zero-value obligations
    if len(deposits) == 0 and len(borrows) == 0:
        return None

    # Compute deposited/borrowed USD from Decimal fields
    total_dep_usd = 0.0
    deposit_details = []
    for dep in deposits:
        if not isinstance(dep, dict):
            continue
        mv = dep.get("market_value", {})
        mv_usd = suilend_decimal(mv.get("value", "0")) if isinstance(mv, dict) else 0
        total_dep_usd += mv_usd
        deposit_details.append({
            "reserve_index": dep.get("reserve_array_index"),
            "ctoken_amount": dep.get("deposited_ctoken_amount", "0"),
            "market_value_usd": round(mv_usd, 2),
        })

    total_bor_usd = 0.0
    borrow_details = []
    for bor in borrows:
        if not isinstance(bor, dict):
            continue
        mv = bor.get("market_value", {})
        mv_usd = suilend_decimal(mv.get("value", "0")) if isinstance(mv, dict) else 0
        total_bor_usd += mv_usd
        ba = bor.get("borrowed_amount", {})
        ba_val = suilend_decimal(ba.get("value", "0")) if isinstance(ba, dict) else 0
        borrow_details.append({
            "reserve_index": bor.get("reserve_array_index"),
            "borrowed_amount": ba_val,
            "market_value_usd": round(mv_usd, 2),
        })

    net_value_usd = total_dep_usd - total_bor_usd

    position = {
        "protocol": "Suilend",
        "pool": cap_info["pool_name"],
        "obligation_id": obl_obj["OBJECT_ID"],
        "deposited_usd": round(total_dep_usd, 2),
        "borrowed_usd": round(total_bor_usd, 2),
        "net_value_usd": round(net_value_usd, 2),
        "deposit_count": len(deposits),
        "borrow_count": len(borrows),
        "deposits": deposit_details,
        "borrows": borrow_details,
    }

    # Call view functions concurrently (is_healthy, is_liquidatable)
    pool_type = cap_info["pool_type"]
    with ThreadPoolExecutor(max_workers=2) as executor:
        fn_futures = {
            executor.submit(
                call_suilend_view,
                fn_name, obl_obj["OBJECT_ID"], obl_obj["TYPE"], obl_obj["OBJECT_JSON"],
                pool_type, suilend_bytecodes,
            ): fn_name
            for fn_name in ("is_healthy", "is_liquidatable")
        }
        for future in as_completed(fn_futures):
            fn_name = fn_futures[future]
            try:
                result = future.result()
                if result and result.get("success") and result.get("return_values"):
                    raw_bytes = base64.b64decode(result["return_values"][0])
                    position[fn_name] = decode_bool(raw_bytes)
                else:
                    err = result.get("error") if result else "call failed"
                    position[fn_name] = f"error: {err}"
            except Exception as e:
                position[fn_name] = f"error: {e}"

    return position


def parse_alphafi_position(cap_obj: dict, shared_objects: dict[str, dict]) -> dict | None:
    """Parse AlphaFi/AlphaLend position from cap + pool data. Returns None if zero-value."""
    cap_type = cap_obj["TYPE"]
    cap_json = json.loads(cap_obj["OBJECT_JSON"])

    # AlphaFi Bluefin Receipt
    if "Receipt" in cap_type and "alphafi" in cap_type.lower():
        xtokens = int(cap_json.get("xTokenBalance", "0"))
        if xtokens == 0:
            return None

        # Find the pool from referenced IDs
        pool_id = cap_json.get("pool_id", "")
        pool = shared_objects.get(pool_id)
        pool_name_str = "Unknown Pool"
        pool_supply = 0
        pool_invested = 0

        if pool:
            pool_json = json.loads(pool["OBJECT_JSON"])
            pool_supply = int(pool_json.get("xTokenSupply", "0"))
            pool_invested = int(pool_json.get("tokensInvested", "0"))
            # Try to extract pool name from type
            pool_type = pool.get("TYPE", "")
            if "stsui_sui" in pool_type.lower() or "bluefin" in pool_type.lower():
                pool_name_str = "Bluefin stSUI-SUI"
            elif "alpha" in pool_type.lower():
                pool_name_str = "ALPHA"
            else:
                pool_name_str = pool_type.split("::")[-1].split("<")[0] if "::" in pool_type else pool_type

        share_pct = (xtokens / pool_supply * 100) if pool_supply > 0 else 0

        return {
            "protocol": "AlphaFi",
            "pool": pool_name_str,
            "object_id": cap_obj["OBJECT_ID"],
            "xtokens": xtokens,
            "pool_supply": pool_supply,
            "pool_invested": pool_invested,
            "share_pct": round(share_pct, 4),
        }

    # AlphaFi Ember Position (ALPHA pool)
    if "alphafi_ember_pool::Position" in cap_type:
        xtokens = int(cap_json.get("xtokens", "0"))
        if xtokens == 0:
            return None

        pool_id = cap_json.get("pool_id", "")
        pool = shared_objects.get(pool_id)

        # Pending rewards
        pending = cap_json.get("pending_rewards", {}).get("contents", [])
        pending_rewards = {}
        for entry in pending:
            key = entry.get("key", {}).get("name", "")
            symbol = key.split("::")[-1] if "::" in key else key
            value = int(entry.get("value", "0"))
            if value > 0:
                decimals = TOKEN_DECIMALS.get(symbol, 9)
                pending_rewards[symbol] = value / (10 ** decimals)

        exchange_rate = 1.0
        if pool:
            pool_json = json.loads(pool["OBJECT_JSON"])
            er = pool_json.get("current_exchange_rate", {})
            if isinstance(er, dict) and "value" in er:
                exchange_rate = suilend_decimal(er["value"])  # Also uses 10^18 scaling

        # Token amount = xtokens * exchange_rate
        token_amount = xtokens * exchange_rate
        coin_type = cap_json.get("coin_type", {}).get("name", "")
        symbol = coin_type.split("::")[-1] if "::" in coin_type else "?"
        decimals = TOKEN_DECIMALS.get(symbol, 9)

        return {
            "protocol": "AlphaFi",
            "pool": f"Ember {symbol}",
            "object_id": cap_obj["OBJECT_ID"],
            "xtokens": xtokens,
            "token_amount": round(token_amount / (10 ** decimals), 4),
            "token_symbol": symbol,
            "exchange_rate": round(exchange_rate, 6),
            "pending_rewards": pending_rewards,
        }

    # AlphaLend Slush Position
    if "alphalend_slush_pool::Position" in cap_type:
        principal = int(cap_json.get("principal", "0"))
        xtokens = int(cap_json.get("xtokens", "0"))
        if principal == 0 and xtokens == 0:
            return None

        coin_type = cap_json.get("coin_type", {}).get("name", "")
        symbol = coin_type.split("::")[-1] if "::" in coin_type else "?"
        decimals = TOKEN_DECIMALS.get(symbol, 9)

        return {
            "protocol": "AlphaLend",
            "pool": f"Slush {symbol}",
            "object_id": cap_obj["OBJECT_ID"],
            "principal": round(principal / (10 ** decimals), 4),
            "xtokens": xtokens,
            "token_symbol": symbol,
        }

    return None


def parse_scallop_position(cap_obj: dict, shared_objects: dict[str, dict]) -> dict | None:
    """Parse Scallop position. Returns None if zero-value."""
    cap_json = json.loads(cap_obj["OBJECT_JSON"])
    cap_type = cap_obj["TYPE"]

    if "ObligationKey" in cap_type or "obligation" in cap_type.lower():
        obl_id = cap_json.get("ownership_of", "")
        if not obl_id:
            obl_id = extract_object_refs(cap_json)[0] if extract_object_refs(cap_json) else ""
        obl = shared_objects.get(obl_id)
        if obl:
            obl_json = json.loads(obl["OBJECT_JSON"])
            collaterals = obl_json.get("collaterals", {})
            debts = obl_json.get("debts", {})
            balances = obl_json.get("balances", {})

            # Check sizes
            coll_size = 0
            debt_size = 0
            bal_size = 0
            if isinstance(collaterals, dict):
                tbl = collaterals.get("table", {}) if isinstance(collaterals.get("table"), dict) else {}
                coll_size = int(tbl.get("size", "0"))
            if isinstance(debts, dict):
                tbl = debts.get("table", {}) if isinstance(debts.get("table"), dict) else {}
                debt_size = int(tbl.get("size", "0"))
            if isinstance(balances, dict):
                bag = balances.get("bag", {}) if isinstance(balances.get("bag"), dict) else {}
                bal_size = int(bag.get("size", "0"))

            # Filter out if all zero
            rewards_point = int(obl_json.get("rewards_point", "0"))
            if coll_size == 0 and debt_size == 0 and bal_size == 0 and rewards_point == 0:
                return None

            # Even with bal_size > 0, if the balance is 0 (checked via dynamic field), filter out
            # We already checked this: the balance bag has 1 entry with value=0
            if coll_size == 0 and debt_size == 0 and rewards_point > 0:
                return {
                    "protocol": "Scallop",
                    "pool": "Obligation",
                    "object_id": obl_id,
                    "collaterals": coll_size,
                    "debts": debt_size,
                    "balance_entries": bal_size,
                    "rewards_point": rewards_point,
                    "note": "Rewards points only, no active positions",
                }

        return None

    # VeScaKey - just a reference key, no intrinsic value
    if "VeScaKey" in cap_type or "ve_sca" in cap_type.lower():
        return None

    return None


def parse_deepbook_position(cap_obj: dict) -> dict | None:
    """Parse DeepBook position. Returns None if zero-value."""
    cap_json = json.loads(cap_obj["OBJECT_JSON"])
    cap_type = cap_obj["TYPE"]

    if "SupplierCap" in cap_type:
        return {
            "protocol": "DeepBook",
            "pool": "Supplier",
            "object_id": cap_obj["OBJECT_ID"],
            "note": "Pool supplier position — value stored in shared margin pool",
        }

    return None


def parse_alphalend_market_position(
    cap_obj: dict,
    pkg_bytecodes: dict[str, list[bytes]],
) -> dict | None:
    """Parse AlphaLend Main Market position via RPC object fetch + local VM view functions.

    The position is a dynamic object field inside the LendingProtocol's
    positions ObjectTable.  We fetch it via suix_getDynamicFieldObject,
    read its JSON fields for USD values, and call position::is_healthy
    via the local Move VM.
    """
    cap_json = json.loads(cap_obj["OBJECT_JSON"])
    position_id = cap_json.get("position_id", "")
    if not position_id:
        return None

    # --- Fetch Position from RPC (dynamic object field of positions table) ---
    try:
        position_bcs, pos_fields = fetch_dynamic_object_field_bcs(
            ALPHALEND_POSITIONS_TABLE, position_id
        )
    except Exception as e:
        print(f"    Failed to fetch AlphaLend position {position_id[:16]}...: {e}", file=sys.stderr)
        return {
            "protocol": "AlphaLend",
            "pool": "Main Market",
            "cap_id": cap_obj["OBJECT_ID"],
            "position_id": position_id,
            "note": f"RPC fetch failed: {e}",
        }

    # --- Parse USD values from Position fields (18-decimal Number type) ---
    def parse_number(field) -> float:
        if isinstance(field, dict) and "fields" in field:
            return int(field["fields"].get("value", "0")) / SUILEND_DECIMAL_SCALE
        return 0.0

    collateral_usd = parse_number(pos_fields.get("total_collateral_usd"))
    loan_usd = parse_number(pos_fields.get("total_loan_usd"))
    net_usd = collateral_usd - loan_usd
    safe_collateral_usd = parse_number(pos_fields.get("safe_collateral_usd"))
    liquidation_value = parse_number(pos_fields.get("liquidation_value"))
    additional_borrow = parse_number(pos_fields.get("additional_permissible_borrow_usd"))

    # Stored flags (set by last refresh())
    stored_liquidatable = pos_fields.get("is_position_liquidatable")

    # Parse collateral and loan details
    collateral_entries = []
    coll_contents = pos_fields.get("collaterals", {}).get("fields", {}).get("contents", [])
    for c in coll_contents:
        cf = c.get("fields", {})
        collateral_entries.append({
            "market_id": int(cf.get("key", 0)),
            "amount": int(cf.get("value", 0)),
        })

    loan_entries = []
    for loan in pos_fields.get("loans", []):
        lf = loan.get("fields", {})
        loan_entries.append({
            "market_id": int(lf.get("market_id", 0)),
            "amount": int(lf.get("amount", 0)),
        })

    # --- Call is_healthy via local Move VM ---
    is_healthy = None
    try:
        result = sui_sandbox.call_view_function(
            ALPHALEND_MARKET_PKG, "position", "is_healthy",
            type_args=[],
            object_inputs=[{
                "object_id": position_id,
                "bcs_bytes": position_bcs,
                "type_tag": f"{ALPHALEND_MARKET_PKG}::position::Position",
                "is_shared": False,
            }],
            package_bytecodes=pkg_bytecodes,
            fetch_deps=False,
        )
        if result.get("success") and result.get("return_values"):
            raw = base64.b64decode(result["return_values"][0])
            is_healthy = raw[0] != 0
        else:
            is_healthy = f"VM error: {result.get('error', 'unknown')}"
    except Exception as e:
        is_healthy = f"error: {e}"

    return {
        "protocol": "AlphaLend",
        "pool": "Main Market",
        "cap_id": cap_obj["OBJECT_ID"],
        "position_id": position_id,
        "collateral_usd": round(collateral_usd, 2),
        "loan_usd": round(loan_usd, 2),
        "net_value_usd": round(net_usd, 2),
        "safe_collateral_usd": round(safe_collateral_usd, 2),
        "liquidation_value": round(liquidation_value, 2),
        "additional_borrow_usd": round(additional_borrow, 2),
        "is_healthy": is_healthy,
        "is_liquidatable": stored_liquidatable,
        "collaterals": collateral_entries,
        "loans": loan_entries,
        "data_source": "rpc+vm",
    }


# ---------------------------------------------------------------------------
# Profile building
# ---------------------------------------------------------------------------

def build_profile(data_dir: Path) -> dict:
    profile: dict = {
        "wallet": WALLET,
        "coins": [],
        "defi_positions": [],
        "nfts": [],
        "other": [],
        "summary": {},
    }

    # ── Phase 1: Load pre-fetched data ────────────────────────────────────
    print(f"\n[1/5] Loading pre-fetched Snowflake data...")
    coins_data = load_json(data_dir / "coins.json")
    all_objects_data = load_json(data_dir / "all_objects.json")
    obligations_data = load_json_optional(data_dir / "suilend_obligations.json")
    shared_objects_data = load_json_optional(data_dir / "defi_shared_objects.json")

    # Build shared objects lookup by ID
    shared_by_id: dict[str, dict] = {}
    for obj in shared_objects_data:
        if obj.get("OBJECT_JSON"):
            shared_by_id[obj["OBJECT_ID"]] = obj


    # coins_data has just coin objects
    coin_ids = {c["OBJECT_ID"] for c in coins_data}
    non_coins = [o for o in all_objects_data if o["OBJECT_ID"] not in coin_ids]

    print(f"      Coins: {len(coins_data)}, Non-coins: {len(non_coins)}, "
          f"Shared objects: {len(shared_by_id)}, "
          f"Suilend obligations: {len(obligations_data)}")

    # ── Phase 2: Classify non-coin objects ────────────────────────────────
    print(f"\n[2/5] Classifying objects...")
    caps: list[dict] = []
    nfts: list[dict] = []
    other: list[dict] = []

    for obj in non_coins:
        obj_type = obj["TYPE"]
        if any(kw in obj_type for kw in ["Cap", "Key", "Receipt", "Position"]):
            caps.append(obj)
        elif any(kw in obj_type for kw in ["NFT", "Nft", "nft", "Hero", "Collection", "Sbt", "TICKET"]):
            nfts.append(obj)
        else:
            other.append(obj)

    print(f"      Caps/DeFi: {len(caps)}, NFTs: {len(nfts)}, Other: {len(other)}")

    # ── Phase 3: Coin balances via coin::value ────────────────────────────
    print(f"\n[3/5] Computing coin balances via local Move VM...")
    t0 = time.time()

    coins_by_type: dict[str, list[dict]] = defaultdict(list)
    for c in coins_data:
        inner = parse_coin_type(c["TYPE"])
        if inner:
            coins_by_type[inner].append(c)

    coin_pkg_ids = set()
    for inner_type in coins_by_type:
        parts = inner_type.split("::")
        if parts:
            coin_pkg_ids.add(parts[0])

    print(f"      {len(coins_by_type)} token types across {len(coin_pkg_ids)} packages")
    print(f"      Fetching package bytecodes...")

    all_coin_bytecodes: dict[str, list[bytes]] = {}
    for pkg_id in coin_pkg_ids:
        try:
            pkgs = get_package_bytecodes(pkg_id, resolve_deps=True)
            all_coin_bytecodes.update(pkgs)
        except Exception as e:
            print(f"      Warning: failed to fetch {pkg_id[:16]}...: {e}", file=sys.stderr)

    print(f"      Loaded {len(all_coin_bytecodes)} packages, calling coin::value...")

    balance_results: list[dict] = []
    total_calls = sum(len(objs) for objs in coins_by_type.values())

    # Build flat list of (inner_type, coin_obj) for concurrent execution
    coin_tasks: list[tuple[str, dict]] = []
    for inner_type, coin_objs in sorted(coins_by_type.items(), key=lambda x: len(x[1]), reverse=True):
        for c in coin_objs:
            coin_tasks.append((inner_type, c))

    # Execute coin::value calls concurrently (GIL is released in Rust)
    MAX_WORKERS = 8
    print(f"      Calling coin::value for {total_calls} coins ({MAX_WORKERS} threads)...")
    coin_balances: dict[str, list[int]] = defaultdict(list)  # inner_type -> [balance, ...]
    completed = 0

    with ThreadPoolExecutor(max_workers=MAX_WORKERS) as executor:
        futures = {
            executor.submit(
                call_coin_value,
                c["OBJECT_ID"], c["TYPE"], inner_type,
                c["OBJECT_JSON"], all_coin_bytecodes,
            ): inner_type
            for inner_type, c in coin_tasks
        }
        for future in as_completed(futures):
            inner_type = futures[future]
            completed += 1
            try:
                balance = future.result()
                if balance is not None:
                    coin_balances[inner_type].append(balance)
            except Exception as e:
                print(f"      Thread error: {e}", file=sys.stderr)
            if completed % 20 == 0 or completed == total_calls:
                print(f"      [{completed}/{total_calls}] completed")

    for inner_type, balances in coin_balances.items():
        total_raw = sum(balances)
        if total_raw > 0:
            symbol = token_symbol(inner_type)
            coin_count = len(coins_by_type[inner_type])
            balance_results.append({
                "symbol": symbol,
                "type": inner_type,
                "raw_balance": total_raw,
                "formatted": format_balance(total_raw, symbol),
                "coin_count": coin_count,
                "decimals": TOKEN_DECIMALS.get(symbol, 9),
            })

    balance_results.sort(key=lambda x: x["raw_balance"] / (10 ** x["decimals"]), reverse=True)
    profile["coins"] = balance_results
    elapsed = time.time() - t0
    print(f"      Done: {len(balance_results)} token balances ({elapsed:.1f}s)")

    # ── Phase 4: DeFi positions ───────────────────────────────────────────
    print(f"\n[4/5] Analyzing DeFi positions...")
    t0 = time.time()

    # --- Suilend ---
    suilend_caps = [
        c for c in caps
        if "ObligationOwnerCap" in c["TYPE"] and SUILEND_PKG in c["TYPE"]
    ]

    if suilend_caps:
        print(f"      Suilend: {len(suilend_caps)} obligation caps")
        suilend_bytecodes = get_package_bytecodes(SUILEND_PKG, resolve_deps=True)

        cap_to_obligation: dict[str, dict] = {}
        for cap in suilend_caps:
            cap_json = json.loads(cap["OBJECT_JSON"])
            obl_id = cap_json.get("obligation_id")
            pool_type = parse_pool_type_arg(cap["TYPE"])
            if obl_id and pool_type:
                cap_to_obligation[obl_id] = {
                    "cap_id": cap["OBJECT_ID"],
                    "pool_type": pool_type,
                    "pool_name": pool_name(pool_type),
                }

        obl_by_id = {o["OBJECT_ID"]: o for o in obligations_data}

        for obl_id, cap_info in cap_to_obligation.items():
            obl_obj = obl_by_id.get(obl_id)
            if not obl_obj:
                continue

            pos = parse_suilend_position(obl_obj, cap_info, suilend_bytecodes)
            if pos:
                profile["defi_positions"].append(pos)
                print(f"        {cap_info['pool_name']}: ${pos['deposited_usd']:,.2f} dep / "
                      f"${pos['borrowed_usd']:,.2f} bor = ${pos['net_value_usd']:,.2f} net")
            else:
                print(f"        {cap_info['pool_name']}: FILTERED (zero value)")

    # --- AlphaFi / AlphaLend ---
    alphafi_caps = [
        c for c in caps
        if any(kw in c["TYPE"].lower() for kw in ["alphafi", "alphalend"])
        and c not in suilend_caps
    ]

    for cap in alphafi_caps:
        pos = parse_alphafi_position(cap, shared_by_id)
        if pos:
            profile["defi_positions"].append(pos)
            pool = pos.get("pool", "?")
            if "principal" in pos:
                print(f"        {pos['protocol']} {pool}: {pos['principal']} {pos.get('token_symbol', '?')}")
            elif "token_amount" in pos:
                print(f"        {pos['protocol']} {pool}: {pos['token_amount']} {pos.get('token_symbol', '?')}")
            elif "share_pct" in pos:
                print(f"        {pos['protocol']} {pool}: {pos['share_pct']}% pool share")

    # Also check shared objects that are Position types (linked from caps)
    seen_positions = {p.get("object_id") for p in profile["defi_positions"]}
    for obj_id, obj in shared_by_id.items():
        if obj_id in seen_positions:
            continue
        obj_type = obj.get("TYPE", "")
        if "::Position" in obj_type and ("alphafi" in obj_type.lower() or "alphalend" in obj_type.lower()):
            pos = parse_alphafi_position(obj, shared_by_id)
            if pos:
                profile["defi_positions"].append(pos)
                pool = pos.get("pool", "?")
                if "principal" in pos:
                    print(f"        {pos['protocol']} {pool}: {pos['principal']} {pos.get('token_symbol', '?')}")
                elif "token_amount" in pos:
                    print(f"        {pos['protocol']} {pool}: {pos['token_amount']} {pos.get('token_symbol', '?')}")
                elif "share_pct" in pos:
                    print(f"        {pos['protocol']} {pool}: {pos['share_pct']}% pool share")

    # --- Scallop ---
    scallop_caps = [
        c for c in caps
        if any(kw in c["TYPE"].lower() for kw in ["scallop", "ve_sca", "obligation_key"])
        and c not in suilend_caps
    ]

    for cap in scallop_caps:
        pos = parse_scallop_position(cap, shared_by_id)
        if pos:
            profile["defi_positions"].append(pos)
            print(f"        {pos['protocol']}: {pos.get('note', '')}")
        else:
            cap_short = cap["TYPE"].split("::")[-1]
            print(f"        Scallop {cap_short}: FILTERED (zero value)")

    # --- DeepBook ---
    deepbook_caps = [
        c for c in caps
        if "deepbook" in c["TYPE"].lower() or "SupplierCap" in c["TYPE"]
    ]

    for cap in deepbook_caps:
        pos = parse_deepbook_position(cap)
        if pos:
            profile["defi_positions"].append(pos)
            print(f"        {pos['protocol']}: {pos.get('note', '')}")

    # --- AlphaLend Main Market ---
    alphalend_market_caps = [
        c for c in caps
        if ALPHALEND_MARKET_PKG in c["TYPE"] and "PositionCap" in c["TYPE"]
    ]

    if alphalend_market_caps:
        print(f"      AlphaLend Main Market: {len(alphalend_market_caps)} position caps")
        print(f"        Fetching package bytecodes...")
        alphalend_bytecodes = get_package_bytecodes(ALPHALEND_MARKET_PKG, resolve_deps=True)

    for cap in alphalend_market_caps:
        pos = parse_alphalend_market_position(cap, alphalend_bytecodes)
        if pos:
            profile["defi_positions"].append(pos)
            if "collateral_usd" in pos:
                print(f"        AlphaLend Main Market: "
                      f"${pos['collateral_usd']:,.2f} collateral / "
                      f"${pos['loan_usd']:,.2f} borrowed = "
                      f"${pos['net_value_usd']:,.2f} net  "
                      f"[is_healthy={pos['is_healthy']} via VM]")
            else:
                print(f"        AlphaLend Main Market: {pos.get('note', '')}")

    elapsed = time.time() - t0
    print(f"      Done: {len(profile['defi_positions'])} active DeFi positions ({elapsed:.1f}s)")

    # ── Phase 5: NFTs and summary ─────────────────────────────────────────
    print(f"\n[5/5] Building summary...")

    for nft in nfts:
        nft_type = nft["TYPE"]
        short_type = nft_type.split("::")[-1] if "::" in nft_type else nft_type
        module = nft_type.split("::")[-2] if nft_type.count("::") >= 2 else ""
        profile["nfts"].append({
            "type": short_type,
            "module": module,
            "object_id": nft["OBJECT_ID"],
        })

    for o in other:
        o_type = o["TYPE"]
        short_type = o_type.split("::")[-1] if "::" in o_type else o_type
        profile["other"].append({
            "type": short_type,
            "full_type": o_type,
            "object_id": o["OBJECT_ID"],
        })

    total_objects = len(coins_data) + len(non_coins)
    profile["summary"] = {
        "total_objects": total_objects,
        "coin_types": len(balance_results),
        "total_coins": len(coins_data),
        "defi_positions": len(profile["defi_positions"]),
        "nfts": len(nfts),
        "other": len(other),
    }

    return profile


# ---------------------------------------------------------------------------
# Display
# ---------------------------------------------------------------------------

def print_profile(profile: dict):
    w = profile["wallet"]
    s = profile["summary"]

    print(f"\n{'='*70}")
    print(f"  WALLET PROFILE: {w[:10]}...{w[-6:]}")
    print(f"{'='*70}")
    print(f"  Objects: {s['total_objects']}  |  Tokens: {s['coin_types']}  |  "
          f"DeFi: {s['defi_positions']}  |  NFTs: {s['nfts']}")
    print(f"{'='*70}")

    # Coin balances
    print(f"\n  TOKEN BALANCES ({s['coin_types']} tokens, {s['total_coins']} coin objects)")
    print(f"  {'':─<66}")
    print(f"  {'Token':<15} {'Balance':>25}  {'Coins':>5}")
    print(f"  {'':─<66}")
    for coin in profile["coins"]:
        sym = coin["symbol"]
        bal = coin["formatted"]
        cnt = coin["coin_count"]
        print(f"  {sym:<15} {bal:>25}  {cnt:>5}")

    # DeFi positions
    if profile["defi_positions"]:
        print(f"\n  DEFI POSITIONS ({len(profile['defi_positions'])} active)")
        print(f"  {'':─<66}")
        for pos in profile["defi_positions"]:
            protocol = pos.get("protocol", "Unknown")

            if protocol == "Suilend":
                pool = pos.get("pool", "?")
                healthy = pos.get("is_healthy", "?")
                liquidatable = pos.get("is_liquidatable", "?")
                print(f"  [{protocol}] {pool}")
                print(f"    Deposited: ${pos['deposited_usd']:,.2f}  |  "
                      f"Borrowed: ${pos['borrowed_usd']:,.2f}  |  "
                      f"Net: ${pos['net_value_usd']:,.2f}")
                print(f"    Healthy: {healthy}  |  Liquidatable: {liquidatable}")
                for dep in pos.get("deposits", []):
                    print(f"      Deposit (reserve {dep['reserve_index']}): "
                          f"${dep['market_value_usd']:,.2f}")
                for bor in pos.get("borrows", []):
                    print(f"      Borrow (reserve {bor['reserve_index']}): "
                          f"${bor['market_value_usd']:,.2f}")
                print()

            elif protocol in ("AlphaFi", "AlphaLend"):
                pool = pos.get("pool", "?")
                print(f"  [{protocol}] {pool}")
                if "collateral_usd" in pos:
                    # AlphaLend Main Market (lending position)
                    healthy = pos.get("is_healthy", "?")
                    liquidatable = pos.get("is_liquidatable", "?")
                    src = pos.get("data_source", "")
                    src_tag = f" [{src}]" if src else ""
                    print(f"    Collateral: ${pos['collateral_usd']:,.2f}  |  "
                          f"Borrowed: ${pos['loan_usd']:,.2f}  |  "
                          f"Net: ${pos['net_value_usd']:,.2f}")
                    print(f"    Healthy: {healthy}  |  Liquidatable: {liquidatable}{src_tag}")
                    print(f"    Safe collateral: ${pos.get('safe_collateral_usd', 0):,.2f}  |  "
                          f"Additional borrow: ${pos.get('additional_borrow_usd', 0):,.2f}")
                    for coll in pos.get("collaterals", []):
                        print(f"      Collateral (market {coll['market_id']}): "
                              f"{coll['amount']:,} tokens")
                    for loan in pos.get("loans", []):
                        print(f"      Loan (market {loan['market_id']}): "
                              f"{loan['amount']:,} tokens")
                elif "principal" in pos:
                    print(f"    Principal: {pos['principal']} {pos.get('token_symbol', '')}")
                elif "token_amount" in pos:
                    print(f"    Tokens: {pos['token_amount']} {pos.get('token_symbol', '')}")
                    print(f"    Exchange rate: {pos.get('exchange_rate', '?')}")
                elif "share_pct" in pos:
                    print(f"    Pool share: {pos['share_pct']}%")
                if pos.get("pending_rewards"):
                    for sym, amt in pos["pending_rewards"].items():
                        print(f"    Pending reward: {amt:.4f} {sym}")
                if pos.get("note"):
                    print(f"    Note: {pos['note']}")
                print()

            elif protocol == "Scallop":
                print(f"  [{protocol}] {pos.get('pool', '?')}")
                if pos.get("rewards_point"):
                    print(f"    Rewards: {pos['rewards_point']:,} points")
                if pos.get("note"):
                    print(f"    Note: {pos['note']}")
                print()

            elif protocol == "DeepBook":
                print(f"  [{protocol}] {pos.get('pool', '?')}")
                if pos.get("note"):
                    print(f"    Note: {pos['note']}")
                print()

            else:
                print(f"  [{protocol}] {pos.get('type', '?')}")
                print(f"    Object: {pos.get('object_id', '?')[:30]}...")
                print()

    # NFTs
    if profile["nfts"]:
        print(f"\n  NFTS ({len(profile['nfts'])})")
        print(f"  {'':─<66}")
        for nft in profile["nfts"]:
            print(f"    {nft['module']}::{nft['type']}  {nft['object_id'][:30]}...")

    print(f"\n{'='*70}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Sui Wallet Profile Generator")
    parser.add_argument("--data-dir", default="wallet_data",
                        help="Directory with pre-fetched Snowflake JSON files")
    parser.add_argument("--output", default=None, help="Output JSON file path")
    args = parser.parse_args()

    data_dir = Path(args.data_dir)
    if not data_dir.exists():
        print(f"Error: data directory '{data_dir}' not found.")
        print(f"Fetch data via igloo-mcp first and save to {data_dir}/")
        sys.exit(1)

    required = ["coins.json", "all_objects.json"]
    for f in required:
        if not (data_dir / f).exists():
            print(f"Error: missing {data_dir / f}")
            sys.exit(1)

    print(f"Sui Wallet Profile Generator")
    print(f"Data: {data_dir}")

    profile = build_profile(data_dir)
    print_profile(profile)

    output_path = args.output or f"wallet_profile.json"
    with open(output_path, "w") as f:
        json.dump(profile, f, indent=2, default=str)
    print(f"\nProfile saved to: {output_path}")


if __name__ == "__main__":
    main()
