from __future__ import annotations

import json
from pathlib import Path

from smi_bench.dataset import collect_packages, sample_packages


def _write_metadata(path: Path, package_id: str) -> None:
    (path / "metadata.json").write_text(json.dumps({"id": package_id}) + "\n")


def test_collect_packages_discovers_dirs(tmp_path: Path) -> None:
    corpus = tmp_path / "mainnet_most_used"
    (corpus / "0x00").mkdir(parents=True)

    pkg_dir = corpus / "0x00" / "pkgA"
    (pkg_dir / "bytecode_modules").mkdir(parents=True)
    _write_metadata(pkg_dir, "0xabc")

    pkgs = collect_packages(corpus)
    assert len(pkgs) == 1
    assert pkgs[0].package_id == "0xabc"


def test_sample_packages_is_deterministic(tmp_path: Path) -> None:
    corpus = tmp_path / "mainnet_most_used"
    (corpus / "0x00").mkdir(parents=True)

    ids = ["0x1", "0x2", "0x3", "0x4"]
    for i, pid in enumerate(ids):
        pkg_dir = corpus / "0x00" / f"pkg{i}"
        (pkg_dir / "bytecode_modules").mkdir(parents=True)
        _write_metadata(pkg_dir, pid)

    pkgs = collect_packages(corpus)
    s1 = sample_packages(pkgs, 2, 123)
    s2 = sample_packages(pkgs, 2, 123)
    assert [p.package_id for p in s1] == [p.package_id for p in s2]
