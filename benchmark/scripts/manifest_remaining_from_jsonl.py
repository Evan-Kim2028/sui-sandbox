from __future__ import annotations

import argparse
import json
from pathlib import Path


def _load_ids(path: Path) -> list[str]:
    out: list[str] = []
    for line in path.read_text().splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        out.append(s)
    return out


def _done_from_packages_jsonl(path: Path) -> set[str]:
    done: set[str] = set()
    for raw in path.read_text().splitlines():
        raw = raw.strip()
        if not raw:
            continue
        try:
            row = json.loads(raw)
        except Exception:
            continue
        if isinstance(row, dict):
            pkg = row.get("package_id")
            if isinstance(pkg, str) and pkg:
                done.add(pkg)
    return done


def main() -> None:
    p = argparse.ArgumentParser(
        description="Compute which manifest ids are not yet present in a JSONL packages log (packages.jsonl)."
    )
    p.add_argument("--manifest", type=Path, required=True)
    p.add_argument("--packages-jsonl", type=Path, required=True)
    p.add_argument("--write-remaining", type=Path, help="Optional path to write remaining ids (one per line)")
    p.add_argument(
        "--remaining-only",
        action="store_true",
        help="Print only the remaining count (as an integer) to stdout.",
    )
    args = p.parse_args()

    manifest_ids = _load_ids(args.manifest)
    done = _done_from_packages_jsonl(args.packages_jsonl)
    remaining = [i for i in manifest_ids if i not in done]

    if args.remaining_only:
        print(len(remaining))
    else:
        print(f"manifest_total={len(manifest_ids)} done={len(done)} remaining={len(remaining)}")

    if args.write_remaining:
        args.write_remaining.parent.mkdir(parents=True, exist_ok=True)
        args.write_remaining.write_text("\n".join(remaining) + "\n")
        print(f"wrote: {args.write_remaining}")


if __name__ == "__main__":
    main()
