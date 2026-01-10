#!/usr/bin/env python3
"""End-to-end runner: dataset pkg -> LLM helper Move pkg -> MM2 mapping -> local tx-sim.

Offline by default: set SMI_E2E_REAL_LLM=1 to call a real OpenAI-compatible provider
(OpenRouter supported via OPENROUTER_API_KEY/SMI_API_KEY).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
BENCH_ROOT = REPO_ROOT / "benchmark"
SCHEMAS_DIR = BENCH_ROOT / "src" / "smi_bench" / "schemas"


@dataclass(frozen=True)
class RunConfig:
    corpus_root: Path
    dataset: str
    samples: int
    seed: int
    model: str | None
    enable_dryrun: bool
    out_dir: Path


def _sha256_bytes(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def _sha256_file(p: Path) -> str:
    return _sha256_bytes(p.read_bytes())


def _atomic_write_text(p: Path, text: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_suffix(p.suffix + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    tmp.replace(p)


def _atomic_write_json(p: Path, obj: Any) -> None:
    _atomic_write_text(p, json.dumps(obj, indent=2, sort_keys=True) + "\n")


def _read_dataset_first_id(dataset: str) -> str:
    dataset_path = BENCH_ROOT / "manifests" / "datasets" / f"{dataset}.txt"
    if not dataset_path.exists():
        raise SystemExit(f"dataset not found: {dataset_path}")
    for raw in dataset_path.read_text(encoding="utf-8").splitlines():
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        if not s.startswith("0x"):
            raise SystemExit(f"invalid package id in dataset: {s}")
        return s
    raise SystemExit(f"dataset empty: {dataset_path}")


def _read_dataset_ids(dataset: str) -> list[str]:
    dataset_path = BENCH_ROOT / "manifests" / "datasets" / f"{dataset}.txt"
    if not dataset_path.exists():
        raise SystemExit(f"dataset not found: {dataset_path}")
    out: list[str] = []
    for raw in dataset_path.read_text(encoding="utf-8").splitlines():
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        if not s.startswith("0x"):
            raise SystemExit(f"invalid package id in dataset: {s}")
        out.append(s)
    return out


def _find_package_dir_in_corpus(*, corpus_root: Path, package_id: str) -> Path:
    # Corpus layout: <corpus_root>/0x??/<pkgid> or <corpus_root>/<pkgid>
    direct = corpus_root / package_id
    if direct.is_dir() and (direct / "bytecode_modules").is_dir():
        return direct.resolve()
    # Common sui-packages layout uses a 0x?? prefix directory containing leaf dirs named by hex without 0x.
    hex_part = package_id[2:] if package_id.startswith("0x") else package_id
    prefix_dir = corpus_root / f"0x{hex_part[:2]}"
    if prefix_dir.is_dir():
        # leaf dir drops the leading 2 bytes (they are the prefix dir name)
        cand = prefix_dir / hex_part[2:]
        if cand.is_dir() and (cand / "bytecode_modules").is_dir():
            return cand.resolve()
    for prefix in sorted(p for p in corpus_root.iterdir() if p.is_dir()):
        cand = prefix / package_id
        if cand.is_dir() and (cand / "bytecode_modules").is_dir():
            return cand.resolve()
    raise SystemExit(f"package_id not found in corpus_root: {package_id}")


def _safe_rel_sources_path(path_s: str) -> Path:
    p = Path(path_s)
    if p.is_absolute() or ".." in p.parts:
        raise ValueError(f"invalid path (must be relative, no ..): {path_s}")
    if p.parts[:1] != ("sources",):
        raise ValueError(f"invalid path (must be under sources/): {path_s}")
    return p


def _validate_llm_helper_payload(obj: Any) -> dict[str, Any]:
    # JSON Schema gate first.
    try:
        import jsonschema

        schema_path = SCHEMAS_DIR / "helper_pkg_v1.schema.json"
        schema = json.loads(schema_path.read_text(encoding="utf-8"))
        jsonschema.validate(instance=obj, schema=schema)
    except Exception as e:
        raise ValueError(f"helper_pkg_v1 schema validation failed: {e}") from e

    if not isinstance(obj, dict):
        raise ValueError("LLM response must be a JSON object")
    move_toml = obj.get("move_toml")
    files = obj.get("files")
    entrypoints = obj.get("entrypoints", [])
    assumptions = obj.get("assumptions", [])

    if not isinstance(move_toml, str) or not move_toml.strip():
        raise ValueError("missing/invalid 'move_toml'")
    if not isinstance(files, dict) or not files:
        raise ValueError("missing/invalid 'files'")

    normalized_files: dict[str, str] = {}
    total_bytes = 0
    for k, v in files.items():
        if not isinstance(k, str) or not isinstance(v, str):
            raise ValueError("files keys/values must be strings")
        rel = _safe_rel_sources_path(k)
        if rel.suffix != ".move":
            raise ValueError(f"file must end with .move: {k}")
        total_bytes += len(v.encode("utf-8"))
        normalized_files[str(rel)] = v
    if total_bytes > 600_000:
        raise ValueError("helper package too large")

    if not isinstance(entrypoints, list):
        raise ValueError("invalid 'entrypoints' (must be list)")
    for ep in entrypoints:
        if not isinstance(ep, dict):
            raise ValueError("invalid entrypoint (must be object)")
        tgt = ep.get("target")
        if not isinstance(tgt, str) or tgt.count("::") != 2:
            raise ValueError("invalid entrypoint.target (must be 'addr::module::func')")
    if not isinstance(assumptions, list) or not all(isinstance(x, str) for x in assumptions):
        raise ValueError("invalid 'assumptions' (must be string list)")

    return {
        "move_toml": move_toml,
        "files": normalized_files,
        "entrypoints": entrypoints,
        "assumptions": assumptions,
    }


def _enforce_entrypoints_target_requested(*, helper_payload: dict[str, Any], requested_targets: list[str]) -> None:
    if not requested_targets:
        return
    eps = helper_payload.get("entrypoints")
    if not isinstance(eps, list) or not eps:
        raise ValueError("missing/invalid helper entrypoints")
    for ep in eps:
        if not isinstance(ep, dict):
            continue
        tgt = ep.get("target")
        if isinstance(tgt, str) and tgt in set(requested_targets):
            return
    raise ValueError("no helper entrypoint matches requested_targets")


def _extract_simple_entry_targets_from_interface(*, iface: dict[str, Any], limit: int = 5) -> list[str]:
    # Best-effort extraction of entry function targets from bytecode interface JSON.
    out: list[str] = []
    modules = iface.get("modules")
    if not isinstance(modules, dict):
        return out
    for mod_name, mod in modules.items():
        if not isinstance(mod_name, str) or not isinstance(mod, dict):
            continue
        addr = mod.get("address")
        if not isinstance(addr, str):
            continue
        fns = mod.get("functions")
        if not isinstance(fns, dict):
            continue
        for fn_name, fn in fns.items():
            if not isinstance(fn_name, str) or not isinstance(fn, dict):
                continue
            if fn.get("is_entry") is not True:
                continue
            params = fn.get("parameters")
            if isinstance(params, list) and len(params) == 0:
                out.append(f"{addr}::{mod_name}::{fn_name}")
            if len(out) >= limit:
                return out
    return out


def _extract_module_names_from_interface(iface: dict[str, Any]) -> set[str]:
    modules = iface.get("module_names")
    if isinstance(modules, list) and all(isinstance(x, str) for x in modules):
        return set(modules)
    modules = iface.get("modules")
    if not isinstance(modules, dict):
        return set()
    return set(k for k in modules.keys() if isinstance(k, str))




def _scan_illegal_address_uses_for_framework_only(*, move_sources: dict[str, str]) -> list[str]:
    # Intentionally permissive: only reject `use 0x...::...` when the address is NOT a core/framework address.
    # This keeps the model grounded on-chain while still allowing experimentation with target deps.
    allowed_addrs = {"0x1", "0x2", "0x3"}
    illegal: list[str] = []
    for path, content in move_sources.items():
        if not isinstance(content, str):
            continue
        for i, line in enumerate(content.splitlines(), start=1):
            s = line.strip()
            if not s.startswith("use 0x"):
                continue
            try:
                after_use = s[len("use ") :]
                addr_part, _rest = after_use.split("::", 1)
            except ValueError:
                continue
            addr = addr_part.strip()
            if addr not in allowed_addrs:
                illegal.append(f"{path}:{i}: non-framework address in use statement: {addr}")
    return illegal


def _scan_move_2024_only_syntax(*, move_sources: dict[str, str]) -> list[str]:
    # Fail-fast on common 2024-only syntax that breaks legacy builds.
    # Keep this intentionally shallow and pattern-based.
    patterns = [
        "public struct ",
        "public(friend) ",
        "public(package) ",
        "public(script) ",
    ]
    illegal: list[str] = []
    for path, content in move_sources.items():
        if not isinstance(content, str):
            continue
        for i, line in enumerate(content.splitlines(), start=1):
            s = line.strip()
            for p in patterns:
                if p in s:
                    illegal.append(f"{path}:{i}: disallowed Move 2024 syntax: {p.strip()}")
                    break
    return illegal


def _lint_model_move_sources(
    *,
    move_sources: dict[str, str],
    allowed_target_modules: set[str],
    allowed_target_modules_to_addr: dict[str, str],
) -> list[str]:
    errors: list[str] = []
    # Only constrain explicit `use 0x...::...` imports to framework/core addrs.
    # Leave target references unconstrained at this stage.
    errors.extend(_scan_illegal_address_uses_for_framework_only(move_sources=move_sources))
    errors.extend(_scan_move_2024_only_syntax(move_sources=move_sources))
    return errors


def _run(cmd: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None, timeout_s: int = 300) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd,
        cwd=str(cwd) if cwd else None,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout_s,
        check=False,
    )


def _benchmark_local_cli_prefix() -> list[str]:
    # Prefer installed binary (Docker image), otherwise use `cargo run` for local dev.
    if shutil.which("sui_move_interface_extractor") is not None:
        return ["sui_move_interface_extractor"]
    return ["cargo", "run", "--quiet", "--bin", "sui_move_interface_extractor", "--"]


def _persist_tmp_tree(*, run_dir: Path, tmp_root: Path | None) -> None:
    if tmp_root is None:
        return
    try:
        dest = run_dir / "persisted_tmp"
        dest.mkdir(parents=True, exist_ok=True)
        if not tmp_root.exists():
            return
        for p in sorted(tmp_root.rglob("*")):
            if p.is_dir():
                continue
            rel = p.relative_to(tmp_root)
            out = dest / rel
            out.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(p, out)
    except Exception:
        return


def _call_real_llm_openai_compatible(*, model: str | None, prompt: str, seed: int) -> dict[str, Any]:
    # Reuse the project’s OpenAI-compatible agent implementation.
    from smi_bench.agents.real_agent import RealAgent, RealAgentConfig

    # Minimal deterministic-ish defaults; keep them explicit.
    api_key = os.environ.get("SMI_API_KEY") or os.environ.get("OPENROUTER_API_KEY") or os.environ.get("OPENAI_API_KEY") or ""
    if not api_key.strip():
        raise RuntimeError("missing API key (set SMI_API_KEY or OPENROUTER_API_KEY or OPENAI_API_KEY)")

    cfg = RealAgentConfig(
        provider="openai_compatible",
        model=model or os.environ.get("SMI_MODEL") or "openai/gpt-5.2",
        base_url=os.environ.get("SMI_API_BASE_URL") or os.environ.get("OPENROUTER_BASE_URL") or "https://openrouter.ai/api/v1",
        api_key=api_key,
        temperature=float(os.environ.get("SMI_TEMPERATURE") or "0"),
        max_tokens=int(os.environ.get("SMI_MAX_TOKENS") or "4096"),
        thinking=os.environ.get("SMI_THINKING"),
        response_format=os.environ.get("SMI_RESPONSE_FORMAT"),
        clear_thinking=None,
        min_request_timeout_s=float(os.environ.get("SMI_TIMEOUT_SECONDS") or "60"),
        max_request_retries=int(os.environ.get("SMI_MAX_REQUEST_RETRIES") or "2"),
    )
    agent = RealAgent(cfg)
    return agent.complete_json(prompt)


def _call_real_llm_with_repair(*, model: str | None, prompt_obj: dict[str, Any], seed: int) -> dict[str, Any]:
    prompt_txt = json.dumps(prompt_obj, sort_keys=True)
    try:
        return _call_real_llm_openai_compatible(model=model, prompt=prompt_txt, seed=seed)
    except Exception as e:
        repair = (
            "Your previous response was not valid JSON. "
            "Return ONLY a single valid JSON object matching schema helper_pkg_v1. "
            "Ensure move_toml is a STRING and all Move source contents are properly escaped.\n\n"
            f"Error: {type(e).__name__}: {e}"
        )
        repaired = dict(prompt_obj)
        repaired["repair"] = repair
        return _call_real_llm_openai_compatible(model=model, prompt=json.dumps(repaired, sort_keys=True), seed=seed)


def _call_real_llm_with_lint_retries(
    *,
    model: str | None,
    prompt_obj: dict[str, Any],
    seed: int,
    max_attempts: int,
    lint_errors: list[str],
) -> dict[str, Any]:
    # Try to get valid JSON first, then iterate with concrete lint errors.
    last_err = ""
    obj = prompt_obj
    for attempt in range(1, max_attempts + 1):
        try:
            raw = _call_real_llm_with_repair(model=model, prompt_obj=obj, seed=seed + attempt)
            if attempt == 1:
                return raw
            return raw
        except Exception as e:
            last_err = f"{type(e).__name__}: {e}"
        # If we failed at call/parse level, just keep trying.
    raise RuntimeError(f"LLM call failed after {max_attempts} attempts: {last_err}")


def _coerce_model_output_to_helper_payload(raw: Any) -> Any:
    # Some models echo the request schema field back; strip it for strict validation.
    if isinstance(raw, dict) and "schema" in raw and isinstance(raw.get("move_toml"), str):
        out = dict(raw)
        out.pop("schema", None)
        return out
    return raw


def _stub_llm_helper_payload() -> dict[str, Any]:
    # Minimal helper package fixture that should build and expose an entry function.
    # NOTE: This is intentionally generic; MM2 mapping/tx-sim wiring can evolve.
    return {
        "move_toml": """[package]\nname = \"helper_pkg\"\nversion = \"0.0.1\"\n\n[addresses]\nhelper_pkg = \"0x0\"\n""",
        "files": {
            "sources/helper.move": """module helper_pkg::helper {\n  public entry fun noop() { }\n}\n""",
        },
        "entrypoints": [{"target": "helper_pkg::helper::noop", "args": []}],
        "assumptions": ["This is a stub helper package."],
    }


def _write_helper_package(*, helper_dir: Path, payload: dict[str, Any]) -> None:
    if helper_dir.exists():
        shutil.rmtree(helper_dir)
    (helper_dir / "sources").mkdir(parents=True, exist_ok=True)
    _atomic_write_text(helper_dir / "Move.toml", payload["move_toml"])
    for rel_s, content in payload["files"].items():
        rel = Path(rel_s)
        out = helper_dir / rel
        out.parent.mkdir(parents=True, exist_ok=True)
        _atomic_write_text(out, content)


def _vendor_target_deps_into_helper(*, target_pkg_dir: Path, helper_dir: Path) -> None:
    # Allow the helper package to `use`/reference types from the target package by vendoring its
    # bytecode modules as a local dependency.
    dep_name = "target_pkg"
    dep_dir = helper_dir / "deps" / dep_name
    if dep_dir.exists():
        shutil.rmtree(dep_dir)
    (dep_dir / "bytecode_modules").mkdir(parents=True, exist_ok=True)
    shutil.copy2(target_pkg_dir / "metadata.json", dep_dir / "metadata.json")

    # Move needs a Move.toml in the dependency package directory.
    # Bind the named address `target_pkg` to the actual module address from metadata.json.
    module_addr = "0x1"
    try:
        meta_obj = json.loads((dep_dir / "metadata.json").read_text(encoding="utf-8"))
        if isinstance(meta_obj, dict) and isinstance(meta_obj.get("module_address"), str):
            module_addr = meta_obj["module_address"]
        elif isinstance(meta_obj, dict) and isinstance(meta_obj.get("originalPackageId"), str):
            module_addr = meta_obj["originalPackageId"]
    except Exception:
        pass
    dep_move_toml = (
        "[package]\n"
        "name = \"target_pkg\"\n"
        "version = \"0.0.1\"\n\n"
        "[addresses]\n"
        f"target_pkg = \"{module_addr}\"\n"
    )
    _atomic_write_text(dep_dir / "Move.toml", dep_move_toml)

    src_mods = target_pkg_dir / "bytecode_modules"
    for mv in sorted(src_mods.glob("*.mv")):
        shutil.copy2(mv, dep_dir / "bytecode_modules" / mv.name)

    # Inject minimal dependency stanza into Move.toml if not present.
    toml_path = helper_dir / "Move.toml"
    toml = toml_path.read_text(encoding="utf-8")

    # Force legacy-friendly edition to reduce model-induced build failures.
    if "edition" in toml:
        lines = []
        for line in toml.splitlines():
            if line.strip().startswith("edition"):
                continue
            lines.append(line)
        toml = "\n".join(lines).rstrip() + "\n"
    if "[package]" in toml and "edition" not in toml:
        # Insert edition after [package] line.
        out_lines = []
        inserted = False
        for line in toml.splitlines():
            out_lines.append(line)
            if not inserted and line.strip() == "[package]":
                out_lines.append("edition = \"legacy\"")
                inserted = True
        toml = "\n".join(out_lines).rstrip() + "\n"

    # Strip model-added dependency blocks; we own dependency injection.
    if "[dependencies]" in toml:
        toml = toml.split("[dependencies]", 1)[0].rstrip() + "\n"
    if "[dependencies]" not in toml:
        toml = toml.rstrip() + "\n\n[dependencies]\n"
    if f"{dep_name} =" not in toml:
        # Path is relative to Move.toml, so prefix with ./
        toml = toml.rstrip() + f"\n{dep_name} = {{ local = \"./deps/{dep_name}\" }}\n"

    # Provide address mapping so callers can use the on-chain/original module address.
    if "[addresses]" not in toml:
        toml = toml.rstrip() + "\n\n[addresses]\n"
    if f"{dep_name} =" not in toml.split("[addresses]", 1)[-1]:
        toml = toml.rstrip() + f"\n{dep_name} = \"{module_addr}\"\n"
    _atomic_write_text(toml_path, toml)


def _sui_move_build(helper_dir: Path) -> tuple[bool, str, str]:
    # Prefer system `sui` if present.
    proc = _run(["sui", "move", "build"], cwd=helper_dir, timeout_s=300)
    return proc.returncode == 0, proc.stdout, proc.stderr


def _sui_move_build_with_bytecode(helper_dir: Path) -> tuple[bool, str, str]:
    proc = _run(
        ["sui", "move", "build", "--dump-bytecode-as-base64", "--with-unpublished-dependencies"],
        cwd=helper_dir,
        timeout_s=300,
    )
    return proc.returncode == 0, proc.stdout, proc.stderr


def _run_local_vm_entry(*, helper_dir: Path, call_target: str, out_path: Path) -> tuple[bool, dict[str, Any]]:
    # Leverage the Rust `benchmark-local` command as the local VM execution harness.
    # It executes entry functions when params are empty and tier-b is enabled.
    # We run it over the helper package bytecode corpus and then filter for the requested target.
    build_bytecode = helper_dir / "build" / "helper_pkg" / "bytecode_modules"
    target_corpus = build_bytecode if build_bytecode.exists() else (helper_dir / "bytecode_modules")
    tmp_out = out_path.parent / "txsim_benchmark_local.jsonl"

    proc = _run(
        [
            *_benchmark_local_cli_prefix(),
            "benchmark-local",
            "--target-corpus",
            str(target_corpus),
            "--output",
            str(tmp_out),
            "--restricted-state",
        ],
        cwd=REPO_ROOT,
        timeout_s=600,
    )
    _atomic_write_text(out_path.parent / "txsim_benchmark_local_stdout.log", proc.stdout)
    _atomic_write_text(out_path.parent / "txsim_benchmark_local_stderr.log", proc.stderr)
    if proc.returncode != 0:
        return False, {"error": "benchmark-local failed", "returncode": proc.returncode}

    match: dict[str, Any] | None = None
    for line in tmp_out.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        tgt = f"{row.get('target_package')}::{row.get('target_module')}::{row.get('target_function')}"
        if tgt == call_target:
            match = row
            break
    if match is None:
        return False, {"error": "target not found in local benchmark output", "target": call_target}

    _atomic_write_json(out_path, match)
    _atomic_write_text(out_path.parent / "txsim_benchmark_local.jsonl", tmp_out.read_text(encoding="utf-8"))
    return True, match


def _run_local_vm_entry_on_corpus(*, corpus_dir: Path, call_target: str, out_path: Path) -> tuple[bool, dict[str, Any]]:
    tmp_out = out_path.parent / "txsim_target_benchmark_local.jsonl"
    proc = _run(
        [
            *_benchmark_local_cli_prefix(),
            "benchmark-local",
            "--target-corpus",
            str(corpus_dir),
            "--output",
            str(tmp_out),
            "--restricted-state",
        ],
        cwd=REPO_ROOT,
        timeout_s=600,
    )
    _atomic_write_text(out_path.parent / "txsim_target_benchmark_local_stdout.log", proc.stdout)
    _atomic_write_text(out_path.parent / "txsim_target_benchmark_local_stderr.log", proc.stderr)
    if proc.returncode != 0:
        return False, {"error": "benchmark-local failed", "returncode": proc.returncode}

    match: dict[str, Any] | None = None
    for line in tmp_out.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        tgt = f"{row.get('target_package')}::{row.get('target_module')}::{row.get('target_function')}"
        if tgt == call_target:
            match = row
            break
    if match is None:
        return False, {"error": "target not found in local benchmark output", "target": call_target}

    _atomic_write_json(out_path, match)
    _atomic_write_text(out_path.parent / "txsim_target_benchmark_local.jsonl", tmp_out.read_text(encoding="utf-8"))
    return True, match


def _build_combined_corpus_dir(*, run_dir: Path, target_pkg_dir: Path, helper_dir: Path) -> Path:
    # Combine target + helper bytecode modules into a single corpus directory so benchmark-local
    # can resolve cross-module types and still execute entry functions.
    combined = run_dir / "combined_corpus" / "bytecode_modules"
    if combined.parent.exists():
        shutil.rmtree(combined.parent)
    combined.mkdir(parents=True, exist_ok=True)

    # Copy target first.
    for mv in sorted((target_pkg_dir / "bytecode_modules").glob("*.mv")):
        shutil.copy2(mv, combined / mv.name)

    # Copy helper modules (built).
    build_bytecode = helper_dir / "build" / "helper_pkg" / "bytecode_modules"
    helper_mods = build_bytecode if build_bytecode.exists() else (helper_dir / "bytecode_modules")
    for mv in sorted(helper_mods.glob("*.mv")):
        # Avoid overwriting: helper should not conflict with target.
        if (combined / mv.name).exists():
            continue
        shutil.copy2(mv, combined / mv.name)

    return combined


def _mm2_map_helper(*, helper_dir: Path, out_path: Path) -> tuple[bool, dict[str, Any]]:
    # Use the Rust no-chain local benchmark as Tier A/B executor for helper package bytecode.
    # This is the current in-repo path to MM2-style local validation (no RPC, no gas).
    tmp_out = out_path.parent / "mm2_benchmark_local.jsonl"
    # `sui move build` emits bytecode under build/<pkg>/bytecode_modules.
    build_bytecode = helper_dir / "build" / "helper_pkg" / "bytecode_modules"
    target_corpus = build_bytecode if build_bytecode.exists() else (helper_dir / "bytecode_modules")
    if not target_corpus.exists():
        return False, {"error": f"missing bytecode_modules at {target_corpus}"}

    proc = _run(
        [
            *_benchmark_local_cli_prefix(),
            "benchmark-local",
            "--target-corpus",
            str(target_corpus),
            "--output",
            str(tmp_out),
            "--restricted-state",
        ],
        cwd=REPO_ROOT,
        timeout_s=600,
    )
    _atomic_write_text(out_path.parent / "mm2_benchmark_local_stdout.log", proc.stdout)
    _atomic_write_text(out_path.parent / "mm2_benchmark_local_stderr.log", proc.stderr)
    if proc.returncode != 0:
        return False, {"error": "benchmark-local failed", "returncode": proc.returncode}

    # Parse JSONL into a compact mapping summary.
    accepted: list[dict[str, Any]] = []
    rejected: list[dict[str, Any]] = []
    for line in tmp_out.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        status = row.get("status")
        if status in {"tier_a_hit", "tier_b_hit"}:
            accepted.append(row)
        else:
            rejected.append(row)

    data = {
        "schema_version": 1,
        "kind": "mm2_mapping",
        "helper_dir": str(helper_dir),
        "accepted": accepted,
        "rejected": rejected,
    }
    _atomic_write_json(out_path, data)
    _atomic_write_text(out_path.parent / "mm2_benchmark_local.jsonl", tmp_out.read_text(encoding="utf-8"))
    return True, data


def _mm2_map_target(*, target_pkg_dir: Path, out_path: Path) -> tuple[bool, dict[str, Any]]:
    tmp_out = out_path.parent / "mm2_target_benchmark_local.jsonl"
    corpus = target_pkg_dir / "bytecode_modules"
    if not corpus.exists():
        return False, {"error": f"missing bytecode_modules at {corpus}"}

    proc = _run(
        [
            *_benchmark_local_cli_prefix(),
            "benchmark-local",
            "--target-corpus",
            str(corpus),
            "--output",
            str(tmp_out),
            "--restricted-state",
        ],
        cwd=REPO_ROOT,
        timeout_s=600,
    )
    _atomic_write_text(out_path.parent / "mm2_target_benchmark_local_stdout.log", proc.stdout)
    _atomic_write_text(out_path.parent / "mm2_target_benchmark_local_stderr.log", proc.stderr)
    if proc.returncode != 0:
        return False, {"error": "benchmark-local failed", "returncode": proc.returncode}

    accepted: list[dict[str, Any]] = []
    rejected: list[dict[str, Any]] = []
    for line in tmp_out.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        status = row.get("status")
        if status in {"tier_a_hit", "tier_b_hit"}:
            accepted.append(row)
        else:
            rejected.append(row)

    data = {
        "schema_version": 1,
        "kind": "mm2_mapping",
        "target": "target_pkg",
        "target_pkg_dir": str(target_pkg_dir),
        "accepted": accepted,
        "rejected": rejected,
    }
    _atomic_write_json(out_path, data)
    _atomic_write_text(out_path.parent / "mm2_target_benchmark_local.jsonl", tmp_out.read_text(encoding="utf-8"))
    return True, data


def _mm2_map_combined(*, combined_corpus_dir: Path, out_path: Path) -> tuple[bool, dict[str, Any]]:
    tmp_out = out_path.parent / "mm2_combined_benchmark_local.jsonl"
    if not combined_corpus_dir.exists():
        return False, {"error": f"missing combined corpus at {combined_corpus_dir}"}

    proc = _run(
        [
            *_benchmark_local_cli_prefix(),
            "benchmark-local",
            "--target-corpus",
            str(combined_corpus_dir),
            "--output",
            str(tmp_out),
            "--restricted-state",
        ],
        cwd=REPO_ROOT,
        timeout_s=600,
    )
    _atomic_write_text(out_path.parent / "mm2_combined_benchmark_local_stdout.log", proc.stdout)
    _atomic_write_text(out_path.parent / "mm2_combined_benchmark_local_stderr.log", proc.stderr)
    if proc.returncode != 0:
        return False, {"error": "benchmark-local failed", "returncode": proc.returncode}

    accepted: list[dict[str, Any]] = []
    rejected: list[dict[str, Any]] = []
    for line in tmp_out.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        status = row.get("status")
        if status in {"tier_a_hit", "tier_b_hit"}:
            accepted.append(row)
        else:
            rejected.append(row)

    data = {
        "schema_version": 1,
        "kind": "mm2_mapping",
        "target": "combined_target_plus_helper",
        "accepted": accepted,
        "rejected": rejected,
    }
    _atomic_write_json(out_path, data)
    _atomic_write_text(out_path.parent / "mm2_combined_benchmark_local.jsonl", tmp_out.read_text(encoding="utf-8"))
    return True, data


def _ptb_plan_from_mapping(*, mm2: dict[str, Any]) -> dict[str, Any]:
    # PTB generation: pick a Tier B-hit candidate with no params (so local VM can execute).
    # NOTE: benchmark-local currently treats any object param as non-executable in Tier B.
    accepted = mm2.get("accepted")
    if not isinstance(accepted, list) or not accepted:
        return {"calls": []}

    pick: dict[str, Any] | None = None
    for row in accepted:
        if isinstance(row, dict) and row.get("status") == "tier_b_hit":
            pick = row
            break
    if pick is None:
        return {"calls": []}

    target_mod = pick.get("target_module")
    func = pick.get("target_function")
    pkg = pick.get("target_package")
    if not (isinstance(target_mod, str) and isinstance(func, str) and isinstance(pkg, str)):
        return {"calls": []}

    # Build a best-effort Move target; args/type_args are empty because benchmark-local
    # currently only supports entry fns with no params for Tier B.
    target = f"{pkg}::{target_mod}::{func}"
    return {"calls": [{"target": target, "type_args": [], "args": []}]}


def _ptb_plan_target_first(*, target_mm2: dict[str, Any], helper_mm2: dict[str, Any]) -> dict[str, Any]:
    # Prefer a Tier B-executable target-package call; fall back to helper mapping.
    plan = _ptb_plan_from_mapping(mm2=target_mm2)
    if isinstance(plan.get("calls"), list) and plan["calls"]:
        return plan
    return _ptb_plan_from_mapping(mm2=helper_mm2)


def _validate_artifacts(*, run_dir: Path, mm2: dict[str, Any], txsim: dict[str, Any] | None) -> dict[str, Any]:
    # Minimal validation report; real checks will be tightened as wiring is completed.
    ok = True
    errors: list[str] = []
    if not (run_dir / "helper_pkg" / "Move.toml").exists():
        ok = False
        errors.append("missing helper_pkg/Move.toml")
    if not (run_dir / "target_interface.json").exists():
        ok = False
        errors.append("missing target_interface.json")
    if not (run_dir / "mm2_target_mapping.json").exists():
        ok = False
        errors.append("missing mm2_target_mapping.json")
    if not (run_dir / "mm2_combined_mapping.json").exists():
        ok = False
        errors.append("missing mm2_combined_mapping.json")
    if not (run_dir / "txsim_source.json").exists():
        ok = False
        errors.append("missing txsim_source.json")
    if mm2.get("kind") != "mm2_mapping":
        ok = False
        errors.append("unexpected mm2 kind")

    tmm2 = None
    try:
        tmm2 = json.loads((run_dir / "mm2_target_mapping.json").read_text(encoding="utf-8"))
    except Exception:
        tmm2 = None
    if not isinstance(tmm2, dict) or tmm2.get("kind") != "mm2_mapping":
        ok = False
        errors.append("invalid mm2_target_mapping.json")
    else:
        tacc = tmm2.get("accepted")
        if not isinstance(tacc, list) or not tacc:
            ok = False
            errors.append("no target mm2 accepted hits")
        else:
            # Require at least Tier A evidence for the raw target package.
            if not any(isinstance(r, dict) and r.get("status") in {"tier_a_hit", "tier_b_hit"} for r in tacc):
                ok = False
                errors.append("no target hit (tier_a_hit or tier_b_hit)")

    accepted = mm2.get("accepted")
    if not isinstance(accepted, list) or not accepted:
        ok = False
        errors.append("no mm2 accepted hits")
    if txsim is None:
        ok = False
        errors.append("missing txsim effects")
    else:
        status = txsim.get("status")
        if status != "tier_b_hit":
            ok = False
            errors.append(f"txsim not tier_b_hit (status={status})")

    # Real-corpus evidence: require at least one Tier B hit in the combined mapping.
    try:
        cmm2 = json.loads((run_dir / "mm2_combined_mapping.json").read_text(encoding="utf-8"))
    except Exception:
        cmm2 = None
    if isinstance(cmm2, dict):
        acc = cmm2.get("accepted")
        if isinstance(acc, list):
            is_fake = (run_dir / "run_config.json").read_text(encoding="utf-8").find("fake_corpus") != -1
            if not is_fake:
                if not any(isinstance(r, dict) and r.get("status") == "tier_b_hit" for r in acc):
                    ok = False
                    errors.append("no combined tier_b_hit")
    return {"ok": ok, "errors": errors}


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description="E2E: pkg -> LLM helper -> MM2 -> tx-sim")
    p.add_argument("--corpus-root", type=Path, required=True)
    p.add_argument("--dataset", type=str, default="type_inhabitation_top25")
    p.add_argument("--samples", type=int, default=1)
    p.add_argument("--package-id", type=str, default=None)
    p.add_argument("--dataset-index", type=int, default=0)
    p.add_argument("--dataset-count", type=int, default=1)
    p.add_argument("--per-package-timeout-seconds", type=int, default=120)
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--model", type=str, default=None)
    p.add_argument("--enable-dryrun", action="store_true")
    p.add_argument("--out-dir", type=Path, default=BENCH_ROOT / "results")
    p.add_argument(
        "--persist-tmp-dir",
        type=Path,
        default=None,
        help="If set, copy any /tmp benchmark-local artifacts and logs into this directory for postmortem analysis.",
    )
    args = p.parse_args(argv)

    # Load benchmark/.env so real LLM runs work even when the caller didn't export env vars.
    try:
        from smi_bench.env import load_dotenv

        dotenv = load_dotenv(BENCH_ROOT / ".env")
        for k, v in dotenv.items():
            os.environ.setdefault(k, v)
    except Exception:
        pass

    if args.dataset_count < 1:
        raise SystemExit("dataset-count must be >= 1")
    if args.per_package_timeout_seconds < 1:
        raise SystemExit("per-package-timeout-seconds must be >= 1")

    if args.package_id:
        pkg_ids = [args.package_id]
    else:
        ids = _read_dataset_ids(args.dataset)
        if not ids:
            raise SystemExit(f"dataset empty: {args.dataset}")
        if args.dataset_index < 0 or args.dataset_index >= len(ids):
            raise SystemExit(f"dataset-index out of range: {args.dataset_index} (len={len(ids)})")
        end = min(len(ids), args.dataset_index + args.dataset_count)
        pkg_ids = ids[args.dataset_index:end]

    overall_ok = True
    for pkg_id in pkg_ids:
        started_pkg = time.monotonic()
        target_pkg_dir = _find_package_dir_in_corpus(corpus_root=args.corpus_root, package_id=pkg_id)
        stamp = int(time.time())
        run_dir = args.out_dir / f"e2e_{stamp}_{pkg_id[:10]}"
        run_dir.mkdir(parents=True, exist_ok=True)

        # If running in Docker/CI, persist temp artifacts under the run directory.
        tmp_root = args.persist_tmp_dir
        if tmp_root is not None:
            tmp_root.mkdir(parents=True, exist_ok=True)

        cfg = RunConfig(
            corpus_root=args.corpus_root,
            dataset=args.dataset,
            samples=args.samples,
            seed=args.seed,
            model=args.model,
            enable_dryrun=bool(args.enable_dryrun),
            out_dir=run_dir,
        )

        if args.persist_tmp_dir is not None:
            (run_dir / "persisted_tmp").mkdir(parents=True, exist_ok=True)
        _atomic_write_json(
            run_dir / "run_config.json",
            {
                "corpus_root": str(cfg.corpus_root),
                "dataset": cfg.dataset,
                "samples": cfg.samples,
                "package_id": pkg_id,
                "target_pkg_dir": str(target_pkg_dir),
                "dataset_index": args.dataset_index,
                "dataset_count": args.dataset_count,
                "per_package_timeout_seconds": args.per_package_timeout_seconds,
                "seed": cfg.seed,
                "model": cfg.model,
                "enable_dryrun": cfg.enable_dryrun,
                "out_dir": str(cfg.out_dir),
            },
        )

        def remaining_s() -> float:
            return max(0.0, float(args.per_package_timeout_seconds) - (time.monotonic() - started_pkg))

        def timed_out() -> bool:
            return remaining_s() <= 0.0

        # LLM helper package generation
        # Summarize target package interface to condition helper generation.
        iface_summary = ""
        requested_targets: list[str] = []
        allowed_target_modules: set[str] = set()
        target_addr = ""
        try:
            from smi_bench.inhabit.executable_subset import summarize_interface
            from smi_bench.rust import default_rust_binary, emit_bytecode_json, validate_rust_binary

            rust_bin = default_rust_binary()
            validate_rust_binary(rust_bin)
            iface = emit_bytecode_json(package_dir=target_pkg_dir, rust_bin=rust_bin)
            # Most packages compile modules under non-package-id addresses; treat module address as ground truth.
            target_addr = iface.get("package_id") if isinstance(iface, dict) else ""
            iface_summary = summarize_interface(iface, max_functions=30, mode="entry_then_public")
            _atomic_write_json(run_dir / "target_interface.json", iface)
            requested_targets = _extract_simple_entry_targets_from_interface(iface=iface, limit=5)
            allowed_target_modules = _extract_module_names_from_interface(iface)
        except Exception as e:
            iface_summary = f"<failed to summarize interface: {e}>"

        prompt = {
            "schema": "helper_pkg_v1",
            "package_id": pkg_id,
            "seed": cfg.seed,
            "target_interface_summary": iface_summary,
            "requested_targets": requested_targets,
            "allowed_target_address": target_addr,
            "allowed_target_modules": sorted(allowed_target_modules),
            "instruction": (
                "Generate a local helper Move package as JSON: {move_toml, files, entrypoints, assumptions}. "
                "IMPORTANT: move_toml MUST be a STRING containing the full Move.toml file contents. "
                "Do NOT return a structured object for move_toml. "
                "All keys in files MUST be relative paths under sources/ (e.g. 'sources/helper.move'). "
                "entrypoints[*].target MUST be formatted as 'addr::module::func' (exactly 2 '::'). "
                "entrypoints[*].target MUST be one of requested_targets (if requested_targets is non-empty). "
                "Do NOT add any git dependencies. Do NOT add any local dependencies; the runner will add dependencies automatically. "
                "In Move source: you MAY use 0x1/0x2/0x3 framework modules. "
                "Avoid importing or referencing any other on-chain 0x... addresses besides the target package unless absolutely necessary. "
                "If you reference the target package, prefer fully-qualified calls `0xADDR::module::func` matching the provided interface. "
                "The helper package should include at least one public entry function that can be executed locally "
                "(prefer zero-arg entry if possible), and should be designed to help inhabit types/functions from the target interface."
            ),
        }
        _atomic_write_json(run_dir / "llm_request.json", prompt)

        if timed_out():
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["package timeout"]})
            overall_ok = False
            continue

        use_real = os.environ.get("SMI_E2E_REAL_LLM") == "1"
        helper_payload: dict[str, Any] | None = None
        last_errors: list[str] = []
        if use_real:
            for attempt in range(1, 4):
                if timed_out():
                    _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["package timeout"]})
                    overall_ok = False
                    break
                try:
                    raw = _call_real_llm_with_repair(model=cfg.model, prompt_obj=prompt, seed=cfg.seed + attempt)
                except Exception as e:
                    last_errors = [f"llm request failed: {e}"]
                    prompt = dict(prompt)
                    prompt["repair"] = "Return ONLY valid helper_pkg_v1 JSON. Prior request failed: " + str(e)
                    continue

                _atomic_write_json(run_dir / f"llm_response_attempt_{attempt}.json", raw)
                raw = _coerce_model_output_to_helper_payload(raw)
                try:
                    helper_payload = _validate_llm_helper_payload(raw)
                    _enforce_entrypoints_target_requested(
                        helper_payload=helper_payload,
                        requested_targets=requested_targets,
                    )
                except Exception as e:
                    last_errors = [f"llm payload invalid: {e}"]
                    prompt = dict(prompt)
                    prompt["repair"] = "Return ONLY valid helper_pkg_v1 JSON. Validation error: " + str(e)
                    continue

                lint_errors = _lint_model_move_sources(
                    move_sources=helper_payload["files"],
                    allowed_target_modules=allowed_target_modules,
                    allowed_target_modules_to_addr={},
                )
                if lint_errors:
                    last_errors = ["lint failed", *lint_errors]
                    _atomic_write_json(run_dir / f"lint_errors_attempt_{attempt}.json", {"errors": lint_errors})
                    prompt = dict(prompt)
                    prompt["repair"] = (
                        "Fix these exact issues, then return ONLY helper_pkg_v1 JSON.\n- "
                        + "\n- ".join(lint_errors[:50])
                    )
                    helper_payload = None
                    continue

                break
        else:
            raw = _stub_llm_helper_payload()
            _atomic_write_json(run_dir / "llm_response.json", raw)
            raw = _coerce_model_output_to_helper_payload(raw)
            try:
                helper_payload = _validate_llm_helper_payload(raw)
            except Exception as e:
                last_errors = [f"llm payload invalid: {e}"]

        if helper_payload is None:
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": last_errors or ["llm payload invalid"]})
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue

        if timed_out():
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["package timeout"]})
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue

        helper_dir = run_dir / "helper_pkg"
        _write_helper_package(helper_dir=helper_dir, payload=helper_payload)

        # Lint was applied before build.

        # Vendor target bytecode modules as a local dependency so the helper can reference target types.
        _vendor_target_deps_into_helper(target_pkg_dir=target_pkg_dir, helper_dir=helper_dir)

        ok_build, out_s, err_s = _sui_move_build_with_bytecode(helper_dir)
        _atomic_write_text(run_dir / "helper_build_stdout.log", out_s)
        _atomic_write_text(run_dir / "helper_build_stderr.log", err_s)
        if not ok_build:
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["helper package build failed"]})
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue

        if timed_out():
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["package timeout"]})
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue

        # MM2 mapping/type-inhabitation
        ok_mm2, mm2 = _mm2_map_helper(helper_dir=helper_dir, out_path=run_dir / "mm2_mapping.json")
        if not ok_mm2:
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["mm2 mapping failed"]})
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue
        _atomic_write_json(run_dir / "mm2_summary.json", {"accepted": len(mm2.get("accepted", [])), "rejected": len(mm2.get("rejected", []))})

        ok_tmm2, tmm2 = _mm2_map_target(target_pkg_dir=target_pkg_dir, out_path=run_dir / "mm2_target_mapping.json")
        if not ok_tmm2:
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["target mm2 mapping failed"]})
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue
        _atomic_write_json(
            run_dir / "mm2_target_summary.json",
            {"accepted": len(tmm2.get("accepted", [])), "rejected": len(tmm2.get("rejected", []))},
        )

        combined_corpus_dir = _build_combined_corpus_dir(run_dir=run_dir, target_pkg_dir=target_pkg_dir, helper_dir=helper_dir)
        ok_cmm2, cmm2 = _mm2_map_combined(combined_corpus_dir=combined_corpus_dir, out_path=run_dir / "mm2_combined_mapping.json")
        if not ok_cmm2:
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["combined mm2 mapping failed"]})
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue
        _atomic_write_json(
            run_dir / "mm2_combined_summary.json",
            {"accepted": len(cmm2.get("accepted", [])), "rejected": len(cmm2.get("rejected", []))},
        )

        # PTB plan + tx-sim: combined target+helper first (best chance of Tier B), then target, then helper.
        ptb = _ptb_plan_target_first(target_mm2=cmm2, helper_mm2=tmm2)
        if not (isinstance(ptb.get("calls"), list) and ptb["calls"]):
            ptb = _ptb_plan_target_first(target_mm2=tmm2, helper_mm2=mm2)
        _atomic_write_json(run_dir / "ptb_plan.json", ptb)

        if timed_out():
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["package timeout"]})
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue

        txsim: dict[str, Any] | None = None
        txsim_source = None
        if isinstance(ptb.get("calls"), list) and ptb["calls"]:
            first_call = ptb["calls"][0]
            if isinstance(first_call, dict) and isinstance(first_call.get("target"), str):
                tgt = first_call["target"]
                if isinstance(tmm2.get("target"), str) and tmm2.get("target") == "target_pkg" and tgt.startswith(pkg_id + "::"):
                    ok_tx, tx = _run_local_vm_entry_on_corpus(
                        corpus_dir=target_pkg_dir / "bytecode_modules",
                        call_target=tgt,
                        out_path=run_dir / "txsim_target_effects.json",
                    )
                    if ok_tx:
                        txsim = tx
                        txsim_source = "target"
                if txsim is None:
                    ok_tx, tx = _run_local_vm_entry_on_corpus(
                        corpus_dir=combined_corpus_dir,
                        call_target=tgt,
                        out_path=run_dir / "txsim_combined_effects.json",
                    )
                    if ok_tx:
                        txsim = tx
                        txsim_source = "combined"
                if txsim is None:
                    ok_tx, tx = _run_local_vm_entry(
                        helper_dir=helper_dir,
                        call_target=tgt,
                        out_path=run_dir / "txsim_effects.json",
                    )
                    if ok_tx:
                        txsim = tx
                        txsim_source = "helper"

        if txsim_source is not None:
            _atomic_write_json(run_dir / "txsim_source.json", {"source": txsim_source})

        report = _validate_artifacts(run_dir=run_dir, mm2=mm2, txsim=txsim)
        _atomic_write_json(run_dir / "validation_report.json", report)
        _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)

        hit = bool(report.get("ok") is True)
        _atomic_write_json(
            run_dir / "hit_metric.json",
            {
                "package_id": pkg_id,
                "hit": hit,
            },
        )
        overall_ok = overall_ok and hit

        hashes: dict[str, str] = {}
        for rel in [
            "run_config.json",
            "llm_request.json",
            "llm_response.json",
            "mm2_mapping.json",
            "mm2_summary.json",
            "mm2_target_mapping.json",
            "mm2_target_summary.json",
            "mm2_combined_mapping.json",
            "mm2_combined_summary.json",
            "txsim_source.json",
            "txsim_target_effects.json",
            "txsim_combined_effects.json",
            "ptb_plan.json",
            "validation_report.json",
            "hit_metric.json",
            "target_interface.json",
            "helper_pkg/Move.toml",
        ]:
            pth = run_dir / rel
            if pth.exists():
                hashes[rel] = _sha256_file(pth)
        _atomic_write_json(run_dir / "artifact_hashes.json", hashes)

    return 0 if overall_ok else 6


if __name__ == "__main__":
    raise SystemExit(main())
