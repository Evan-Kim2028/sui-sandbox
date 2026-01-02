from __future__ import annotations

import argparse
import json
from pathlib import Path

from smi_bench.inhabit.metrics import compute_phase2_metrics


def main() -> None:
    p = argparse.ArgumentParser(description="Compare multiple Phase II run outputs (TSV leaderboard).")
    p.add_argument("out_json", nargs="+", type=Path, help="One or more Phase II output JSON files.")
    args = p.parse_args()

    header = [
        "run",
        "agent",
        "packages",
        "dry_run_ok_rate",
        "any_hit_rate",
        "macro_avg_hit_rate",
        "micro_hit_rate",
        "hits",
        "targets",
    ]
    print("\t".join(header))

    for path in args.out_json:
        data = json.loads(path.read_text())
        rows = data.get("packages")
        if not isinstance(rows, list):
            raise SystemExit(f"invalid out json (missing packages[]): {path}")
        aggregate = data.get("aggregate") if isinstance(data.get("aggregate"), dict) else {}
        m = compute_phase2_metrics(rows=[r for r in rows if isinstance(r, dict)], aggregate=aggregate)
        micro = (m.hits / m.targets) if m.targets else 0.0

        agent = data.get("agent")
        agent_s = str(agent) if isinstance(agent, str) else ""

        dry_run_ok_rate = (m.dry_run_ok / m.packages) if m.packages else 0.0
        any_hit_rate = (m.any_hit / m.packages) if m.packages else 0.0

        print(
            "\t".join(
                [
                    path.name,
                    agent_s,
                    str(m.packages),
                    f"{dry_run_ok_rate:.3f}",
                    f"{any_hit_rate:.3f}",
                    f"{m.macro_avg_hit_rate:.6f}",
                    f"{micro:.6f}",
                    str(m.hits),
                    str(m.targets),
                ]
            )
        )


if __name__ == "__main__":
    main()
