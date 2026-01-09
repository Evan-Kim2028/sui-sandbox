#!/usr/bin/env python3
"""
Inhabit a single local bytecode package:

- Emits canonical interface JSON from local bytecode (bytecode-first ground truth)
- Runs the Rust tx simulator (smi_tx_sim) on a PTB spec
- Computes hits=|targets ∩ created| with stable normalization rules
- Saves all artifacts deterministically for later evaluation (no temp-only outputs)

Outputs under --out-dir (default: out/inhabit_debug/<PACKAGE_ID>/):
  - iface.json: canonical interface JSON
  - ptb_spec.json: copy of PTB spec used for the run (if provided)
  - tx_sim.json: raw simulator output (dev-inspect/dry-run/build-only)
  - pt_bcs_base64.txt + pt.bcs (if simulator emits PT BCS in build-only mode)
  - summary.json: scorer-friendly summary (targets, created, hits)
  - run_metadata.json: argv, timestamps, rpc_url, mode, tool versions, git HEAD
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


def _run(cmd: list[str]) -> str:
    """Run a command and return stdout, raising on non-zero exit.

    Captures stderr for inclusion in exception details for debuggability.
    """
    p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if p.returncode != 0:
        raise RuntimeError(
            "command failed\n"
            f"cmd: {cmd}\n"
            f"exit: {p.returncode}\n"
            f"stdout:\n{p.stdout}\n"
            f"stderr:\n{p.stderr}\n"
        )
    return p.stdout


def _git_head(repo_root: Path) -> str | None:
    try:
        return (
            subprocess.run(
                ["git", "rev-parse", "HEAD"], cwd=str(repo_root), check=True, capture_output=True, text=True
            ).stdout.strip()
        )
    except Exception:
        return None


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=(
            "Generate interface JSON from local bytecode, run smi_tx_sim on a PTB spec, compute hits, and persist all artifacts."
        )
    )
    ap.add_argument("--package-id", required=True, help="Published package id (0x…) for type attribution")
    ap.add_argument("--bytecode-package-dir", type=Path, required=True, help="Path to Move build/<pkg>")
    ap.add_argument("--ptb-spec", type=Path, help="Path to PTB spec JSON. If omitted, prints targets and exits.")
    ap.add_argument("--sender", required=True, help="Sui address used as sender label / dry-run owner")
    ap.add_argument("--rpc-url", default="https://fullnode.mainnet.sui.io:443", help="RPC URL for smi_tx_sim")
    ap.add_argument("--mode", choices=["dry-run", "dev-inspect", "build-only"], default="build-only")
    ap.add_argument("--gas-budget", type=int, default=10_000_000)
    ap.add_argument("--gas-coin", default=None)
    ap.add_argument("--out-dir", type=Path, default=REPO_ROOT / "out" / "inhabit_debug")
    args = ap.parse_args(argv)

    # Ensure we can import benchmark helpers when running from repo root or elsewhere.
    sys.path.insert(0, str(REPO_ROOT / "benchmark" / "src"))

    # Create a stable run directory under out_dir/<package_id>
    out_root: Path = args.out_dir
    run_dir: Path = out_root / args.package_id
    run_dir.mkdir(parents=True, exist_ok=True)

    rust_bin = os.environ.get("SMI_RUST_BIN") or str(REPO_ROOT / "target" / "release" / "sui_move_interface_extractor")
    tx_sim_bin = os.environ.get("SMI_TX_SIM_BIN") or str(REPO_ROOT / "target" / "release" / "smi_tx_sim")

    # Tool versions (safe best-effort)
    try:
        rust_ver = _run([rust_bin, "--version"]).strip()
    except Exception:
        rust_ver = None
    try:
        sim_ver = _run([tx_sim_bin, "--version"]).strip()
    except Exception:
        sim_ver = None

    # File paths
    iface_json_path = run_dir / "iface.json"
    sim_out_path = run_dir / "tx_sim.json"
    ptb_out_path = run_dir / "ptb_spec.json"
    bcs_b64_path = run_dir / "pt_bcs_base64.txt"
    bcs_bin_path = run_dir / "pt.bcs"
    summary_path = run_dir / "summary.json"
    meta_path = run_dir / "run_metadata.json"

    # Extract canonical interface JSON from local bytecode
    _run(
        [
            rust_bin,
            "--package-id",
            args.package_id,
            "--bytecode-package-dir",
            str(args.bytecode_package_dir),
            "--emit-bytecode-json",
            str(iface_json_path),
        ]
    )

    from smi_bench.utils import extract_key_types_from_interface_json
    from smi_bench.inhabit.score import score_inhabitation

    iface = json.loads(iface_json_path.read_text(encoding="utf-8"))
    targets = extract_key_types_from_interface_json(iface)

    # If no PTB provided, emit targets and exit
    if args.ptb_spec is None:
        payload = {
            "package_id": args.package_id,
            "targets_key_types": sorted(targets),
            "iface_json": str(iface_json_path),
        }
        print(json.dumps(payload, indent=2, sort_keys=True))
        return 0

    # Snapshot PTB spec used in the run (ensures reproducibility even if caller edits original file)
    try:
        ptb_text = args.ptb_spec.read_text(encoding="utf-8")
        ptb_out_path.write_text(ptb_text, encoding="utf-8")
    except Exception:
        pass

    # Build tx-sim command
    sim_cmd = [
        tx_sim_bin,
        "--rpc-url",
        args.rpc_url,
        "--sender",
        args.sender,
        "--ptb-spec",
        str(args.ptb_spec),
        "--bytecode-package-dir",
        str(args.bytecode_package_dir),
        "--mode",
        args.mode,
    ]
    if args.mode == "dry-run":
        sim_cmd += ["--gas-budget", str(args.gas_budget)]
        if args.gas_coin:
            sim_cmd += ["--gas-coin", str(args.gas_coin)]

    started = datetime.now(timezone.utc).isoformat()
    raw = _run(sim_cmd)
    finished = datetime.now(timezone.utc).isoformat()

    sim = json.loads(raw)
    sim_out_path.write_text(json.dumps(sim, indent=2, sort_keys=True), encoding="utf-8")

    # Save PT BCS if present
    pt_b64 = sim.get("programmableTransactionBcsBase64")
    if isinstance(pt_b64, str) and pt_b64:
        bcs_b64_path.write_text(pt_b64 + "\n", encoding="utf-8")
        try:
            b = base64.b64decode(pt_b64)
            bcs_bin_path.write_bytes(b)
        except Exception:
            pass

    # Compute hits exactly like benchmark/src/smi_bench/inhabit/score.py
    created = set(sim.get("createdObjectTypes") or [])
    static_created = set(sim.get("staticCreatedObjectTypes") or [])
    created_all = created | static_created
    score = score_inhabitation(target_key_types=set(targets), created_object_types=created_all)

    # Best-effort transaction digest extraction (present for dry-run/dev-inspect)
    def _extract_digest(v: dict) -> str | None:
        # Try common nesting patterns
        try:
            eff = v.get("dryRun") or v.get("devInspect") or {}
            if isinstance(eff, dict):
                # Some shapes nest under effects
                if isinstance(eff.get("effects"), dict):
                    d = eff["effects"].get("transactionDigest")
                    if isinstance(d, str):
                        return d
                d = eff.get("transactionDigest")
                if isinstance(d, str):
                    return d
        except Exception:
            pass
        # Fallback: DFS search for first string-valued key named 'transactionDigest'
        stack = [v]
        while stack:
            cur = stack.pop()
            if isinstance(cur, dict):
                for k, val in cur.items():
                    if k == "transactionDigest" and isinstance(val, str):
                        return val
                    if isinstance(val, (dict, list)):
                        stack.append(val)
            elif isinstance(cur, list):
                for it in cur:
                    if isinstance(it, (dict, list)):
                        stack.append(it)
        return None

    tx_digest = _extract_digest(sim)
    # Use Suiscan mainnet explorer for links
    explorer_url = (
        f"https://suiscan.xyz/mainnet/tx/{tx_digest}" if tx_digest else None
    )

    summary = {
        "package_id": args.package_id,
        "mode_used": sim.get("modeUsed"),
        "targets": score.targets,
        "created_distinct": score.created_distinct,
        "created_hits": score.created_hits,
        "missing": score.missing,
        "created_object_types": sorted(created),
        "static_created_object_types": sorted(static_created),
        "iface_json": str(iface_json_path),
        "tx_sim_json": str(sim_out_path),
        "tx_digest": tx_digest,
        "tx_explorer_url": explorer_url,
        "ptb_spec_json": str(ptb_out_path) if ptb_out_path.exists() else None,
        "pt_bcs_base64": str(bcs_b64_path) if bcs_b64_path.exists() else None,
        "pt_bcs_bin": str(bcs_bin_path) if bcs_bin_path.exists() else None,
    }
    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True), encoding="utf-8")
    print(json.dumps(summary, indent=2, sort_keys=True))

    # Persist run metadata for reproducibility / attribution
    run_meta = {
        "argv": sys.argv[1:],
        "started": started,
        "finished": finished,
        "rpc_url": args.rpc_url,
        "mode": args.mode,
        "gas_budget": args.gas_budget,
        "gas_coin": args.gas_coin,
        "sender": args.sender,
        "package_id": args.package_id,
        "bytecode_package_dir": str(args.bytecode_package_dir),
        "tool_versions": {
            "sui_move_interface_extractor": rust_ver,
            "smi_tx_sim": sim_ver,
        },
        "git_head": _git_head(REPO_ROOT),
        "outputs": {
            "iface_json": str(iface_json_path),
            "sim_json": str(sim_out_path),
            "ptb_spec": str(ptb_out_path) if ptb_out_path.exists() else None,
            "summary_json": str(summary_path),
            "pt_bcs_base64": str(bcs_b64_path) if bcs_b64_path.exists() else None,
            "pt_bcs_bin": str(bcs_bin_path) if bcs_bin_path.exists() else None,
        },
    }
    meta_path.write_text(json.dumps(run_meta, indent=2, sort_keys=True), encoding="utf-8")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
