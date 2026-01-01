from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


@dataclass(frozen=True)
class PackageRef:
    package_id: str
    package_dir: str


def _fnv1a64(seed: int, s: str) -> int:
    h = 1469598103934665603 ^ seed
    for b in s.encode("utf-8"):
        h ^= b
        h = (h * 1099511628211) & 0xFFFFFFFFFFFFFFFF
    return h


def read_package_id_from_metadata(package_dir: Path) -> str | None:
    metadata_path = package_dir / "metadata.json"
    if not metadata_path.exists():
        return None
    try:
        data = json.loads(metadata_path.read_text())
    except Exception:
        return None
    package_id = data.get("id")
    return package_id if isinstance(package_id, str) and package_id else None


def iter_package_dirs(corpus_root: Path) -> Iterable[Path]:
    """
    Iterate package artifact directories under a corpus root.

    Expected layout matches MystenLabs/sui-packages:

      <corpus_root>/0x00/<pkgid> -> (dir or symlink to dir)

    Each package dir must contain `bytecode_modules/`.
    """
    for prefix in sorted(p for p in corpus_root.iterdir() if p.is_dir()):
        for entry in sorted(prefix.iterdir()):
            if not entry.exists():
                continue
            if not entry.is_dir():
                continue
            yield entry


def collect_packages(corpus_root: Path) -> list[PackageRef]:
    seen: set[str] = set()
    out: list[PackageRef] = []

    for package_dir in iter_package_dirs(corpus_root):
        resolved = package_dir.resolve()
        if not (resolved / "bytecode_modules").is_dir():
            continue
        package_id = read_package_id_from_metadata(resolved)
        if not package_id:
            continue
        if package_id in seen:
            continue
        seen.add(package_id)
        out.append(PackageRef(package_id=package_id, package_dir=str(resolved)))

    out.sort(key=lambda p: p.package_id)
    return out


def sample_packages(packages: list[PackageRef], n: int, seed: int) -> list[PackageRef]:
    if n <= 0 or n >= len(packages):
        return list(packages)

    scored = [(_fnv1a64(seed, p.package_id), p.package_id, p) for p in packages]
    scored.sort(key=lambda t: (t[0], t[1]))
    picked = [p for (_h, _id, p) in scored[:n]]
    picked.sort(key=lambda p: p.package_id)
    return picked
