# Python Bindings Build Guide

Use this guide when PyPI publish is unavailable or you need to build/test Python bindings locally.

All commands below run from the repository root: `sui-sandbox/`.

## Prerequisites

- Rust toolchain (stable)
- Python 3.9-3.13 (matching supported wheels)
- `pip` + virtual environment support

If your default interpreter is Python 3.14, prefer creating a 3.13 virtual environment for local builds.

If you must build with Python 3.14, set:
`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`
when running build/test commands.

## 1. Local Development Install

```bash
python3 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip maturin

PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 \\
maturin develop --manifest-path crates/sui-python/Cargo.toml --release
```

Quick import check:

```bash
python - <<'PY'
import sui_sandbox
print("sui_sandbox version:", sui_sandbox.__version__)
print("has replay:", hasattr(sui_sandbox, "replay"))
print("has extract_interface:", hasattr(sui_sandbox, "extract_interface"))
PY
```

## 2. Build Wheel + Source Distribution

```bash
python -m pip install --upgrade maturin
rm -rf dist

# For Python 3.9-3.13:
maturin build --manifest-path crates/sui-python/Cargo.toml --release -o dist

# For Python 3.14, set ABI3 compatibility explicitly:
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 \\
maturin build --manifest-path crates/sui-python/Cargo.toml --release -o dist
maturin sdist --manifest-path crates/sui-python/Cargo.toml -o dist

ls -1 dist
```

Verify typing files are packaged:

```bash
WHEEL_PATH="$(ls -1 dist/sui_sandbox-*.whl | head -n1)"
python -m zipfile -l "$WHEEL_PATH" | rg "sui_sandbox\\.pyi|py\\.typed"
```

## 3. Run CI-Equivalent Python Smoke Checks Locally

This mirrors the `python-smoke` job in `.github/workflows/ci.yml`.

```bash
python -m pip install --force-reinstall dist/sui_sandbox-*.whl

python - <<'PY'
import json
import tempfile
from pathlib import Path
import sui_sandbox

assert hasattr(sui_sandbox, "import_state")
assert hasattr(sui_sandbox, "deserialize_transaction")
assert hasattr(sui_sandbox, "deserialize_package")
assert isinstance(sui_sandbox.__version__, str)

with tempfile.TemporaryDirectory() as td:
    td_path = Path(td)
    state_path = td_path / "state.json"
    cache_dir = td_path / "cache"
    state = {
        "transaction": {
            "digest": "dummy_digest",
            "sender": "0x1",
            "gas_budget": 1_000_000,
            "gas_price": 1_000,
            "commands": [],
            "inputs": [],
            "effects": None,
            "timestamp_ms": None,
            "checkpoint": None,
        },
        "objects": {},
        "packages": {},
        "protocol_version": 64,
        "epoch": 0,
        "reference_gas_price": None,
        "checkpoint": None,
    }
    state_path.write_text(json.dumps(state))

    import_result = sui_sandbox.import_state(
        state=str(state_path),
        cache_dir=str(cache_dir),
    )
    assert import_result["states_imported"] == 1

    replay = sui_sandbox.replay(
        digest="dummy_digest",
        source="local",
        cache_dir=str(cache_dir),
        analyze_only=True,
    )
    assert replay["digest"] == "dummy_digest"
PY
```

## 4. When Editing Binding APIs

When adding/removing/changing `#[pyfunction]` exports in `crates/sui-python/src/lib.rs`:

- Update `crates/sui-python/sui_sandbox.pyi`
- Update `crates/sui-python/README.md` API docs/examples
- Run:

```bash
cargo fmt
cargo test -p sui-state-fetcher
cargo test -p sui-package-extractor
```

Optional compatibility check (useful on Python 3.14 environments):

```bash
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo check -p sui-python
```

## 5. Running CI From GitHub UI

- Validation only (recommended): push your branch or open a PR to trigger the `ci` workflow.
- Multi-platform wheel builds: open `python-publish` workflow and run `workflow_dispatch`.

Note: `python-publish` includes a PyPI publish step. Wheel/sdist artifacts are still uploaded during build jobs even if PyPI publish fails.
