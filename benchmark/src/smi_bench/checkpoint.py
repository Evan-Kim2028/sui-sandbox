"""
Checkpoint serialization and persistence utilities.

Shared logic for atomic writes, checksum validation, and schema-agnostic loading
to avoid duplication between Phase 1 and Phase 2 runners.
"""

from __future__ import annotations

import json
from dataclasses import asdict, is_dataclass
from pathlib import Path
from typing import Any

from smi_bench.utils import atomic_write_text, compute_json_checksum, safe_read_json


def validate_checkpoint_compatibility(
    cp_data: dict[str, Any], expected: dict[str, Any], context: str = "checkpoint"
) -> None:
    """
    Validate that a loaded checkpoint is compatible with the current run configuration.

    Args:
        cp_data: Data loaded from checkpoint.
        expected: Expected configuration values (agent, seed, schema_version, etc.).
        context: Context for error messages.

    Raises:
        RuntimeError: If checkpoint is incompatible.
    """
    errors = []

    # Check schema version if present
    cp_schema = cp_data.get("schema_version")
    exp_schema = expected.get("schema_version")
    if exp_schema is not None and cp_schema != exp_schema:
        errors.append(f"schema_version mismatch: {cp_schema} vs {exp_schema}")

    # Check critical config fields
    critical_fields = ["agent", "seed", "simulation_mode"]
    for field in critical_fields:
        if field in expected:
            cp_val = cp_data.get(field)
            exp_val = expected.get(field)
            if cp_val != exp_val:
                errors.append(f"{field} mismatch: {cp_val} vs {exp_val}")

    if errors:
        raise RuntimeError(f"Incompatible {context}: {', '.join(errors)}")


def write_checkpoint(
    out_path: Path,
    data_obj: Any,
    validate_fn: Any = None,
) -> None:
    """
    Write checkpoint atomically with checksum validation.

    Uses a temporary file (.tmp suffix) and atomic replace to ensure checkpoint
    integrity. Validates schema (optional) and adds checksum.

    Args:
        out_path: Path to checkpoint file.
        data_obj: Dataclass instance or dict to serialize.
        validate_fn: Optional callable(dict) to validate schema before writing.

    Raises:
        ValueError: If schema validation fails.
        OSError: If file write fails.
    """
    if is_dataclass(data_obj):
        data = asdict(data_obj)
    elif isinstance(data_obj, dict):
        data = data_obj
    else:
        raise TypeError(f"Expected dataclass or dict, got {type(data_obj)}")

    if validate_fn is not None:
        validate_fn(data)

    # Add checksum for corruption detection
    checksum = compute_json_checksum(data)
    data["_checksum"] = checksum

    json_str = json.dumps(data, indent=2, sort_keys=True) + "\n"
    atomic_write_text(out_path, json_str)


def load_checkpoint(out_path: Path, context: str = "checkpoint") -> dict[str, Any]:
    """
    Load checkpoint with checksum validation.

    Reads checkpoint file, validates checksum if present.

    Args:
        out_path: Path to checkpoint file.
        context: Context string for error messages.

    Returns:
        Deserialized dictionary (with _checksum removed).

    Raises:
        FileNotFoundError: If checkpoint file doesn't exist.
        RuntimeError: If file read fails, JSON parse fails, or checksum mismatch.
    """
    if not out_path.exists():
        raise FileNotFoundError(
            f"{context} file not found: {out_path}\n"
            f"  Did you mean to run without --resume?\n"
            f"  Or check that the file path is correct."
        )

    try:
        data = safe_read_json(out_path, context=f"{context} file", raise_on_error=True)
    except (ValueError, FileNotFoundError) as e:
        raise RuntimeError(str(e)) from e

    if data is None:
        raise RuntimeError(
            f"Failed to load {context} from {out_path}. The file may be missing, inaccessible, or corrupted."
        )

    # Validate checksum if present
    stored_checksum = data.pop("_checksum", None)
    if stored_checksum:
        computed_checksum = compute_json_checksum(data)
        if stored_checksum != computed_checksum:
            raise RuntimeError(
                f"{context} checksum mismatch: {out_path}\n"
                f"  Stored: {stored_checksum}\n"
                f"  Computed: {computed_checksum}\n"
                f"  This indicates file corruption.\n"
                f"  Fix: Remove the file and restart without --resume."
            )

    return data
