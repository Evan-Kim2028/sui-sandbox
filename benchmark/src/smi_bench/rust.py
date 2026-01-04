"""Shared utilities for invoking the Rust extractor binary."""

from __future__ import annotations

import logging
import os
import subprocess
from pathlib import Path

from smi_bench.utils import (
    BinaryNotExecutableError,
    BinaryNotFoundError,
    retry_with_backoff,
    safe_json_loads,
    validate_binary,
)

logger = logging.getLogger(__name__)

# Re-export for convenience
__all__ = [
    "default_rust_binary",
    "validate_rust_binary",
    "emit_bytecode_json",
    "build_rust",
    "BinaryNotFoundError",
    "BinaryNotExecutableError",
]


def build_rust() -> None:
    """
    Build the Rust extractor binary in release mode.

    Runs `cargo build --release --locked` from the repository root.
    """
    repo_root = Path(__file__).resolve().parents[3]
    subprocess.check_call(["cargo", "build", "--release", "--locked"], cwd=repo_root)


def default_rust_binary() -> Path:
    """
    Locate the Rust extractor binary.

    Checks (in order):
    1. `target/release/sui_move_interface_extractor` (or `.exe` on Windows) in repo root
    2. `/usr/local/bin/sui_move_interface_extractor`

    Returns:
        Path to the binary (may not exist; caller should check with validate_rust_binary()).
    """
    repo_root = Path(__file__).resolve().parents[3]
    exe = "sui_move_interface_extractor.exe" if os.name == "nt" else "sui_move_interface_extractor"
    local = repo_root / "target" / "release" / exe
    if local.exists():
        return local
    return Path("/usr/local/bin") / exe


def validate_rust_binary(path: Path) -> Path:
    """
    Validate that the Rust extractor binary exists and is executable.

    Args:
        path: Path to the binary.

    Returns:
        The validated path.

    Raises:
        BinaryNotFoundError: If the binary doesn't exist.
        BinaryNotExecutableError: If the binary isn't executable.
    """
    return validate_binary(path, binary_name="Rust extractor binary")


def emit_bytecode_json(*, package_dir: Path, rust_bin: Path, timeout_s: float = 60.0) -> dict:
    """
    Emit bytecode-derived interface JSON for a local bytecode package directory.

    This is the canonical source of truth for interface extraction. The benchmark harness
    treats the emitted JSON as a stable substrate for truth labeling and prompting.

    Args:
        package_dir: Path to a local bytecode package directory (must contain `bytecode_modules/`).
        rust_bin: Path to the Rust extractor binary (should be validated with validate_rust_binary()).
        timeout_s: Timeout in seconds for the subprocess call (default: 60s).

    Returns:
        Parsed JSON dict representing the package interface (see `BytecodePackageInterfaceJson`).

    Raises:
        TimeoutError: If the subprocess times out after retries.
        RuntimeError: If the Rust binary fails after retries.
        ValueError: If the output is not valid JSON (with context).
    """

    def _run() -> str:
        try:
            return subprocess.check_output(
                [
                    str(rust_bin),
                    "--bytecode-package-dir",
                    str(package_dir),
                    "--emit-bytecode-json",
                    "-",
                ],
                text=True,
                timeout=timeout_s,
                stderr=subprocess.PIPE,
            )
        except subprocess.TimeoutExpired as e:
            raise TimeoutError(f"Rust extractor timed out after {timeout_s}s for {package_dir}") from e
        except subprocess.CalledProcessError as e:
            stderr_snippet = e.stderr[:500] if e.stderr else "N/A"
            raise RuntimeError(
                f"Rust extractor failed (exit {e.returncode}) for {package_dir}\nStderr: {stderr_snippet}"
            ) from e

    # Apply retry logic to handle transient filesystem or binary issues
    out = retry_with_backoff(
        _run,
        max_attempts=3,
        base_delay=2.0,
        retryable_exceptions=(RuntimeError, TimeoutError),
    )

    return safe_json_loads(out, context=f"Rust extractor output for {package_dir}")
