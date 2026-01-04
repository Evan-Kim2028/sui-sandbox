import argparse
import logging
import sys
from pathlib import Path

from smi_bench import inhabit_manifest as phase2_manifest
from smi_bench import inhabit_runner as phase2_runner
from smi_bench import runner as phase1
from smi_bench.constants import DEFAULT_RPC_URL

logger = logging.getLogger(__name__)


def run_all(args):
    # Setup directories
    out_dir = args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    if args.samples < 0:
        raise ValueError(f"samples must be >= 0, got {args.samples}")

    # 1. Phase I
    logger.info("--- Running Phase I (Key Struct Discovery) ---")
    p1_out = out_dir / "phase1.json"
    # Note: Phase I currently runs on a sample of the corpus.
    # We use mock-empty to just benchmark the infrastructure/dataset,
    # or the user can override agent if we exposed it.
    # For now, "run-all" assumes a baseline infrastructure check.
    phase1_argv = [
        "--corpus-root",
        str(args.corpus_root),
        "--samples",
        str(args.samples),
        "--out",
        str(p1_out),
        "--agent",
        "mock-empty",
        "--no-log",  # Keep noise down
    ]
    try:
        phase1.main(phase1_argv)
    except SystemExit as e:
        if e.code != 0:
            logger.error(f"Phase I failed with code {e.code}")
            sys.exit(e.code)

    # 2. Phase II Manifest
    logger.info("--- Running Phase II (Manifest Generation) ---")
    p2_ids = out_dir / "phase2_ids.txt"
    p2_plan = out_dir / "phase2_plan.json"
    p2_report = out_dir / "phase2_viability.json"

    manifest_argv = [
        "--corpus-root",
        str(args.corpus_root),
        "--out-ids",
        str(p2_ids),
        "--out-plan",
        str(p2_plan),
        "--out-report",
        str(p2_report),
    ]
    if args.samples > 0:
        manifest_argv.extend(["--limit", str(args.samples)])

    try:
        phase2_manifest.main(manifest_argv)
    except SystemExit as e:
        if e.code != 0:
            logger.error(f"Phase II Manifest failed with code {e.code}")
            sys.exit(e.code)

    # 3. Phase II Execution
    logger.info("--- Running Phase II (Execution) ---")
    p2_exec = out_dir / "phase2_execution.json"

    # Determine mode based on sender availability
    mode = "build-only"
    if args.sender and args.sender != "0x0":
        mode = "dry-run"

    runner_argv = [
        "--corpus-root",
        str(args.corpus_root),
        "--package-ids-file",
        str(p2_ids),
        # We don't limit samples here because we restricted manifest generation via --limit
        # But inhabit_runner treats --samples as a limit on the ids file too.
        # So we should pass a large number or the same number.
        "--samples",
        "1000000",
        "--agent",
        "baseline-search",
        "--baseline-max-candidates",
        "20",
        "--rpc-url",
        args.rpc_url,
        "--simulation-mode",
        mode,
        "--out",
        str(p2_exec),
        "--continue-on-error",
        "--max-errors",
        "1000",
        "--no-log",
    ]
    if mode == "dry-run":
        runner_argv.extend(["--sender", args.sender])
    else:
        # Build-only needs a dummy sender to parse args?
        # inhabit_runner.py defaults sender to None?
        # main() sets default to None.
        # But run() calls _resolve_sender... which defaults to 0x0.
        # So it's fine.
        pass

    try:
        phase2_runner.main(runner_argv)
    except SystemExit as e:
        if e.code != 0:
            logger.error(f"Phase II Execution failed with code {e.code}")
            sys.exit(e.code)

    logger.info(f"All done! Results in {out_dir}")


def main():
    parser = argparse.ArgumentParser(description="Unified AgentBeats Benchmark Runner")
    subparsers = parser.add_subparsers(dest="command", required=True)

    # run-all
    p_all = subparsers.add_parser("run-all", help="Run Phase I and Phase II sequentially")
    p_all.add_argument(
        "--corpus-root", type=Path, required=True, help="Path to sui-packages/packages/mainnet_most_used"
    )
    p_all.add_argument("--out-dir", type=Path, required=True, help="Directory to save all results")
    p_all.add_argument("--samples", type=int, default=100, help="Number of packages to process")
    p_all.add_argument("--rpc-url", type=str, default=DEFAULT_RPC_URL)
    p_all.add_argument("--sender", type=str, help="Funded wallet address for dry-run execution (optional)")

    args = parser.parse_args()
    if args.command == "run-all":
        run_all(args)


if __name__ == "__main__":
    main()
