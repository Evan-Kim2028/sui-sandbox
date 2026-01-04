from __future__ import annotations

import argparse
import logging
from pathlib import Path

from smi_bench.utils import atomic_write_text, safe_read_json

logger = logging.getLogger(__name__)


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="Filter a Phase II run JSON into a manifest")
    p.add_argument("out_json", type=Path, help="Phase II output JSON (from smi-inhabit)")
    p.add_argument("--min-targets", type=int, default=2, help="Keep packages with score.targets >= N")
    p.add_argument("--out-manifest", type=Path, required=True, help="Output manifest file (one package id per line)")
    args = p.parse_args(argv)

    data = safe_read_json(args.out_json, context="Phase II results")
    if data is None:
        raise SystemExit(f"Could not read results from {args.out_json}")
    rows = data.get("packages")
    if not isinstance(rows, list):
        raise SystemExit("invalid out json: missing packages[]")

    out_ids: list[str] = []
    for row in rows:
        if not isinstance(row, dict):
            continue
        pkg = row.get("package_id")
        score = row.get("score")
        if not isinstance(pkg, str) or not pkg:
            continue
        if not isinstance(score, dict):
            continue
        targets = score.get("targets")
        try:
            targets_i = int(targets)
        except (ValueError, TypeError):
            continue
        if targets_i >= args.min_targets:
            out_ids.append(pkg)

    content = "\n".join(out_ids) + ("\n" if out_ids else "")
    atomic_write_text(args.out_manifest, content)
    logger.info(f"packages_kept={len(out_ids)}")


if __name__ == "__main__":
    main()
