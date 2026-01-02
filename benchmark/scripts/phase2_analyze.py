from __future__ import annotations

import argparse
import json
from pathlib import Path


def _hit_rate(score: dict) -> float:
    try:
        targets = int(score.get("targets", 0))
        hits = int(score.get("created_hits", 0))
    except Exception:
        return 0.0
    if targets <= 0:
        return 0.0
    return hits / targets


def main() -> None:
    p = argparse.ArgumentParser(description="Analyze Phase II output JSON")
    p.add_argument("out_json", type=Path)
    p.add_argument("--show", type=str, help="Print the full row JSON for one package_id")
    args = p.parse_args()

    data = json.loads(args.out_json.read_text())
    rows = data.get("packages")
    if not isinstance(rows, list):
        raise SystemExit("invalid output: missing packages[]")

    if args.show:
        for r in rows:
            if isinstance(r, dict) and r.get("package_id") == args.show:
                print(json.dumps(r, indent=2, sort_keys=True))
                return
        raise SystemExit(f"package_id not found: {args.show}")

    # TSV header
    print(
        "\t".join(
            [
                "hit_rate",
                "targets",
                "hits",
                "missing",
                "created_distinct",
                "dry_run_ok",
                "fell_back",
                "sim_mode",
                "elapsed_s",
                "timed_out",
                "error",
                "package_id",
            ]
        )
    )

    enriched = []
    for r in rows:
        if not isinstance(r, dict):
            continue
        score = r.get("score") or {}
        if not isinstance(score, dict):
            score = {}
        hr = _hit_rate(score)
        enriched.append((hr, r))

    enriched.sort(key=lambda t: (-t[0], str(t[1].get("package_id", ""))))

    for hr, r in enriched:
        score = r.get("score") or {}
        targets = int(score.get("targets", 0)) if isinstance(score, dict) else 0
        hits = int(score.get("created_hits", 0)) if isinstance(score, dict) else 0
        missing = int(score.get("missing", 0)) if isinstance(score, dict) else 0
        created_distinct = int(score.get("created_distinct", 0)) if isinstance(score, dict) else 0
        dry_run_ok = r.get("dry_run_ok")
        fell_back = r.get("fell_back_to_dev_inspect")
        sim_mode = r.get("simulation_mode")
        elapsed = r.get("elapsed_seconds")
        timed_out = r.get("timed_out")
        err = r.get("error")
        pkg = r.get("package_id")
        print(
            "\t".join(
                [
                    f"{hr:.3f}",
                    str(targets),
                    str(hits),
                    str(missing),
                    str(created_distinct),
                    str(bool(dry_run_ok)) if dry_run_ok is not None else "",
                    str(bool(fell_back)) if fell_back is not None else "",
                    str(sim_mode) if isinstance(sim_mode, str) else "",
                    f"{float(elapsed):.3f}" if isinstance(elapsed, (int, float)) else "",
                    str(bool(timed_out)) if timed_out is not None else "",
                    (str(err)[:120].replace("\t", " ") if err else ""),
                    str(pkg),
                ]
            )
        )


if __name__ == "__main__":
    main()
