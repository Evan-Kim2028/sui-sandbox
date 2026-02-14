#!/usr/bin/env python3
"""
07_workflow_auto_from_package.py

Python pass-through for Rust `workflow auto` generation from package id.
Runs: auto -> validate -> run (--dry-run by default).
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import tempfile
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate + validate + dry-run an auto draft adapter workflow"
    )
    parser.add_argument(
        "--package-id",
        default="0x2",
        help="Package id for `workflow auto`",
    )
    parser.add_argument(
        "--template",
        choices=["generic", "cetus", "suilend", "scallop"],
        help="Optional template override",
    )
    parser.add_argument(
        "--digest",
        help="Optional seed digest (requires --checkpoint)",
    )
    parser.add_argument(
        "--checkpoint",
        type=int,
        help="Optional seed checkpoint (requires --digest)",
    )
    parser.add_argument(
        "--best-effort",
        action="store_true",
        help="Emit scaffold even when dependency closure validation fails",
    )
    parser.add_argument(
        "--format",
        default="json",
        choices=["json", "yaml"],
        help="Output format for generated spec",
    )
    parser.add_argument(
        "--output",
        help="Output path for generated spec (defaults to temp dir)",
    )
    parser.add_argument(
        "--run",
        action="store_true",
        help="Execute workflow for real (default: dry-run)",
    )
    parser.add_argument(
        "--keep-files",
        action="store_true",
        help="Keep generated workflow/report files when using temp output",
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

    if (args.digest is None) != (args.checkpoint is None):
        raise SystemExit("Provide both --digest and --checkpoint together")

    if shutil.which(args.sui_sandbox_bin) is None and not Path(args.sui_sandbox_bin).exists():
        raise SystemExit(f"Could not find sui-sandbox binary: {args.sui_sandbox_bin}")

    tmpdir = Path(tempfile.mkdtemp(prefix="sui-sandbox-workflow-auto-"))
    if args.output:
        spec_path = Path(args.output)
        spec_path.parent.mkdir(parents=True, exist_ok=True)
    else:
        spec_path = tmpdir / f"workflow.auto.{args.format}"
    report_path = spec_path.parent / "workflow.auto.report.json"

    print("=== Workflow Auto (Python) ===", flush=True)
    print(f"package_id: {args.package_id}", flush=True)
    print(f"template:   {args.template or 'inferred'}", flush=True)
    print(f"spec:       {spec_path}", flush=True)
    print(f"report:     {report_path}", flush=True)
    print("mode:       execute" if args.run else "mode:       dry-run", flush=True)

    auto_cmd = [
        args.sui_sandbox_bin,
        "workflow",
        "auto",
        "--package-id",
        args.package_id,
        "--format",
        args.format,
        "--output",
        str(spec_path),
        "--force",
    ]
    if args.template:
        auto_cmd.extend(["--template", args.template])
    if args.digest and args.checkpoint is not None:
        auto_cmd.extend(["--digest", args.digest, "--checkpoint", str(args.checkpoint)])
    if args.best_effort:
        auto_cmd.append("--best-effort")
    run_or_fail(auto_cmd)

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

    if args.output or args.keep_files:
        print(f"kept files in: {spec_path.parent}", flush=True)
    else:
        spec_path.unlink(missing_ok=True)
        report_path.unlink(missing_ok=True)
        tmpdir.rmdir()


if __name__ == "__main__":
    main()
