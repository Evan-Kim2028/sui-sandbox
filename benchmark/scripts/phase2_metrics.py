from __future__ import annotations

import argparse
import json
from pathlib import Path

from smi_bench.inhabit.metrics import compute_phase2_metrics


def main() -> None:
    p = argparse.ArgumentParser(description="Compute aggregate Phase II metrics for a run output JSON")
    p.add_argument("out_json", type=Path)
    args = p.parse_args()

    data = json.loads(args.out_json.read_text())
    rows = data.get("packages")
    if not isinstance(rows, list):
        raise SystemExit("invalid out json: missing packages[]")

    aggregate = data.get("aggregate") if isinstance(data.get("aggregate"), dict) else {}
    m = compute_phase2_metrics(rows=rows, aggregate=aggregate)
    micro = (m.hits / m.targets) if m.targets else 0.0

    print(f"run={args.out_json}")
    print(f"packages={m.packages}")
    print(f"dry_run_ok_rate={(m.dry_run_ok / m.packages) if m.packages else 0.0:.3f} ({m.dry_run_ok}/{m.packages})")
    print(f"any_hit_rate={(m.any_hit / m.packages) if m.packages else 0.0:.3f} ({m.any_hit}/{m.packages})")
    print(f"macro_avg_hit_rate={m.macro_avg_hit_rate:.6f}")
    print(f"micro_hit_rate={micro:.6f} (hits={m.hits} targets={m.targets})")
    print(f"avg_created_distinct={(m.created_distinct_sum / m.packages) if m.packages else 0.0:.3f}")


if __name__ == "__main__":
    main()
