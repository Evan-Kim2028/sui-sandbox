from __future__ import annotations

from pathlib import Path

from smi_bench.env import load_dotenv


def test_load_dotenv_parses_basic(tmp_path: Path) -> None:
    p = tmp_path / ".env"
    p.write_text(
        """
# comment
SMI_API_KEY=abc
SMI_MODEL="m"
EMPTY=
""".strip()
        + "\n"
    )
    env = load_dotenv(p)
    assert env["SMI_API_KEY"] == "abc"
    assert env["SMI_MODEL"] == "m"
    assert env["EMPTY"] == ""
