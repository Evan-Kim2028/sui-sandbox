"""Documentation verification tests.

These tests act as a "Continuous Documentation" check by ensuring that the
commands mentioned in the README/Getting Started guides actually work
and produce expected help outputs.
"""

from __future__ import annotations

import subprocess

import pytest


@pytest.mark.parametrize(
    "cmd, expected_keyword",
    [
        (["uv", "run", "smi-inhabit", "--help"], "Phase II"),
        (["uv", "run", "smi-phase1", "--help"], "Key-struct"),
        (["uv", "run", "smi-a2a-smoke", "--help"], "Run local A2A smoke"),
        (["uv", "run", "smi-agentbeats-scenario", "--help"], "Run an AgentBeats scenario"),
    ],
)
def test_cli_help_commands_match_documentation(cmd: list[str], expected_keyword: str) -> None:
    """Invariant: Core CLI tools must be reachable and return 0 for --help."""
    res = subprocess.run(cmd, check=False, capture_output=True, text=True)
    assert res.returncode == 0
    assert expected_keyword.lower() in res.stdout.lower()


def test_dotenv_example_exists() -> None:
    """Invariant: The .env.example file must exist for newcomers to follow setup."""
    from pathlib import Path

    assert Path(".env.example").exists()


def test_manifests_directory_structure() -> None:
    """Invariant: Required manifest files for standard benchmarks must be present."""
    from pathlib import Path

    manifest_dir = Path("manifests")
    assert manifest_dir.is_dir()

    # Must have standard benchmark manifests
    assert (manifest_dir / "standard_phase2_no_framework.txt").exists()
