"""Shared utility functions for error handling, validation, and resource management."""

from __future__ import annotations

import hashlib
import json
import os
import stat
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any


class BinaryNotFoundError(FileNotFoundError):
    """Raised when a required binary is not found."""

    pass


class BinaryNotExecutableError(PermissionError):
    """Raised when a binary exists but is not executable."""

    pass


def validate_binary(path: Path, *, binary_name: str = "binary") -> Path:
    """
    Validate that a binary exists and is executable.

    Args:
        path: Path to the binary.
        binary_name: Human-readable name for error messages.

    Returns:
        The validated path.

    Raises:
        BinaryNotFoundError: If the binary doesn't exist.
        BinaryNotExecutableError: If the binary isn't executable.
    """
    if not path.exists():
        raise BinaryNotFoundError(f"{binary_name} not found: {path}\nRun: cargo build --release --locked")
    # Prefer stat-based checks so we can distinguish between directories/files.
    # Note: some tests mock `Path.exists()` without creating a real file. In that case,
    # `stat()` may fail; treat that as "cannot verify" and allow the caller to proceed.
    try:
        st = path.stat()
    except OSError:
        return path

    if not stat.S_ISREG(st.st_mode):
        raise BinaryNotFoundError(f"{binary_name} is not a regular file: {path}")
    if not os.access(path, os.X_OK):
        raise BinaryNotExecutableError(f"{binary_name} is not executable: {path}")
    return path


def safe_json_loads(text: str, *, context: str = "", max_snippet_len: int = 100) -> Any:
    """
    Parse JSON with better error messages and robust recovery from noisy strings.

    Heuristic: If direct parsing fails, it looks for the first/last bracket/brace
    spans to extract a JSON blob from surrounding prose or logs.
    """
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        # Robust heuristic: look for JSON object or array blocks
        # This handles cases where logs/warnings are mixed with JSON.
        s = text.strip()
        for opener, closer in (("{", "}"), ("[", "]")):
            start = s.find(opener)
            if start == -1:
                continue
            end = s.rfind(closer)
            if end == -1 or end <= start:
                continue
            candidate = s[start : end + 1]
            try:
                return json.loads(candidate)
            except json.JSONDecodeError:
                continue

    # If simple load and heuristics both failed, raise the original error (re-caught)
    try:
        return json.loads(text)
    except json.JSONDecodeError as e:
        # Extract snippet around error position
        start = max(0, e.pos - max_snippet_len // 2)
        end = min(len(text), e.pos + max_snippet_len // 2)
        snippet = text[start:end]
        if start > 0:
            snippet = "..." + snippet
        if end < len(text):
            snippet = snippet + "..."
        raise ValueError(
            f"JSON parse error{f' in {context}' if context else ''}: {e.msg}\nPosition {e.pos}, snippet: {snippet!r}"
        ) from e


def run_json_helper(
    cmd: list[str],
    *,
    timeout_s: float,
    context: str = "subprocess",
    cwd: Path | None = None,
) -> dict[str, Any]:
    """
    Run a subprocess and robustly parse its stdout as JSON.

    Handles mixed stdout (logs + JSON) by attempting to find the JSON blob.
    Standardizes error handling for timeouts and non-zero exit codes.

    Args:
        cmd: Command to execute.
        timeout_s: Timeout in seconds.
        context: Context name for error messages.
        cwd: Working directory (optional).

    Returns:
        Parsed JSON dictionary.

    Raises:
        TimeoutError: If process times out.
        RuntimeError: If process fails or output is not valid JSON.
    """
    try:
        out = subprocess.check_output(
            cmd,
            text=True,
            timeout=timeout_s,
            stderr=subprocess.PIPE,  # Capture stderr to avoid polluting parent output, or include in error
            cwd=str(cwd) if cwd else None,
        )
    except subprocess.TimeoutExpired as e:
        raise TimeoutError(f"{context} timed out after {timeout_s}s") from e
    except subprocess.CalledProcessError as e:
        stderr_snip = e.stderr[:500] if e.stderr else "N/A"
        raise RuntimeError(
            f"{context} failed (exit {e.returncode})\nCommand: {' '.join(cmd)}\nStderr: {stderr_snip}"
        ) from e

    try:
        data = safe_json_loads(out, context=context)
    except ValueError as e:
        raise RuntimeError(f"{context} returned invalid JSON: {e}\nOutput start: {out[:200]!r}") from e

    if not isinstance(data, dict):
        raise RuntimeError(f"{context} returned non-object JSON: {type(data).__name__}")

    return data


def compute_json_checksum(data: dict[str, Any]) -> str:
    """
    Compute a short checksum for JSON data (for corruption detection).

    Args:
        data: Dictionary to checksum.

    Returns:
        8-character hex checksum.
    """
    json_str = json.dumps(data, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(json_str.encode()).hexdigest()[:8]


def cleanup_old_temp_files(tmp_dir: Path, *, max_age_seconds: int = 86400) -> int:
    """
    Remove temporary files older than max_age_seconds.

    Args:
        tmp_dir: Directory containing temp files.
        max_age_seconds: Maximum age in seconds (default: 24 hours).

    Returns:
        Number of files removed.
    """
    if not tmp_dir.exists() or not tmp_dir.is_dir():
        return 0

    removed = 0
    now = time.time()
    for p in tmp_dir.glob("ptb_spec_*.json"):
        try:
            if now - p.stat().st_mtime > max_age_seconds:
                p.unlink()
                removed += 1
        except Exception:
            # Best-effort cleanup; ignore errors
            pass
    return removed


def ensure_temp_dir(tmp_dir: Path) -> Path:
    """
    Ensure a temp directory exists and clean up old files.

    Args:
        tmp_dir: Path to temp directory.

    Returns:
        The temp directory path.
    """
    tmp_dir.mkdir(parents=True, exist_ok=True)
    cleanup_old_temp_files(tmp_dir)
    return tmp_dir


def get_smi_temp_dir() -> Path:
    """
    Get the temporary directory for SMI benchmark artifacts.

    Respects SMI_TEMP_DIR environment variable, otherwise uses system temp.
    Ensures the directory exists and cleans up old files.
    """
    base = os.environ.get("SMI_TEMP_DIR")
    if base:
        p = Path(base)
    else:
        p = Path(tempfile.gettempdir()) / "smi_bench_tmp"
    return ensure_temp_dir(p)
