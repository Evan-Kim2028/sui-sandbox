"""Shared utility functions for error handling, validation, and resource management."""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import random
import signal
import stat
import subprocess
import sys
import tempfile
import time
import traceback
from collections.abc import Callable
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, TypeVar

logger = logging.getLogger(__name__)

T = TypeVar("T")


def retry_with_backoff(
    fn: Callable[[], T],
    *,
    max_attempts: int = 3,
    base_delay: float = 1.0,
    max_delay: float = 30.0,
    retryable_exceptions: tuple[type[Exception], ...] = (Exception,),
) -> T:
    """
    Retry a synchronous function with exponential backoff and jitter.

    Args:
        fn: The function to retry.
        max_attempts: Maximum number of attempts (must be >= 1).
        base_delay: Initial delay in seconds.
        max_delay: Maximum delay in seconds.
        retryable_exceptions: Tuple of exception types that trigger a retry.

    Returns:
        The return value of fn.

    Raises:
        The last exception encountered if all attempts fail.
    """
    if max_attempts < 1:
        return fn()

    last_exc: Exception | None = None
    for attempt in range(max_attempts):
        try:
            return fn()
        except retryable_exceptions as e:
            last_exc = e
            if attempt == max_attempts - 1:
                break

            delay = min(max_delay, base_delay * (2**attempt) + random.uniform(0, 1))
            logger.warning(f"Retry {attempt + 1}/{max_attempts} after {delay:.1f}s (reason: {type(e).__name__}: {e})")
            time.sleep(delay)

    if last_exc is not None:
        raise last_exc
    return fn()  # Should not reach here if max_attempts >= 1


async def async_retry_with_backoff(
    fn: Callable[[], Any],
    *,
    max_attempts: int = 3,
    base_delay: float = 1.0,
    max_delay: float = 30.0,
    retryable_exceptions: tuple[type[Exception], ...] = (Exception,),
) -> Any:
    """
    Retry an awaitable function with exponential backoff and jitter.
    """
    if max_attempts < 1:
        return await fn()

    last_exc: Exception | None = None
    for attempt in range(max_attempts):
        try:
            return await fn()
        except retryable_exceptions as e:
            last_exc = e
            if attempt == max_attempts - 1:
                break

            delay = min(max_delay, base_delay * (2**attempt) + random.uniform(0, 1))
            logger.warning(
                f"Async retry {attempt + 1}/{max_attempts} after {delay:.1f}s (reason: {type(e).__name__}: {e})"
            )
            await asyncio.sleep(delay)

    if last_exc is not None:
        raise last_exc
    return await fn()


class BinaryNotFoundError(FileNotFoundError):
    """Raised when a required binary is not found."""

    pass


class BinaryNotExecutableError(PermissionError):
    """Raised when a binary exists but is not executable."""

    pass


def safe_read_lines(path: Path, context: str = "") -> list[str]:
    """
    Read lines from a file with comprehensive error handling.
    """
    text = safe_read_text(path, context=context)
    if text is None:
        return []
    return text.splitlines()


def safe_read_json(path: Path, context: str = "", raise_on_error: bool = False) -> Any | None:
    """
    Read and parse JSON from a file with comprehensive error handling.

    Args:
        path: Path to the JSON file.
        context: Context for error messages.
        raise_on_error: If True, re-raises errors instead of returning None.

    Returns:
        Parsed JSON data, or None if reading/parsing failed.
    """
    text = safe_read_text(path, context=context)
    if text is None:
        if raise_on_error:
            raise FileNotFoundError(f"File not found: {path} ({context})")
        return None
    try:
        return safe_json_loads(text, context=context)
    except ValueError as e:
        logger.error(f"Invalid JSON in {path} ({context}): {e}")
        if raise_on_error:
            raise
        return None


def safe_read_text(path: Path, context: str = "") -> str | None:
    """
    Read text from a file with comprehensive error handling.

    Args:
        path: Path to the file.
        context: Context for error messages.

    Returns:
        File content as string, or None if reading failed.
    """
    if not path.exists():
        logger.debug(f"File not found: {path} ({context})")
        return None

    try:
        return path.read_text(encoding="utf-8")
    except (OSError, PermissionError) as e:
        logger.error(f"Failed to read {path} ({context}): {e}")
        return None


def atomic_write_text(path: Path, content: str) -> None:
    """
    Write text to a file atomically using a temporary file and rename.

    Args:
        path: Destination path.
        content: Text content to write.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + f".tmp.{os.getpid()}")
    try:
        tmp.write_text(content, encoding="utf-8")
        tmp.replace(path)
    except (OSError, UnicodeEncodeError) as e:
        logger.error(f"Atomic write to {path} failed: {e}")
        if tmp.exists():
            try:
                tmp.unlink()
            except OSError:
                pass
        raise


def atomic_write_json(path: Path, data: Any) -> None:
    """
    Write data to a JSON file atomically.

    Args:
        path: Destination path.
        data: Data to serialize to JSON.
    """
    json_str = json.dumps(data, indent=2, sort_keys=True)
    atomic_write_text(path, json_str)


@asynccontextmanager
async def managed_subprocess(*args: Any, **kwargs: Any):
    """
    Async context manager for subprocesses ensuring cleanup on failure or exit.

    Args:
        *args: Arguments for asyncio.create_subprocess_exec.
        **kwargs: Keyword arguments for asyncio.create_subprocess_exec.

    Yields:
        The created process object.
    """
    proc = await asyncio.create_subprocess_exec(*args, **kwargs)
    try:
        yield proc
    finally:
        if proc.returncode is None:
            try:
                proc.terminate()
                try:
                    await asyncio.wait_for(proc.wait(), timeout=5.0)
                except TimeoutError:
                    proc.kill()
                    await proc.wait()
            except ProcessLookupError:
                pass
            except Exception as e:
                logger.warning(f"Error cleaning up subprocess: {e}")


def safe_parse_float(
    val: Any, default: float, min_val: float = -float("inf"), max_val: float = float("inf"), name: str = "value"
) -> float:
    """
    Safe float parsing with range validation.
    """
    try:
        f = float(val)
    except (ValueError, TypeError):
        logger.warning(f"Invalid {name}={val!r}, using default {default}")
        return default

    if f < min_val or f > max_val:
        logger.warning(f"{name}={f} out of range [{min_val}, {max_val}], clamping")
        return max(min_val, min(max_val, f))
    return f


def safe_parse_int(
    val: Any, default: int, min_val: int = -sys.maxsize, max_val: int = sys.maxsize, name: str = "value"
) -> int:
    """
    Safe integer parsing with range validation.
    """
    try:
        i = int(val)
    except (ValueError, TypeError):
        logger.warning(f"Invalid {name}={val!r}, using default {default}")
        return default

    if i < min_val or i > max_val:
        logger.warning(f"{name}={i} out of range [{min_val}, {max_val}], clamping")
        return max(min_val, min(max_val, i))
    return i


def validate_range(
    val: Any, min_val: float = -float("inf"), max_val: float = float("inf"), name: str = "value"
) -> float:
    """
    Strictly validate that a numeric value is within a range.
    Raises ValueError if invalid or out of range.
    """
    try:
        f = float(val)
    except (ValueError, TypeError):
        raise ValueError(f"Invalid {name}: must be a number, got {val!r}")

    if f < min_val or f > max_val:
        raise ValueError(f"{name} {f} out of range [{min_val}, {max_val}]")
    return f


def safe_bool(val: Any, default: bool) -> bool:
    """
    Parse a boolean value with common string aliases (true, 1, yes).
    """
    if val is None:
        return default
    if isinstance(val, bool):
        return val
    if isinstance(val, str):
        return val.lower() in ("true", "1", "yes")
    return bool(val)


def setup_signal_handlers(cleanup_fn: Any) -> None:
    """
    Setup signal handlers for SIGTERM and SIGINT to ensure cleanup.

    Args:
        cleanup_fn: Function to call on signal.
    """

    def _handler(sig, frame):
        logger.info(f"Received signal {sig}, cleaning up...")
        cleanup_fn()
        sys.exit(128 + sig)

    signal.signal(signal.SIGTERM, _handler)
    signal.signal(signal.SIGINT, _handler)


def log_exception(msg: str, extra: dict[str, Any] | None = None) -> None:
    """
    Log an exception with traceback and optional extra structured info.
    """
    err_info = {
        "error_type": getattr(sys.exc_info()[0], "__name__", "Unknown"),
        "error_message": str(sys.exc_info()[1]),
        "traceback": traceback.format_exc(),
    }
    if extra:
        err_info.update(extra)
    logger.error(f"{msg}: {err_info['error_message']}", extra={"structured_error": err_info})


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
        except (OSError, PermissionError):
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


def find_git_root(start: Path) -> Path | None:
    """
    Find the git repository root by walking up from start path.

    Args:
        start: Starting directory path.

    Returns:
        Path to .git directory's parent, or None if not found.
    """
    cur = start.resolve()
    while True:
        if (cur / ".git").exists():
            return cur
        if cur.parent == cur:
            return None
        cur = cur.parent


def extract_key_types_from_interface_json(interface_json: dict[str, Any]) -> set[str]:
    """
    Extract all struct types with 'key' ability from interface JSON.

    Args:
        interface_json: Parsed bytecode interface JSON (from Rust extractor).

    Returns:
        Set of canonical type strings (format: "0xADDR::module::Struct").
    """
    out: set[str] = set()
    modules = interface_json.get("modules")
    if not isinstance(modules, dict):
        return out

    for module_name, module_def in modules.items():
        if not isinstance(module_name, str) or not isinstance(module_def, dict):
            continue
        address = module_def.get("address")
        if not isinstance(address, str):
            continue
        structs = module_def.get("structs")
        if not isinstance(structs, dict):
            continue
        for struct_name, struct_def in structs.items():
            if not isinstance(struct_name, str) or not isinstance(struct_def, dict):
                continue
            abilities = struct_def.get("abilities")
            if not isinstance(abilities, list):
                continue
            if "key" in abilities:
                out.add(f"{address}::{module_name}::{struct_name}")
    return out
