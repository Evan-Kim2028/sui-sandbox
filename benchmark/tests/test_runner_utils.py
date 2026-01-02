from __future__ import annotations

from pathlib import Path

from smi_bench.runner import _load_ids_file_ordered


def test_load_ids_file_ordered_preserves_order_and_dedups(tmp_path: Path) -> None:
    p = tmp_path / "ids.txt"
    p.write_text(
        "\n".join(
            [
                "# comment",
                "",
                "0x2",
                "0x1",
                "0x2",
                "  0x3  ",
            ]
        )
        + "\n"
    )
    out = _load_ids_file_ordered(p)
    assert out == ["0x2", "0x1", "0x3"]
