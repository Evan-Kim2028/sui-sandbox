#!/usr/bin/env python3
"""
run_full_experiment.py

Orchestrates the full wrapper-coin experiment end-to-end in a repeatable, organized way.

Responsibilities:
  - (Optional) Build the Move package with `sui move build`.
  - (Optional) Dry-run publish for sanity.
  - Run PTB plans via scripts/inhabit_single_package_local.py and persist artifacts
    under out/inhabit_debug/<PACKAGE_ID>/runs/<RUN_NAME>/
  - (Optional) Execute a real on-chain mint and save stdout + digest.
  - Emit out/inhabit_debug/<PACKAGE_ID>/index.json summarizing all runs.

This is intentionally light on blockchain parsing logic; it shells out to existing
tools and copies their outputs into per-run subfolders for long-term retention.

Usage (examples):
  python scripts/run_full_experiment.py \
    --package-id 0x... \
    --sender 0x... \
    --bytecode-package-dir packages/wrapper_coin/build/wrapper_coin \
    --mint-ptb packages/wrapper_coin/ptb_mint_wrap.json \
    --probe-ptb packages/wrapper_coin/ptb_probe_cap.json \
    --fresh-3of4-ptb packages/wrapper_coin/ptb_fresh_build_3of4.json \
    --fresh-4of4-ptb packages/wrapper_coin/ptb_fresh_build_4of4.json \
    --do-build --do-dry-run-publish

  # Execute a real mint
  python scripts/run_full_experiment.py \
    --package-id 0x... \
    --sender 0x... \
    --bytecode-package-dir packages/wrapper_coin/build/wrapper_coin \
    --exec-mint --treasury-cap-id 0x... --gas-coin 0x... --gas-budget 20000000

Outputs:
  out/inhabit_debug/<PACKAGE_ID>/
    iface.json, tx_sim.json, ptb_spec.json, summary.json (last-run)
    runs/<RUN_NAME>/ { summary.json, tx_sim.json, ptb_spec.json, run_metadata.json, pt.bcs?, pt_bcs_base64.txt? }
    index.json (aggregated summary across runs)
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
RUNNER = REPO_ROOT / "scripts" / "inhabit_single_package_local.py"


def _run(cmd: list[str], cwd: Path | None = None) -> str:
    p = subprocess.run(cmd, cwd=str(cwd) if cwd else None, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if p.returncode != 0:
        raise RuntimeError(
            "command failed\n"
            f"cmd: {cmd}\n"
            f"exit: {p.returncode}\n"
            f"stdout:\n{p.stdout}\n"
            f"stderr:\n{p.stderr}\n"
        )
    return p.stdout


def _copy_run_outputs(pkg_dir: Path, run_dir: Path) -> None:
    """Copy last-run artifacts into a per-run directory."""
    for name in [
        "iface.json",
        "tx_sim.json",
        "ptb_spec.json",
        "summary.json",
        "run_metadata.json",
        "pt_bcs_base64.txt",
        "pt.bcs",
    ]:
        src = pkg_dir / name
        if src.exists():
            dst = run_dir / name
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)


def _append_index(index_path: Path, entry: dict) -> None:
    idx: dict
    if index_path.exists():
        idx = json.loads(index_path.read_text(encoding="utf-8"))
    else:
        idx = {"runs": []}
    idx["runs"].append(entry)
    index_path.write_text(json.dumps(idx, indent=2, sort_keys=True), encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Orchestrate wrapper coin experiment runs and persist results")
    ap.add_argument("--package-id", required=True)
    ap.add_argument("--sender", required=True)
    ap.add_argument("--bytecode-package-dir", type=Path, required=True)

    ap.add_argument("--rpc-url", default="https://fullnode.mainnet.sui.io:443")
    ap.add_argument("--gas-budget", type=int, default=20_000_000)
    ap.add_argument("--gas-coin", default=None)

    # Runner specs
    ap.add_argument("--mint-ptb", type=Path, default=REPO_ROOT / "packages" / "wrapper_coin" / "ptb_mint_wrap.json")
    ap.add_argument("--probe-ptb", type=Path, default=REPO_ROOT / "packages" / "wrapper_coin" / "ptb_probe_cap.json")
    ap.add_argument("--fresh-2of4-ptb", type=Path, default=REPO_ROOT / "packages" / "wrapper_coin" / "ptb_fresh_build.json")
    ap.add_argument("--fresh-3of4-ptb", type=Path, default=REPO_ROOT / "packages" / "wrapper_coin" / "ptb_fresh_build_3of4.json")
    ap.add_argument("--fresh-4of4-ptb", type=Path, default=REPO_ROOT / "packages" / "wrapper_coin" / "ptb_fresh_build_4of4.json")

    # Controls
    ap.add_argument("--do-build", action="store_true")
    ap.add_argument("--do-dry-run-publish", action="store_true")
    ap.add_argument("--exec-mint", action="store_true")
    ap.add_argument("--treasury-cap-id", help="Required for --exec-mint")

    args = ap.parse_args(argv)

    pkg_root = REPO_ROOT / "out" / "inhabit_debug" / args.package_id
    runs_root = pkg_root / "runs"
    runs_root.mkdir(parents=True, exist_ok=True)
    index_path = pkg_root / "index.json"

    # Optional build (sui move build)
    if args.do_build:
        _run(["sui", "move", "build"], cwd=REPO_ROOT / "packages" / "wrapper_coin")

    # Optional dry-run publish
    if args.do_dry_run_publish:
        out = _run([
            "sui", "client", "publish", "packages/wrapper_coin",
            "--skip-fetch-latest-git-deps", "--gas-budget", "100000000", "--dry-run",
        ], cwd=REPO_ROOT)
        (runs_root / "publish_dry_run").mkdir(parents=True, exist_ok=True)
        (runs_root / "publish_dry_run" / "stdout.txt").write_text(out, encoding="utf-8")
        _append_index(index_path, {"name": "publish_dry_run", "mode": "dry_run", "stdout": str((runs_root/"publish_dry_run"/"stdout.txt").absolute())})

    # Helper to invoke the single runner and copy artifacts
    def do_run(run_name: str, ptb: Path, mode: str, extra_args: list[str] | None = None):
        cmd = [
            sys.executable, str(RUNNER),
            "--package-id", args.package_id,
            "--bytecode-package-dir", str(args.bytecode_package_dir),
            "--ptb-spec", str(ptb),
            "--sender", args.sender,
            "--mode", mode,
            "--rpc-url", args.rpc_url,
        ]
        if mode == "dry-run":
            cmd += ["--gas-budget", str(args.gas_budget)]
            if args.gas_coin:
                cmd += ["--gas-coin", args.gas_coin]
        if extra_args:
            cmd += extra_args
        out = _run(cmd)
        # Persist per-run artifacts
        run_dir = runs_root / run_name
        _copy_run_outputs(pkg_root, run_dir)
        # Add to index
        try:
            summary = json.loads((run_dir / "summary.json").read_text(encoding="utf-8"))
        except Exception:
            summary = {}
        _append_index(index_path, {"name": run_name, "mode": mode, "summary": summary, "dir": str(run_dir.absolute())})

    # Run default set
    if args.mint_ptb.exists():
        do_run("mint_dry_run", args.mint_ptb, "dry-run")
    if args.probe_ptb.exists():
        do_run("probe_build_only", args.probe_ptb, "build-only")
    if args.fresh_2of4_ptb.exists():
        do_run("fresh_2of4_build_only", args.fresh_2of4_ptb, "build-only")
    if args.fresh_3of4_ptb.exists():
        do_run("fresh_3of4_build_only", args.fresh_3of4_ptb, "build-only")
    if args.fresh_4of4_ptb.exists():
        do_run("fresh_4of4_build_only", args.fresh_4of4_ptb, "build-only")

    # Optional executed mint
    if args.exec_mint:
        if not args.treasury_cap_id:
            raise SystemExit("--treasury-cap-id is required when --exec-mint is set")
        # Derive type arg
        ty = f"{args.package_id}::wrapper_coin::WRAPPER_COIN"
        cmd = [
            "sui", "client", "call",
            "--package", "0x2",
            "--module", "coin",
            "--function", "mint_and_transfer",
            "--type-args", ty,
            "--args", args.treasury_cap_id, "1", args.sender,
            "--sender", args.sender,
            "--gas-budget", str(args.gas_budget),
        ]
        if args.gas_coin:
            cmd += ["--gas", args.gas_coin]
        out = _run(cmd, cwd=REPO_ROOT)
        run_dir = runs_root / "mint_exec"
        run_dir.mkdir(parents=True, exist_ok=True)
        (run_dir / "stdout.txt").write_text(out, encoding="utf-8")
        # Try to extract digest
        dig = None
        for line in out.splitlines():
            if "Transaction Digest:" in line:
                dig = line.split("Transaction Digest:")[-1].strip()
                break
        _append_index(index_path, {
            "name": "mint_exec",
            "mode": "execute",
            "stdout": str((run_dir/"stdout.txt").absolute()),
            "tx_digest": dig,
            "tx_explorer_url": (f"https://suiscan.xyz/mainnet/tx/{dig}" if dig else None),
        })

    # Auto-generate a compact results table in the experiment report (if present)
    try:
        report_path = REPO_ROOT / "results" / "reports" / "wrapper_coin_experiment.md"
        if report_path.exists():
            idx = json.loads(index_path.read_text(encoding="utf-8")) if index_path.exists() else {"runs": []}
            rows = []
            for e in idx.get("runs", []):
                name = e.get("name") or e.get("plan") or "run"
                mode = e.get("mode") or "-"
                # Human-friendly plan names
                plan = {
                    "publish_dry_run": "Publish WRAPPER_COIN (dry-run)",
                    "mint_exec": "Mint WRAPPER_COIN (executed)",
                    "mint_dry_run": "Mint WRAPPER_COIN (dry-run)",
                    "probe_build_only": "Probe WRAPPER_COIN (build-only)",
                    "fresh_2of4_build_only": "Fresh type (2/4) (build-only)",
                    "fresh_3of4_build_only": "Fresh type (3/4) (build-only)",
                    "fresh_4of4_build_only": "Fresh type (4/4) (build-only)",
                }.get(name, name)

                summ = e.get("summary") or {}
                created = (summ.get("created_object_types") or []) + (summ.get("static_created_object_types") or [])
                # Derive base labels
                kinds = []
                def add(lbl):
                    if lbl not in kinds:
                        kinds.append(lbl)
                for t in created:
                    s = str(t)
                    if "::coin::Coin<" in s:
                        add("Coin")
                    if "::coin::TreasuryCap<" in s:
                        add("TreasuryCap")
                    if "::coin_registry::Currency<" in s:
                        add("Currency")
                    if "::coin_registry::MetadataCap<" in s:
                        add("MetadataCap")
                kinds_str = ", ".join(kinds) if kinds else "—"
                hits = summ.get("created_hits")
                targets = summ.get("targets")
                hits_str = f"{hits}/{targets}" if hits is not None and targets is not None else "—"
                link = summ.get("tx_explorer_url") or e.get("tx_explorer_url") or "—"
                rows.append((plan, mode, kinds_str, hits_str, link))

            # Build markdown table
            header = "| Plan | Mode | Created types (base) | Hits | Link |\n|------|------|-----------------------|------|------|\n"
            table = header + "\n".join([f"| {a} | {b} | {c} | {d} | {e} |" for a,b,c,d,e in rows])
            # Insert after '## Results Table' and before next '## '
            txt = report_path.read_text(encoding="utf-8")
            anchor = "## Results Table"
            i = txt.find(anchor)
            if i != -1:
                j = txt.find("\n## ", i + len(anchor))
                if j == -1:
                    j = len(txt)
                new_txt = txt[: i + len(anchor)] + "\n\n" + table + "\n\n" + txt[j:]
                report_path.write_text(new_txt, encoding="utf-8")
    except Exception as ex:
        print(f"[warn] failed to auto-update report table: {ex}")

    print(f"\nIndex written: {index_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
