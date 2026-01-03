"""
Checkpoint serialization and persistence utilities.

Shared logic for atomic writes, checksum validation, and schema-agnostic loading
to avoid duplication between Phase 1 and Phase 2 runners.
"""

from __future__ import annotations

import hashlib
import json
import logging
from dataclasses import asdict, is_dataclass
from pathlib import Path
from typing import Any, TypeVar

from smi_bench.utils import safe_json_loads

T = TypeVar("T")
logger = logging.getLogger(__name__)


def compute_json_checksum(data: dict[str, Any]) -> str:
    """
    Compute a short checksum for JSON data (for corruption detection).

    Args:
        data: Dictionary to checksum.

    Returns:
        8-character hex checksum.
    """
    json_str = json.dumps(data, sort_keys=True, separators=( ",", ":"))
    return hashlib.sha256(json_str.encode()).hexdigest()[:8]


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
    tmp = out_path.with_suffix(out_path.suffix + ".tmp")
    try:
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
        tmp.write_text(json_str)
        tmp.replace(out_path)
    except (OSError, TypeError, ValueError, OverflowError):
        # Clean up .tmp file on failure to prevent accumulation
        if tmp.exists():
            try:
                tmp.unlink()
            except OSError:
                pass  # Best-effort cleanup
        raise


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
    try:
        text = out_path.read_text()
    except FileNotFoundError as exc:
        raise FileNotFoundError(
            f"{context} file not found: {out_path}\n"
            f"  Did you mean to run without --resume?\n"
            f"  Or check that the file path is correct."
        ) from exc
    except (OSError, PermissionError) as exc:
        raise RuntimeError(
            f"Failed to read {context} file: {out_path}\n"
            f"  Error: {exc}\n"
            f"  Check file permissions and disk space."
        ) from exc

    try:
        data = safe_json_loads(text, context=f"{context} file {out_path}")
    except ValueError as e:
        raise RuntimeError(
            f"{context} JSON parse error: {out_path}\n"
            f"  {e}\n"
            f"  The file may be corrupted. Consider removing it and restarting."
        ) from e

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
