from __future__ import annotations

import argparse
import json
from pathlib import Path


def _fmt(v: object) -> str:
    if v is None:
        return "-"
    if isinstance(v, float):
        return f"{v:.4f}"
    return str(v)


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="Per-package Phase I analysis helper")
    p.add_argument("--run-json", type=Path, required=True)
    p.add_argument("--top", type=int, default=10)
    p.add_argument(
        "--sort",
        type=str,
        default="f1",
        choices=["f1", "precision", "recall", "tp", "fp", "fn", "elapsed", "truth", "pred"],
    )
    p.add_argument("--show", type=str, help="Show details for a specific package_id")
    args = p.parse_args(argv)

    data = json.loads(args.run_json.read_text())
    pkgs = data.get("packages", [])
    if not isinstance(pkgs, list):
        raise SystemExit("invalid run json: packages is not a list")

    if args.show:
        for r in pkgs:
            if isinstance(r, dict) and r.get("package_id") == args.show:
                print(json.dumps(r, indent=2, sort_keys=True))
                return
        raise SystemExit(f"package_id not found: {args.show}")

    rows = []
    for r in pkgs:
        if not isinstance(r, dict):
            continue
        score = r.get("score") or {}
        if not isinstance(score, dict):
            score = {}
        rows.append(
            {
                "package_id": r.get("package_id"),
                "truth": r.get("truth_key_types"),
                "pred": r.get("predicted_key_types"),
                "tp": score.get("tp"),
                "fp": score.get("fp"),
                "fn": score.get("fn"),
                "precision": score.get("precision"),
                "recall": score.get("recall"),
                "f1": score.get("f1"),
                "elapsed": r.get("elapsed_seconds"),
                "attempts": r.get("attempts"),
                "max_structs": r.get("max_structs_used"),
                "timed_out": r.get("timed_out"),
                "error": r.get("error"),
            }
        )

    def key(row: dict) -> float:
        k = args.sort
        if k in ("truth", "pred", "tp", "fp", "fn", "attempts", "max_structs"):
            try:
                return float(row.get(k) or 0)
            except Exception:
                return 0.0
        if k == "elapsed":
            try:
                return float(row.get("elapsed") or 0.0)
            except Exception:
                return 0.0
        try:
            return float(row.get(k) or 0.0)
        except Exception:
            return 0.0

    rows.sort(key=key, reverse=True)

    print(
        "package_id\tf1\tprecision\trecall\ttp\tfp\tfn\ttruth\tpred\telapsed_s\tattempts\tmax_structs\ttimed_out\terror"
    )
    for row in rows[: args.top]:
        print(
            "\t".join(
                [
                    _fmt(row.get("package_id")),
                    _fmt(row.get("f1")),
                    _fmt(row.get("precision")),
                    _fmt(row.get("recall")),
                    _fmt(row.get("tp")),
                    _fmt(row.get("fp")),
                    _fmt(row.get("fn")),
                    _fmt(row.get("truth")),
                    _fmt(row.get("pred")),
                    _fmt(row.get("elapsed")),
                    _fmt(row.get("attempts")),
                    _fmt(row.get("max_structs")),
                    _fmt(row.get("timed_out")),
                    _fmt(row.get("error")),
                ]
            )
        )


if __name__ == "__main__":
    main()

