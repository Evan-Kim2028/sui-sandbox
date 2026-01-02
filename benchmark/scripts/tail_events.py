from __future__ import annotations

import argparse
import json
import time
from pathlib import Path


def _format_event(row: dict) -> str:
    t = row.get("t")
    ev = row.get("event")
    pkg = row.get("package_id")
    i = row.get("i")
    err = row.get("error")
    parts: list[str] = []
    if isinstance(t, int):
        parts.append(str(t))
    if isinstance(ev, str):
        parts.append(ev)
    if isinstance(i, int):
        parts.append(f"i={i}")
    if isinstance(pkg, str):
        parts.append(f"pkg={pkg}")
    if isinstance(err, str) and err:
        parts.append(f"error={err[:120]}")
    return " ".join(parts) if parts else json.dumps(row, sort_keys=True)


def main() -> None:
    p = argparse.ArgumentParser(description="Tail a benchmark events.jsonl file (best-effort pretty printing).")
    p.add_argument("events_jsonl", type=Path)
    p.add_argument("--follow", action="store_true", help="Follow the file for new events.")
    p.add_argument("--interval", type=float, default=0.25, help="Polling interval when --follow is set.")
    p.add_argument("--raw", action="store_true", help="Print raw JSON lines (no formatting).")
    p.add_argument("--event", type=str, help="Filter by event name (exact match).")
    p.add_argument("--package-id", type=str, help="Filter by package_id (exact match).")
    args = p.parse_args()

    path = args.events_jsonl
    if not path.exists():
        raise SystemExit(f"not found: {path}")

    with path.open("r", encoding="utf-8") as f:
        while True:
            line = f.readline()
            if not line:
                if not args.follow:
                    return
                time.sleep(args.interval)
                continue
            line = line.strip()
            if not line:
                continue
            if args.raw:
                print(line)
                continue
            try:
                row = json.loads(line)
            except Exception:
                print(line)
                continue
            if not isinstance(row, dict):
                continue
            if args.event and row.get("event") != args.event:
                continue
            if args.package_id and row.get("package_id") != args.package_id:
                continue
            print(_format_event(row))


if __name__ == "__main__":
    main()
