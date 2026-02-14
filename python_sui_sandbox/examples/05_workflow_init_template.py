#!/usr/bin/env python3
"""
05_workflow_init_template.py

Python pass-through for Rust workflow template generation (`workflow init`).
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import tempfile
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate and run a built-in workflow template via sui-sandbox"
    )
    parser.add_argument(
        "--template",
        default="generic",
        choices=["generic", "cetus", "suilend", "scallop"],
        help="Built-in workflow template",
    )
    parser.add_argument(
        "--from-config",
        help="Workflow init config file (JSON/YAML). Overrides template/digest/checkpoint options.",
    )
    parser.add_argument(
        "--digest",
        default="At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
        help="Seed digest for replay/analyze steps",
    )
    parser.add_argument(
        "--checkpoint",
        type=int,
        default=239615926,
        help="Seed checkpoint for replay/analyze steps",
    )
    parser.add_argument(
        "--package-id",
        help="Optional package id to include analyze package step",
    )
    parser.add_argument(
        "--view-object",
        action="append",
        default=[],
        help="Optional object id to include view object step (repeatable)",
    )
    parser.add_argument(
        "--run",
        action="store_true",
        help="Execute workflow for real (default: dry-run)",
    )
    parser.add_argument(
        "--keep-files",
        action="store_true",
        help="Keep generated workflow/report files",
    )
    parser.add_argument(
        "--sui-sandbox-bin",
        default="sui-sandbox",
        help="Path to sui-sandbox binary",
    )
    return parser.parse_args()


def run_or_fail(argv: list[str]) -> None:
    completed = subprocess.run(argv, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def main() -> None:
    args = parse_args()
    if shutil.which(args.sui_sandbox_bin) is None and not Path(args.sui_sandbox_bin).exists():
        raise SystemExit(f"Could not find sui-sandbox binary: {args.sui_sandbox_bin}")

    tmpdir = Path(tempfile.mkdtemp(prefix="sui-sandbox-workflow-init-"))
    spec_path = tmpdir / f"workflow.{args.template}.json"
    report_path = tmpdir / "workflow_report.json"

    if args.from_config:
        print("template: from-config", flush=True)
        print(f"config:   {args.from_config}", flush=True)
    else:
        print(f"template: {args.template}", flush=True)
    print(f"spec:     {spec_path}", flush=True)
    print(f"report:   {report_path}", flush=True)
    print("mode:     execute" if args.run else "mode:     dry-run", flush=True)

    init_cmd = [
        args.sui_sandbox_bin,
        "workflow",
        "init",
        "--output",
        str(spec_path),
        "--force",
    ]
    if args.from_config:
        init_cmd.extend(["--from-config", args.from_config])
    else:
        init_cmd.extend(
            [
                "--template",
                args.template,
                "--digest",
                args.digest,
                "--checkpoint",
                str(args.checkpoint),
            ]
        )
        if args.package_id:
            init_cmd.extend(["--package-id", args.package_id])
        for object_id in args.view_object:
            init_cmd.extend(["--view-object", object_id])
    run_or_fail(init_cmd)

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

    if args.keep_files:
        print(f"kept files in: {tmpdir}", flush=True)
    else:
        spec_path.unlink(missing_ok=True)
        report_path.unlink(missing_ok=True)
        tmpdir.rmdir()


if __name__ == "__main__":
    main()
