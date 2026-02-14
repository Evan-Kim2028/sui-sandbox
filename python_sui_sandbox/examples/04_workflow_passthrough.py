#!/usr/bin/env python3
"""
04_workflow_passthrough.py

Builds a typed workflow spec in Python and executes it through the Rust CLI.
This demonstrates the "Python as thin pass-through" model for orchestration.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run a typed workflow spec via sui-sandbox from Python"
    )
    parser.add_argument(
        "--digest",
        default="At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
        help="Replay digest",
    )
    parser.add_argument(
        "--checkpoint",
        type=int,
        default=239615926,
        help="Checkpoint number used for walrus replay/analyze",
    )
    parser.add_argument(
        "--run",
        action="store_true",
        help="Execute workflow for real (default: dry-run)",
    )
    parser.add_argument(
        "--keep-spec",
        action="store_true",
        help="Keep generated workflow spec on disk",
    )
    parser.add_argument(
        "--sui-sandbox-bin",
        default="sui-sandbox",
        help="Path to sui-sandbox binary (default: sui-sandbox in PATH)",
    )
    return parser.parse_args()


def build_spec(digest: str, checkpoint: int) -> dict:
    return {
        "version": 1,
        "name": "python_passthrough_replay_analyze",
        "description": "Python emits typed workflow spec; Rust executes replay/analyze steps.",
        "defaults": {
            "source": "walrus",
            "allow_fallback": True,
            "auto_system_objects": True,
            "prefetch_depth": 3,
            "prefetch_limit": 200,
            "compare": True,
        },
        "steps": [
            {
                "id": "inspect_tx",
                "kind": "analyze_replay",
                "digest": digest,
                "checkpoint": checkpoint,
            },
            {
                "id": "replay_tx",
                "kind": "replay",
                "digest": digest,
                "checkpoint": str(checkpoint),
                "strict": True,
            },
            {"id": "status", "kind": "command", "args": ["status"]},
        ],
    }


def run_or_fail(argv: list[str]) -> None:
    completed = subprocess.run(argv, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def main() -> None:
    args = parse_args()
    if shutil.which(args.sui_sandbox_bin) is None and not Path(args.sui_sandbox_bin).exists():
        raise SystemExit(f"Could not find sui-sandbox binary: {args.sui_sandbox_bin}")

    spec = build_spec(args.digest, args.checkpoint)
    tmpdir = Path(tempfile.mkdtemp(prefix="sui-sandbox-python-workflow-"))
    spec_path = tmpdir / "workflow.json"
    report_path = tmpdir / "workflow_report.json"
    spec_path.write_text(json.dumps(spec, indent=2), encoding="utf-8")

    print(f"spec:   {spec_path}", flush=True)
    print(f"report: {report_path}", flush=True)
    print("mode:   execute" if args.run else "mode:   dry-run", flush=True)

    run_or_fail([args.sui_sandbox_bin, "workflow", "validate", "--spec", str(spec_path)])
    run_cmd = [
        args.sui_sandbox_bin,
        "workflow",
        "run",
        "--spec",
        str(spec_path),
        "--report",
        str(report_path),
    ]
    if not args.run:
        run_cmd.append("--dry-run")
    run_or_fail(run_cmd)

    if args.keep_spec:
        print(f"kept files in: {tmpdir}", flush=True)
    else:
        spec_path.unlink(missing_ok=True)
        report_path.unlink(missing_ok=True)
        tmpdir.rmdir()


if __name__ == "__main__":
    main()
