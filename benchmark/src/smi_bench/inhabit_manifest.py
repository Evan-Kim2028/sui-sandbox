from __future__ import annotations

import argparse
import time
from pathlib import Path

from rich.console import Console
from rich.progress import track

from smi_bench.dataset import collect_packages
from smi_bench.inhabit.executable_subset import (
    SelectStats,
    analyze_package,
    compute_package_viability,
)
from smi_bench.rust import default_rust_binary, emit_bytecode_json
from smi_bench.utils import atomic_write_json, atomic_write_text, extract_key_types_from_interface_json

console = Console()


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(
        description="Phase II: generate an executable-subset manifest (ids + PTB planfile) from a bytecode corpus"
    )
    p.add_argument("--corpus-root", type=Path, required=True)
    p.add_argument("--rust-bin", type=Path, default=default_rust_binary())
    p.add_argument("--out-ids", type=Path, required=True, help="Write selected package ids (one per line).")
    p.add_argument("--out-plan", type=Path, required=True, help="Write JSON mapping package_id -> PTB spec.")
    p.add_argument("--out-report", type=Path, help="Optional JSON report with summary counts and sample selections.")
    p.add_argument("--max-calls-per-package", type=int, default=1)
    p.add_argument("--limit", type=int, help="Optional cap on number of packages scanned.")
    args = p.parse_args(argv)

    if not args.rust_bin.exists():
        raise SystemExit(f"rust binary not found: {args.rust_bin} (run `cargo build --release --locked` at repo root)")

    packages = collect_packages(args.corpus_root)
    if args.limit is not None and args.limit > 0:
        packages = packages[: args.limit]

    started = int(time.time())
    stats = SelectStats(packages_total=len(packages))
    rejection_reasons_global: dict[str, int] = {}

    viability_packages_with_public_entry = 0
    viability_packages_with_public_entry_no_type_params = 0
    viability_packages_with_supported_entry = 0
    viability_public_entry_total = 0
    viability_public_entry_no_type_params_total = 0
    viability_public_entry_supported_total = 0
    viability_packages_with_key_targets = 0
    viability_selected_packages_with_key_targets = 0
    viability_key_targets_total = 0

    ids: list[str] = []
    plan: dict[str, dict] = {}
    samples: list[dict] = []
    rejected_samples: list[dict] = []

    for pkg in track(packages, description="select"):
        try:
            iface = emit_bytecode_json(package_dir=Path(pkg.package_dir), rust_bin=args.rust_bin)
        except Exception as e:
            stats = SelectStats(
                packages_total=stats.packages_total,
                packages_selected=stats.packages_selected,
                packages_failed_interface=stats.packages_failed_interface + 1,
                packages_no_candidates=stats.packages_no_candidates,
                candidate_functions_total=stats.candidate_functions_total,
                rejection_reasons_counts=stats.rejection_reasons_counts,
            )
            if len(samples) < 20:
                samples.append({"package_id": pkg.package_id, "error": str(e)})
            continue

        v = compute_package_viability(iface)
        if v.public_entry_total > 0:
            viability_packages_with_public_entry += 1
        if v.public_entry_no_type_params_total > 0:
            viability_packages_with_public_entry_no_type_params += 1
        if v.public_entry_no_type_params_supported_args_total > 0:
            viability_packages_with_supported_entry += 1
        viability_public_entry_total += v.public_entry_total
        viability_public_entry_no_type_params_total += v.public_entry_no_type_params_total
        viability_public_entry_supported_total += v.public_entry_no_type_params_supported_args_total

        key_targets = extract_key_types_from_interface_json(iface)
        if key_targets:
            viability_packages_with_key_targets += 1
            viability_key_targets_total += len(key_targets)

        analysis = analyze_package(iface)

        # Accumulate reasons
        for r, count in analysis.reasons_summary.items():
            rejection_reasons_global[r] = rejection_reasons_global.get(r, 0) + count

        candidates = analysis.candidates_ok
        if args.max_calls_per_package > 0:
            candidates = candidates[: args.max_calls_per_package]

        # For the plan file, we can only safely include one multi-step plan per package
        # because merging them requires re-indexing Result() references.
        # We pick the first one.
        ptb_spec = {"calls": candidates[0]} if candidates else None

        # Collect rejected sample for debug
        if not candidates and analysis.candidates_rejected and len(rejected_samples) < 50:
            rejected_samples.append(
                {"package_id": pkg.package_id, "rejected_targets": analysis.candidates_rejected[:5]}
            )

        stats = SelectStats(
            packages_total=stats.packages_total,
            packages_selected=stats.packages_selected + (1 if ptb_spec is not None else 0),
            packages_failed_interface=stats.packages_failed_interface,
            packages_no_candidates=stats.packages_no_candidates + (1 if ptb_spec is None else 0),
            candidate_functions_total=stats.candidate_functions_total + len(candidates),
            rejection_reasons_counts=rejection_reasons_global,
        )
        if ptb_spec is None:
            continue

        ids.append(pkg.package_id)
        plan[pkg.package_id] = ptb_spec
        if key_targets:
            viability_selected_packages_with_key_targets += 1
        if len(samples) < 20:
            samples.append({"package_id": pkg.package_id, "calls": candidates, "targets_key_types": len(key_targets)})

    atomic_write_text(args.out_ids, "\n".join(ids) + ("\n" if ids else ""))
    atomic_write_json(args.out_plan, plan)

    console.print(f"wrote: {args.out_ids} (n={len(ids)})")
    console.print(f"wrote: {args.out_plan} (n={len(plan)})")

    if args.out_report:
        finished = int(time.time())
        report = {
            "schema_version": 1,
            "started_at_unix_seconds": started,
            "finished_at_unix_seconds": finished,
            "corpus_root_name": args.corpus_root.name,
            "max_calls_per_package": args.max_calls_per_package,
            "stats": {
                "packages_total": stats.packages_total,
                "packages_selected": stats.packages_selected,
                "packages_failed_interface": stats.packages_failed_interface,
                "packages_no_candidates": stats.packages_no_candidates,
                "candidate_functions_total": stats.candidate_functions_total,
                "rejection_reasons_counts": stats.rejection_reasons_counts,
                "viability": {
                    "packages_with_public_entry": viability_packages_with_public_entry,
                    "packages_with_public_entry_no_type_params": viability_packages_with_public_entry_no_type_params,
                    "packages_with_supported_entry": viability_packages_with_supported_entry,
                    "packages_with_key_targets": viability_packages_with_key_targets,
                    "selected_packages_with_key_targets": viability_selected_packages_with_key_targets,
                    "key_targets_total": viability_key_targets_total,
                    "public_entry_total": viability_public_entry_total,
                    "public_entry_no_type_params_total": viability_public_entry_no_type_params_total,
                    "public_entry_supported_total": viability_public_entry_supported_total,
                },
            },
            "samples": samples,
            "rejected_samples": rejected_samples,
        }
        atomic_write_json(args.out_report, report)
        console.print(f"wrote: {args.out_report}")


if __name__ == "__main__":
    main()
