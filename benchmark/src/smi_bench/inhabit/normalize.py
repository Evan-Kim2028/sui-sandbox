"""
PTB spec normalization for Phase II benchmark.

Normalizes common LLM formatting mistakes before passing to smi_tx_sim,
allowing the benchmark to focus on planning/inhabitation intelligence
rather than JSON formatting quirks.
"""

from __future__ import annotations

import copy
import re
from collections import Counter
from dataclasses import dataclass, field
from enum import Enum
from typing import Any


class CorrectionType(str, Enum):
    """Categories of formatting corrections applied to PTB specs."""

    ARG_KIND_OBJECT_TO_IMM_OR_OWNED = "arg_kind_object_to_imm_or_owned"
    ARG_KIND_OBJECT_ID_TO_IMM_OR_OWNED = "arg_kind_object_id_to_imm_or_owned"
    INTEGER_STRING_TO_INT = "integer_string_to_int"
    RESULT_REF_STRING_TO_INT = "result_ref_string_to_int"
    ADDRESS_MISSING_0X_PREFIX = "address_missing_0x_prefix"
    NESTED_RESULT_STRING_TO_INT = "nested_result_string_to_int"
    BOOLEAN_STRING_TO_BOOL = "boolean_string_to_bool"


# Integer argument keys in PTB spec
_INT_ARG_KEYS = frozenset({"u8", "u16", "u32", "u64", "u128", "u256"})

# Keys that expect object IDs (addresses)
_OBJECT_ID_KEYS = frozenset({"imm_or_owned_object", "object", "object_id"})

# Hex pattern for addresses without 0x prefix
_HEX_PATTERN = re.compile(r"^[0-9a-fA-F]{1,64}$")


@dataclass
class NormalizationResult:
    """Result of PTB spec normalization."""

    spec: dict[str, Any]
    corrections: list[str] = field(default_factory=list)
    correction_counts: Counter = field(default_factory=Counter)

    @property
    def had_corrections(self) -> bool:
        return len(self.corrections) > 0

    def histogram(self) -> dict[str, int]:
        """Return correction counts as a plain dict for JSON serialization."""
        return dict(self.correction_counts)


def _is_hex_address(s: str) -> bool:
    """Check if string looks like a hex address without 0x prefix."""
    return bool(_HEX_PATTERN.match(s))


def _normalize_address(value: str) -> tuple[str, bool]:
    """
    Normalize an address value, adding 0x prefix if needed.
    Returns (normalized_value, was_corrected).
    """
    if not isinstance(value, str):
        return value, False
    s = value.strip()
    if s.startswith("0x") or s.startswith("0X"):
        return s, False
    if _is_hex_address(s):
        return f"0x{s}", True
    return s, False


def _normalize_integer(value: Any, key: str) -> tuple[Any, str | None]:
    """
    Normalize an integer value, converting string to int if needed.
    Returns (normalized_value, correction_type or None).
    """
    if isinstance(value, int):
        return value, None
    if isinstance(value, str):
        try:
            return int(value), CorrectionType.INTEGER_STRING_TO_INT.value
        except ValueError:
            return value, None
    return value, None


def _normalize_boolean(value: Any) -> tuple[Any, str | None]:
    """
    Normalize a boolean value, converting string to bool if needed.
    Returns (normalized_value, correction_type or None).
    """
    if isinstance(value, bool):
        return value, None
    if isinstance(value, str):
        s = value.strip().lower()
        if s in ("true", "1", "yes"):
            return True, CorrectionType.BOOLEAN_STRING_TO_BOOL.value
        if s in ("false", "0", "no"):
            return False, CorrectionType.BOOLEAN_STRING_TO_BOOL.value
    return value, None


def _normalize_arg(arg: dict[str, Any], call_idx: int, arg_idx: int) -> tuple[dict[str, Any], list[str]]:
    """
    Normalize a single argument in a PTB call.
    Returns (normalized_arg, list_of_corrections).
    """
    if not isinstance(arg, dict):
        return arg, []

    corrections: list[str] = []
    normalized = {}

    for key, value in arg.items():
        new_key = key
        new_value = value

        # Normalize "object" → "imm_or_owned_object"
        if key == "object":
            new_key = "imm_or_owned_object"
            corrections.append(
                f"call[{call_idx}].args[{arg_idx}]: {CorrectionType.ARG_KIND_OBJECT_TO_IMM_OR_OWNED.value}"
            )

        # Normalize "object_id" → "imm_or_owned_object"
        elif key == "object_id":
            new_key = "imm_or_owned_object"
            corrections.append(
                f"call[{call_idx}].args[{arg_idx}]: {CorrectionType.ARG_KIND_OBJECT_ID_TO_IMM_OR_OWNED.value}"
            )

        # Normalize result reference: {"result": "0"} → {"result": 0}
        if key == "result" and isinstance(value, str):
            try:
                new_value = int(value)
                corrections.append(f"call[{call_idx}].args[{arg_idx}]: {CorrectionType.RESULT_REF_STRING_TO_INT.value}")
            except ValueError:
                pass

        # Normalize nested_result reference: {"nested_result": ["0", "1"]} → {"nested_result": [0, 1]}
        elif key == "nested_result" and isinstance(value, list):
            new_list = []
            had_str = False
            for i, item in enumerate(value):
                if isinstance(item, str):
                    try:
                        new_list.append(int(item))
                        had_str = True
                    except ValueError:
                        new_list.append(item)
                else:
                    new_list.append(item)
            if had_str:
                new_value = new_list
                corrections.append(
                    f"call[{call_idx}].args[{arg_idx}]: {CorrectionType.NESTED_RESULT_STRING_TO_INT.value}"
                )

        # Normalize integer arguments: {"u64": "100"} → {"u64": 100}
        elif key in _INT_ARG_KEYS:
            new_value, correction = _normalize_integer(value, key)
            if correction:
                corrections.append(f"call[{call_idx}].args[{arg_idx}]: {correction}")

        # Normalize vector integer arguments: {"vector_u64": ["1", "2"]} → {"vector_u64": [1, 2]}
        elif key.startswith("vector_") and key[7:] in _INT_ARG_KEYS:
            if isinstance(value, list):
                new_list = []
                had_correction = False
                for item in value:
                    norm_item, corr = _normalize_integer(item, key)
                    new_list.append(norm_item)
                    if corr:
                        had_correction = True
                if had_correction:
                    new_value = new_list
                    corrections.append(
                        f"call[{call_idx}].args[{arg_idx}]: {CorrectionType.INTEGER_STRING_TO_INT.value}"
                    )

        # Normalize address values (add 0x prefix if missing)
        elif new_key == "imm_or_owned_object" and isinstance(new_value, str):
            new_value, was_corrected = _normalize_address(new_value)
            if was_corrected:
                corrections.append(
                    f"call[{call_idx}].args[{arg_idx}]: {CorrectionType.ADDRESS_MISSING_0X_PREFIX.value}"
                )

        elif key == "address" and isinstance(value, str):
            new_value, was_corrected = _normalize_address(value)
            if was_corrected:
                corrections.append(
                    f"call[{call_idx}].args[{arg_idx}]: {CorrectionType.ADDRESS_MISSING_0X_PREFIX.value}"
                )

        # Normalize shared_object nested structure
        elif key == "shared_object" and isinstance(value, dict):
            shared_corrections = []
            new_shared = {}
            for sk, sv in value.items():
                if sk == "id" and isinstance(sv, str):
                    sv, was_corrected = _normalize_address(sv)
                    if was_corrected:
                        shared_corrections.append(CorrectionType.ADDRESS_MISSING_0X_PREFIX.value)
                elif sk == "mutable":
                    sv, corr = _normalize_boolean(sv)
                    if corr:
                        shared_corrections.append(corr)
                new_shared[sk] = sv
            new_value = new_shared
            for corr in shared_corrections:
                corrections.append(f"call[{call_idx}].args[{arg_idx}].shared_object: {corr}")

        normalized[new_key] = new_value

    return normalized, corrections


def _normalize_call(call: dict[str, Any], call_idx: int) -> tuple[dict[str, Any], list[str]]:
    """
    Normalize a single call in a PTB spec.
    Returns (normalized_call, list_of_corrections).
    """
    if not isinstance(call, dict):
        return call, []

    corrections: list[str] = []
    normalized = dict(call)

    args = call.get("args")
    if isinstance(args, list):
        new_args = []
        for arg_idx, arg in enumerate(args):
            norm_arg, arg_corrections = _normalize_arg(arg, call_idx, arg_idx)
            new_args.append(norm_arg)
            corrections.extend(arg_corrections)
        normalized["args"] = new_args

    return normalized, corrections


def normalize_ptb_spec(ptb_spec: dict[str, Any]) -> NormalizationResult:
    """
    Normalize a PTB spec, fixing common LLM formatting mistakes.

    Normalizations applied:
    - "object" arg kind → "imm_or_owned_object"
    - "object_id" arg kind → "imm_or_owned_object"
    - String integers → integers (e.g., {"u64": "100"} → {"u64": 100})
    - String result refs → integers (e.g., {"result": "0"} → {"result": 0})
    - Addresses without 0x prefix → add prefix

    Args:
        ptb_spec: The PTB specification dict to normalize.

    Returns:
        NormalizationResult with normalized spec and list of corrections applied.
    """
    if not isinstance(ptb_spec, dict):
        return NormalizationResult(spec=ptb_spec)

    normalized = copy.deepcopy(ptb_spec)
    all_corrections: list[str] = []
    correction_counts: Counter = Counter()

    calls = normalized.get("calls")
    if isinstance(calls, list):
        new_calls = []
        for call_idx, call in enumerate(calls):
            norm_call, call_corrections = _normalize_call(call, call_idx)
            new_calls.append(norm_call)
            all_corrections.extend(call_corrections)
            # Count correction types
            for corr in call_corrections:
                # Extract the correction type from the string
                parts = corr.split(": ", 1)
                if len(parts) == 2:
                    correction_counts[parts[1]] += 1
        normalized["calls"] = new_calls

    # Convert unresolved placeholders into a deterministic dummy object id so downstream
    # tooling never fails on an unknown arg kind (useful for build-only scans).
    calls2 = normalized.get("calls")
    if isinstance(calls2, list):
        for call in calls2:
            if not isinstance(call, dict):
                continue
            args = call.get("args")
            if not isinstance(args, list):
                continue
            for i, arg in enumerate(args):
                if isinstance(arg, dict) and "$smi_placeholder" in arg:
                    args[i] = {"imm_or_owned_object": "0x0"}
                    all_corrections.append("placeholder: $smi_placeholder -> imm_or_owned_object(0x0)")
                    correction_counts["placeholder"] += 1

    return NormalizationResult(
        spec=normalized,
        corrections=all_corrections,
        correction_counts=correction_counts,
    )
