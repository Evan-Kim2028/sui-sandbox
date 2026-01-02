from __future__ import annotations

import json
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 2 or sys.argv[1] in ("-h", "--help"):
        print("Usage: python scripts/phase2_status.py <OUT_JSON>")
        raise SystemExit(0)

    path = Path(sys.argv[1])
    data = json.loads(path.read_text())
    samples = int(data.get("samples", 0))
    agg = data.get("aggregate") or {}
    errors = agg.get("errors")
    avg_hit_rate = agg.get("avg_hit_rate")

    last_pkg = None
    last_err = None
    pkgs = data.get("packages")
    if isinstance(pkgs, list) and pkgs:
        last = pkgs[-1]
        if isinstance(last, dict):
            last_pkg = last.get("package_id")
            last_err = last.get("error")

    print(f"samples={samples} errors={errors} avg_hit_rate={avg_hit_rate} last_pkg={last_pkg} last_error={last_err}")


if __name__ == "__main__":
    main()
