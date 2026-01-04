from __future__ import annotations

import json
import os
import secrets
import socket
import sys
import time
from dataclasses import dataclass
from pathlib import Path


def _now_unix() -> int:
    return int(time.time())


def _safe_filename(s: str) -> str:
    out = []
    for ch in s:
        if ch.isalnum() or ch in ("-", "_", "."):
            out.append(ch)
        else:
            out.append("_")
    return "".join(out)[:120]


def default_run_id(*, prefix: str) -> str:
    """
    Generate a unique run ID using timestamp, PID, and a random suffix.
    The random suffix prevents collisions in high-concurrency Docker environments.
    """
    ts = time.strftime("%Y%m%d_%H%M%S", time.gmtime())
    pid = os.getpid()
    rand = secrets.token_hex(3)  # 6 chars
    return f"{prefix}_{ts}_pid{pid}_{rand}"


@dataclass(frozen=True)
class JsonlPaths:
    root: Path
    run_metadata: Path
    events: Path
    packages: Path


class JsonlLogger:
    """
    Simple benchmark logger:
    - run_metadata.json: one JSON object
    - events.jsonl: JSONL stream of status events
    - packages.jsonl: JSONL stream of per-package result rows (finished only)
    - .hostname: identity file for the container/host that created the logs
    """

    def __init__(self, *, base_dir: Path, run_id: str, use_stdout: bool = False) -> None:
        run_id = _safe_filename(run_id)
        root = base_dir / run_id
        root.mkdir(parents=True, exist_ok=True)
        self.paths = JsonlPaths(
            root=root,
            run_metadata=root / "run_metadata.json",
            events=root / "events.jsonl",
            packages=root / "packages.jsonl",
        )
        self.use_stdout = use_stdout

        # Write identity file to help debug shared volume collisions
        try:
            (root / ".hostname").write_text(socket.gethostname(), encoding="utf-8")
        except Exception:
            pass

    def write_run_metadata(self, obj: dict) -> None:
        self.paths.run_metadata.write_text(json.dumps(obj, indent=2, sort_keys=True) + "\n")

    def event(self, name: str, **fields: object) -> None:
        """
        Log an event with consistent schema.

        All events include:
        - `t`: Unix timestamp (seconds)
        - `event`: Event name (string)

        Additional fields are included as provided.
        """
        row = {"t": _now_unix(), "event": name, **fields}
        line = json.dumps(row, sort_keys=True) + "\n"
        with self.paths.events.open("a", encoding="utf-8") as f:
            f.write(line)

        if self.use_stdout:
            sys.stdout.write(f"A2A_EVENT:{line}")
            sys.stdout.flush()

    def package_row(self, row: dict) -> None:
        with self.paths.packages.open("a", encoding="utf-8") as f:
            f.write(json.dumps(row, sort_keys=True) + "\n")
