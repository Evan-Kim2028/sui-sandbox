from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

from smi_bench.dataset import collect_packages


def main() -> None:
    p = argparse.ArgumentParser(description="Write deterministic first/last halves of mainnet_most_used ids")
    p.add_argument("--corpus-root", type=Path, required=True)
    p.add_argument(
        "--out-dir",
        type=Path,
        default=Path("results/manifests"),
        help="Output directory (default: results/manifests)",
    )
    p.add_argument("--first", type=int, default=500, help="Size of the first half (default: 500)")
    args = p.parse_args()

    packages = collect_packages(args.corpus_root)
    ids = [p.package_id for p in packages]
    if len(ids) < args.first:
        raise SystemExit(f"corpus too small: {len(ids)} < {args.first}")

    first_ids = ids[: args.first]
    last_ids = ids[args.first :]

    args.out_dir.mkdir(parents=True, exist_ok=True)
    first_path = args.out_dir / f"{args.corpus_root.name}_first{len(first_ids)}_ids.txt"
    last_path = args.out_dir / f"{args.corpus_root.name}_remaining{len(last_ids)}_ids.txt"
    meta_path = args.out_dir / f"{args.corpus_root.name}_halves_metadata.json"

    first_path.write_text("\n".join(first_ids) + "\n")
    last_path.write_text("\n".join(last_ids) + "\n")
    meta_path.write_text(
        json.dumps(
            {
                "corpus_root_name": args.corpus_root.name,
                "generated_at_unix_seconds": int(time.time()),
                "total": len(ids),
                "first": len(first_ids),
                "remaining": len(last_ids),
                "first_ids_file": first_path.name,
                "remaining_ids_file": last_path.name,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )

    print(f"wrote: {first_path}")
    print(f"wrote: {last_path}")
    print(f"wrote: {meta_path}")


if __name__ == "__main__":
    main()
