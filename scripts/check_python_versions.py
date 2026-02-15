#!/usr/bin/env python3
"""Ensure Rust and Python package versions stay in sync."""

from __future__ import annotations

import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ROOT_CARGO = ROOT / "Cargo.toml"
PY_CARGO = ROOT / "crates" / "sui-python" / "Cargo.toml"
PYPROJECT = ROOT / "crates" / "sui-python" / "pyproject.toml"


def load_toml(path: Path) -> dict:
    with path.open("rb") as f:
        return tomllib.load(f)


def main() -> int:
    root = load_toml(ROOT_CARGO)
    py_cargo = load_toml(PY_CARGO)
    pyproject = load_toml(PYPROJECT)

    versions = {
        "workspace_root": root["package"]["version"],
        "sui_python_cargo": py_cargo["package"]["version"],
        "sui_python_pyproject": pyproject["project"]["version"],
    }

    unique = set(versions.values())
    if len(unique) == 1:
        print(f"Version sync OK: {next(iter(unique))}")
        return 0

    print("Version mismatch detected:")
    for name, value in versions.items():
        print(f"  - {name}: {value}")
    print("Expected all versions to match.")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
