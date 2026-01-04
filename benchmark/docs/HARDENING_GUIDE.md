# Hardening & Reliability Guide

This document defines the reliability patterns and best practices for the SMI Benchmark harness. All new contributions must adhere to these patterns to ensure stable, production-ready execution.

## Core Reliability Patterns

### 1. Atomic File Writes
**Why:** To prevent partial file corruption if the process crashes or the disk becomes full during a write operation.

**Pattern:** Always use `atomic_write_json` or `atomic_write_text`. These functions write to a `.tmp` file first and then perform an atomic rename.

```python
from smi_bench.utils import atomic_write_json

data = {"result": "success"}
# This is safe even if the process is killed midway
atomic_write_json(Path("results.json"), data)
```

### 2. Robust File Reading
**Why:** Centralized error handling for permissions, missing files, and malformed content.

**Pattern:** Use `safe_read_json` or `safe_read_text`. For line-based files (like manifests), use `safe_read_lines`.

```python
from smi_bench.utils import safe_read_json

# Returns None on failure instead of crashing, or raises clear error if raise_on_error=True
config = safe_read_json(Path("config.json"), context="main-config")
```

### 3. Managed Subprocesses
**Why:** To prevent "zombie" processes or orphaned child processes when a parent task is cancelled, times out, or crashes.

**Pattern:** Use the `managed_subprocess` async context manager. It ensures a SIGTERM -> SIGKILL sequence on exit.

```python
from smi_bench.utils import managed_subprocess

async with managed_subprocess("smi_tx_sim", "--args", ...) as proc:
    # If this block is cancelled or times out, 'proc' is guaranteed to be killed
    stdout, _ = await proc.communicate()
```

### 4. Exponential Backoff with Jitter
**Why:** To handle transient infrastructure failures like RPC rate limits, network blips, or filesystem locks.

**Pattern:** Use `retry_with_backoff` (sync) or `async_retry_with_backoff` (async).

```python
from smi_bench.utils import retry_with_backoff

# Retries 3 times with 2s base delay + random jitter
result = retry_with_backoff(
    lambda: call_external_api(),
    max_attempts=3,
    base_delay=2.0,
    retryable_exceptions=(RuntimeError, TimeoutError)
)
```

### 5. Defensive Input Parsing
**Why:** To prevent "poison pill" environment variables or CLI arguments from causing cryptic downstream failures.

**Pattern:** Use `safe_parse_int` and `safe_parse_float` with range clamping.

```python
from smi_bench.utils import safe_parse_float

# Clamps input between 0.0 and 2.0, uses 0.0 default if malformed
temp = safe_parse_float(os.environ.get("SMI_TEMP"), 0.0, min_val=0.0, max_val=2.0)
```

### 6. Checkpoint Integrity
**Why:** To ensure that resumed runs are consistent with original data and haven't been corrupted.

**Pattern:**
1. Checkpoints automatically include a `_checksum` field.
2. Always call `validate_checkpoint_compatibility` before resuming.

```python
from smi_bench.checkpoint import load_checkpoint, validate_checkpoint_compatibility

cp_data = load_checkpoint(out_path)
validate_checkpoint_compatibility(
    cp_data,
    {"agent": agent_name, "seed": seed, "schema_version": 2}
)
```

## Structured Telemetry

When logging errors, use `log_exception` to capture the traceback and structured context. This ensures production logs are machine-readable and contain enough info for rapid diagnosis.

```python
from smi_bench.utils import log_exception

try:
    process_package(pkg_id)
except Exception:
    log_exception("Package processing failed", extra={"package_id": pkg_id})
```

## Review Checklist for New Code

- [ ] Does it perform File I/O? -> Must use `atomic_write` / `safe_read`.
- [ ] Does it spawn a subprocess? -> Must use `managed_subprocess` or `retry_with_backoff`.
- [ ] Does it communicate with RPC/Network? -> Must use `retry_with_backoff`.
- [ ] Does it parse numeric inputs? -> Must use `safe_parse_*`.
- [ ] Does it log errors? -> Must use `log_exception`.
