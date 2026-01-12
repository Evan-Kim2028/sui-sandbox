#!/usr/bin/env python3
"""
E2E One-Package Benchmark Runner
================================

End-to-end evaluation pipeline for LLM-based Move code generation:
  Target Package -> LLM Helper Generation -> Move Build -> Local TX Simulation

QUICK START
-----------

1. Offline test (no API key needed):
   $ cd benchmark
   $ uv run python scripts/e2e_one_package.py \\
       --corpus-root tests/fake_corpus \\
       --package-id 0x1 \\
       --out-dir results/my_test

2. Real LLM test (requires API key):
   $ export SMI_E2E_REAL_LLM=1
   $ export OPENROUTER_API_KEY=sk-or-v1-...
   $ uv run python scripts/e2e_one_package.py \\
       --corpus-root ../sui-packages/packages/mainnet_most_used \\
       --dataset type_inhabitation_top25 \\
       --samples 5 \\
       --model google/gemini-3-flash-preview \\
       --out-dir results/e2e_run

PIPELINE STAGES
---------------

Stage 1: Target Package Analysis
  - Extracts bytecode interface JSON from target package
  - Identifies entry functions and callable targets

Stage 2: LLM Helper Package Generation
  - Prompts LLM to generate a Move helper package
  - Helper package calls target package entry functions
  - Validates LLM output against helper_pkg_v1 schema

Stage 3: Move Build
  - Compiles helper package with `sui move build`
  - Vendors target package bytecode as local dependency
  - Captures build errors for LLM repair loop

Stage 4: MM2 Mapping & TX Simulation
  - Runs local transaction simulator on helper bytecode
  - Maps helper entrypoints to target package functions
  - Reports execution success/failure and created types

OUTPUT ARTIFACTS
----------------

Each run creates a directory: <out_dir>/e2e_<timestamp>_<random>/
  validation_report.json   - Overall success/failure with error details
  mm2_mapping.json         - Function mapping results (accepted/rejected)
  txsim_source.json        - Transaction simulation input
  txsim_effects.json       - Transaction execution effects
  helper_pkg/              - Generated Move helper package
  interface.json           - Target package bytecode interface
  prompt.json              - LLM prompt sent
  llm_response.json        - Raw LLM response

ENVIRONMENT VARIABLES
---------------------

SMI_E2E_REAL_LLM     Set to "1" to use real LLM (default: offline stub)
OPENROUTER_API_KEY   OpenRouter API key (or use SMI_API_KEY, OPENAI_API_KEY)
SMI_API_BASE_URL     Custom API base URL (default: https://openrouter.ai/api/v1)
SMI_MODEL            Default model if --model not specified
SMI_TEMPERATURE      LLM temperature (default: 0)
SMI_MAX_TOKENS       Max response tokens (default: 4096)

COMMON ISSUES
-------------

"dataset not found" - Dataset file missing; check manifests/datasets/<name>.txt
"package_id not found in corpus_root" - Package directory missing bytecode_modules/
"helper_pkg_v1 schema validation failed" - LLM response doesn't match expected format
"benchmark-local failed" - Rust binary not built; run `cargo build --release`

SEE ALSO
--------

- benchmark/GETTING_STARTED.md - Full benchmark documentation
- benchmark/docs/ARCHITECTURE.md - System internals
- benchmark/tests/test_e2e_one_package.py - Offline test coverage
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import shutil
import subprocess
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
    # JSON Schema gate first (optional - gracefully degrade if jsonschema not installed).
    try:
        import jsonschema

        schema_path = SCHEMAS_DIR / "helper_pkg_v1.schema.json"
        if schema_path.exists():
            schema = json.loads(schema_path.read_text(encoding="utf-8"))
            jsonschema.validate(instance=obj, schema=schema)
    except ImportError:
        pass  # jsonschema not installed, skip schema validation
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

    # P0 Security: Limit file count to prevent DoS via many small files
    MAX_FILE_COUNT = 100
    if len(files) > MAX_FILE_COUNT:
        raise ValueError(f"too many files in helper package (max {MAX_FILE_COUNT}, got {len(files)})")

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
            # Support both "params" (bytecode JSON) and "parameters" (other formats)
            params = fn.get("params") or fn.get("parameters")
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


def _extract_module_address_from_interface(iface: dict[str, Any]) -> str | None:
    """
    Extract the actual module address from the interface JSON.

    On Sui, the package ID (on-chain object address) can differ from the module address
    (the address where bytecode modules are compiled). The module address is what the
    Move compiler needs for imports.

    Returns the module address from the first module found, or None if not found.
    """
    modules = iface.get("modules")
    if not isinstance(modules, dict):
        return None
    for mod_name, mod in modules.items():
        if isinstance(mod, dict) and isinstance(mod.get("address"), str):
            return mod["address"]
    return None


def _lint_model_move_sources(
    *,
    move_sources: dict[str, str],
    allowed_target_modules: set[str],
    allowed_target_modules_to_addr: dict[str, str],
) -> list[str]:
    # DISABLED: Lint was blocking LLM from importing target package, which is required
    # for true type inhabitation. The Move compiler will catch actual errors.
    # Re-enable if we need stricter pre-build validation in the future.
    return []


def _run(
    cmd: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None, timeout_s: int = 300
) -> subprocess.CompletedProcess:
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
    api_key = (
        os.environ.get("SMI_API_KEY") or os.environ.get("OPENROUTER_API_KEY") or os.environ.get("OPENAI_API_KEY") or ""
    )
    if not api_key.strip():
        raise RuntimeError("missing API key (set SMI_API_KEY or OPENROUTER_API_KEY or OPENAI_API_KEY)")

    cfg = RealAgentConfig(
        provider="openai_compatible",
        model=model or os.environ.get("SMI_MODEL") or "openai/gpt-5.2",
        base_url=os.environ.get("SMI_API_BASE_URL")
        or os.environ.get("OPENROUTER_BASE_URL")
        or "https://openrouter.ai/api/v1",
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
    response = agent.complete_json(prompt)
    # LLMJsonResponse has .content (dict) and .usage (LLMUsage); we only need content
    return response.content


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


def _type_to_move(t: dict[str, Any], pkg_alias: str = "target_pkg") -> str:
    """Convert interface JSON type representation to Move source syntax.

    .. deprecated:: 0.4.0
        This is part of the deprecated Python stub generator. Use the Rust
        extractor's ``--emit-move-stubs`` instead.
    """
    kind = t.get("kind", "")

    if kind == "bool":
        return "bool"
    elif kind == "u8":
        return "u8"
    elif kind == "u16":
        return "u16"
    elif kind == "u32":
        return "u32"
    elif kind == "u64":
        return "u64"
    elif kind == "u128":
        return "u128"
    elif kind == "u256":
        return "u256"
    elif kind == "address":
        return "address"
    elif kind == "signer":
        return "signer"
    elif kind == "vector":
        inner = _type_to_move(t.get("element", {"kind": "u8"}), pkg_alias)
        return f"vector<{inner}>"
    elif kind == "type_param":
        idx = t.get("index", 0)
        # Use T0, T1, T2, etc. for type params
        return f"T{idx}"
    elif kind == "ref":
        inner = _type_to_move(t.get("to", {"kind": "u8"}), pkg_alias)
        if t.get("mutable"):
            return f"&mut {inner}"
        return f"&{inner}"
    elif kind == "datatype":
        addr = t.get("address", "")
        module = t.get("module", "")
        name = t.get("name", "")
        type_args = t.get("type_args", [])

        # Determine the module prefix based on well-known framework addresses
        # 0x1 = MoveStdlib (std::)
        # 0x2 = Sui Framework (sui::)
        # 0x3 = SuiSystem (sui_system::)
        if addr in ("0x1", "0x0000000000000000000000000000000000000000000000000000000000000001"):
            prefix = f"std::{module}"
        elif addr in ("0x2", "0x0000000000000000000000000000000000000000000000000000000000000002"):
            prefix = f"sui::{module}"
        elif addr in ("0x3", "0x0000000000000000000000000000000000000000000000000000000000000003"):
            prefix = f"sui_system::{module}"
        else:
            # This is a target package type - use the alias
            prefix = f"{pkg_alias}::{module}"

        if type_args:
            args_str = ", ".join(_type_to_move(a, pkg_alias) for a in type_args)
            return f"{prefix}::{name}<{args_str}>"
        return f"{prefix}::{name}"
    else:
        # Unknown type - use placeholder
        return "u8"


def _generate_move_source_stubs(interface: dict[str, Any], pkg_alias: str = "target_pkg") -> dict[str, str]:
    """Generate Move source stub files from interface JSON.

    .. deprecated:: 0.4.0
        Use the Rust extractor's ``--emit-move-stubs`` flag instead via
        :func:`smi_bench.rust.emit_move_stubs`. The Rust version generates
        correct Move 2024 syntax with proper ``use`` imports. This Python
        fallback has known issues with qualified type paths in struct fields
        (causes E03006 errors).

    Creates minimal .move source files that declare all types and function signatures
    from the bytecode interface. Function bodies are stubs that abort.
    This allows the Move compiler to type-check code that imports these types.

    Args:
        interface: The bytecode interface JSON (from emit_bytecode_json)
        pkg_alias: The alias to use for this package in Move code

    Returns:
        Dict mapping module name to Move source code string
    """
    import warnings
    warnings.warn(
        "_generate_move_source_stubs is deprecated since v0.4.0. "
        "Use smi_bench.rust.emit_move_stubs() instead for correct Move 2024 syntax.",
        DeprecationWarning,
        stacklevel=2,
    )
    modules = interface.get("modules", {})
    sources: dict[str, str] = {}

    for mod_name, mod_data in modules.items():
        lines: list[str] = []

        # Module header
        lines.append(f"module {pkg_alias}::{mod_name} {{")
        lines.append("")

        # Imports for common types used in stubs
        lines.append("    // Auto-generated stub module for type-checking")
        lines.append("    // Function bodies abort - real bytecode used at runtime")
        lines.append("")

        # Generate struct declarations
        structs = mod_data.get("structs", {})
        for struct_name, struct_data in structs.items():
            abilities = struct_data.get("abilities", [])
            type_params = struct_data.get("type_params", [])
            fields = struct_data.get("fields", [])
            is_native = struct_data.get("is_native", False)

            # Build type params string
            tp_strs = []
            for i, tp in enumerate(type_params):
                constraints = tp.get("constraints", [])
                is_phantom = tp.get("is_phantom", False)
                tp_str = f"T{i}"
                if is_phantom:
                    tp_str = f"phantom T{i}"
                if constraints:
                    tp_str += ": " + " + ".join(constraints)
                tp_strs.append(tp_str)
            tp_decl = f"<{', '.join(tp_strs)}>" if tp_strs else ""

            # Build abilities string
            abilities_str = f" has {', '.join(abilities)}" if abilities else ""

            if is_native:
                lines.append(f"    public struct {struct_name}{tp_decl}{abilities_str};")
            else:
                # Generate struct with fields
                lines.append(f"    public struct {struct_name}{tp_decl}{abilities_str} {{")
                for field in fields:
                    field_name = field.get("name", "f")
                    field_type = _type_to_move(field.get("type", {"kind": "u8"}), pkg_alias)
                    lines.append(f"        {field_name}: {field_type},")
                lines.append("    }")
            lines.append("")

        # Generate function declarations
        functions = mod_data.get("functions", {})
        for func_name, func_data in functions.items():
            visibility = func_data.get("visibility", "public")
            is_entry = func_data.get("is_entry", False)
            is_native = func_data.get("is_native", False)
            type_params = func_data.get("type_params", [])
            params = func_data.get("params", [])
            returns = func_data.get("returns", [])

            # Build visibility/entry prefix
            vis_str = "public" if visibility == "public" else "public(package)" if visibility == "friend" else ""
            entry_str = " entry" if is_entry else ""

            # Build type params
            tp_strs = []
            for i, tp in enumerate(type_params):
                constraints = tp.get("constraints", [])
                tp_str = f"T{i}"
                if constraints:
                    tp_str += ": " + " + ".join(constraints)
                tp_strs.append(tp_str)
            tp_decl = f"<{', '.join(tp_strs)}>" if tp_strs else ""

            # Build params
            param_strs = []
            for i, p in enumerate(params):
                param_type = _type_to_move(p, pkg_alias)
                param_strs.append(f"_p{i}: {param_type}")
            params_decl = ", ".join(param_strs)

            # Build return type
            if not returns:
                ret_decl = ""
            elif len(returns) == 1:
                ret_decl = f": {_type_to_move(returns[0], pkg_alias)}"
            else:
                ret_types = ", ".join(_type_to_move(r, pkg_alias) for r in returns)
                ret_decl = f": ({ret_types})"

            if is_native:
                lines.append(f"    {vis_str}{entry_str} native fun {func_name}{tp_decl}({params_decl}){ret_decl};")
            else:
                # Non-native function - generate abort stub
                lines.append(f"    {vis_str}{entry_str} fun {func_name}{tp_decl}({params_decl}){ret_decl} {{")
                lines.append("        abort 0")
                lines.append("    }")
            lines.append("")

        lines.append("}")
        sources[mod_name] = "\n".join(lines)

    return sources


def _vendor_target_deps_into_helper(
    *, target_pkg_dir: Path, helper_dir: Path, interface: dict[str, Any] | None = None, rust_bin: Path | None = None
) -> None:
    """Vendor target package as a local dependency for the helper package.

    Creates source stub files using the Rust extractor so the Move compiler can
    type-check imports. At runtime, the real bytecode is used for execution.

    Args:
        target_pkg_dir: Path to the target package directory (with bytecode_modules/)
        helper_dir: Path to the helper package directory
        interface: Optional interface JSON; if not provided, will be loaded from target_pkg_dir (DEPRECATED, unused now)
        rust_bin: Path to the Rust extractor binary; if not provided, will use default
    """
    dep_name = "target_pkg"
    dep_dir = helper_dir / "deps" / dep_name
    if dep_dir.exists():
        shutil.rmtree(dep_dir)

    # Create sources directory for stub files (Move compiler needs source, not just bytecode)
    sources_dir = dep_dir / "sources"
    sources_dir.mkdir(parents=True, exist_ok=True)

    # Also keep bytecode for runtime execution
    (dep_dir / "bytecode_modules").mkdir(parents=True, exist_ok=True)
    shutil.copy2(target_pkg_dir / "metadata.json", dep_dir / "metadata.json")

    # Get module address from metadata
    module_addr = "0x1"
    try:
        meta_obj = json.loads((dep_dir / "metadata.json").read_text(encoding="utf-8"))
        if isinstance(meta_obj, dict) and isinstance(meta_obj.get("module_address"), str):
            module_addr = meta_obj["module_address"]
        elif isinstance(meta_obj, dict) and isinstance(meta_obj.get("originalPackageId"), str):
            module_addr = meta_obj["originalPackageId"]
    except Exception:
        pass

    # Generate source stubs using Rust extractor (preferred method)
    # The Rust extractor generates correct Move 2024 syntax with proper imports
    try:
        from smi_bench.rust import default_rust_binary, emit_move_stubs, validate_rust_binary

        if rust_bin is None:
            rust_bin = default_rust_binary()
        validate_rust_binary(rust_bin)
        emit_move_stubs(package_dir=target_pkg_dir, stubs_dir=sources_dir, rust_bin=rust_bin)
    except Exception as e:
        # Fallback to Python-generated stubs if Rust extractor fails
        # This is a legacy path that may have syntax issues with Move 2024
        logger.warning(f"Rust stub generation failed, falling back to Python stubs: {e}")
        if interface is not None:
            source_stubs = _generate_move_source_stubs(interface, dep_name)
            for mod_name, source in source_stubs.items():
                _atomic_write_text(sources_dir / f"{mod_name}.move", source)

    # Create Move.toml for the dependency
    # The stubs may use `use std::`, `use sui::`, and `use sui_system::` imports,
    # so we need to include framework dependencies. We use the standard Sui framework
    # git dependencies which are automatically resolved by the Move compiler.
    dep_move_toml = (
        "[package]\n"
        f'name = "{dep_name}"\n'
        'version = "0.0.1"\n'
        'edition = "2024.beta"\n\n'
        "[dependencies]\n"
        'Sui = { git = "https://github.com/MystenLabs/sui.git", subdir = "crates/sui-framework/packages/sui-framework", rev = "framework/mainnet" }\n'
        'SuiSystem = { git = "https://github.com/MystenLabs/sui.git", subdir = "crates/sui-framework/packages/sui-system", rev = "framework/mainnet" }\n\n'
        "[addresses]\n"
        f'{dep_name} = "{module_addr}"\n'
        'std = "0x1"\n'
        'sui = "0x2"\n'
        'sui_system = "0x3"\n'
    )
    _atomic_write_text(dep_dir / "Move.toml", dep_move_toml)

    # Copy bytecode for runtime (our VM will use these)
    src_mods = target_pkg_dir / "bytecode_modules"
    for mv in sorted(src_mods.glob("*.mv")):
        shutil.copy2(mv, dep_dir / "bytecode_modules" / mv.name)

    # Inject minimal dependency stanza into Move.toml if not present.
    toml_path = helper_dir / "Move.toml"
    toml = toml_path.read_text(encoding="utf-8")

    # NOTE: We do NOT force any specific Move edition. The LLM can specify its own
    # edition in the Move.toml (e.g. "2024" or "legacy") and use corresponding syntax.

    # Strip model-added dependency blocks; we own dependency injection.
    if "[dependencies]" in toml:
        toml = toml.split("[dependencies]", 1)[0].rstrip() + "\n"

    # Always add [dependencies] section with target_pkg
    toml = toml.rstrip() + f'\n\n[dependencies]\n{dep_name} = {{ local = "./deps/{dep_name}" }}\n'

    # Provide address mapping so callers can use the on-chain/original module address.
    if "[addresses]" not in toml:
        toml = toml.rstrip() + "\n\n[addresses]\n"
    if f"{dep_name} =" not in toml.split("[addresses]", 1)[-1]:
        toml = toml.rstrip() + f'\n{dep_name} = "{module_addr}"\n'
    _atomic_write_text(toml_path, toml)


def _enhance_errors_with_sui_guidance(errors: list[str], combined_output: str) -> list[str]:
    """Append actionable guidance for common Sui-specific build errors.

    This simulates what a developer would learn from a quick docs lookup -
    providing the "ah, here's what you actually need to do" context that
    helps the LLM recover from tooling-specific issues.

    Sources (for internal auditing):
    --------------------------------
    [1] Common Errors: https://docs.sui.io/guides/developer/sui-101/common-errors
    [2] Move 2024 Migration: https://docs.sui.io/guides/developer/advanced/move-2024-migration
    [3] Coin Module API: https://docs.sui.io/references/framework/sui/coin
    [4] Move Book (Sui): https://move-book.com/
    [5] Move 2024 Migration Guide (Move Book): https://move-book.com/guides/2024-migration-guide/
    [6] Sui Object Model: https://docs.sui.io/concepts/object-model
    [7] Coin Standard: https://docs.sui.io/standards/coin
    [8] Create Coins: https://docs.sui.io/guides/developer/coin
    [9] Package Management: https://docs.sui.io/guides/developer/sui-101/move-package-management
    """
    enhanced = list(errors)
    lower_output = combined_output.lower()

    # =========================================================================
    # DEPENDENCY / MOVE.TOML ISSUES
    # Sources: [1] Common Errors, [9] Package Management
    # =========================================================================

    # Pattern: MoveStdlib/Sui auto-bundling confusion [9]
    if "movestdlib" in lower_output and "automatically added" in lower_output:
        enhanced.append(
            "DOCS_HINT: Sui automatically includes Sui, MoveStdlib, Bridge, DeepBook, and SuiSystem. "
            "Do NOT declare them as git dependencies. A minimal Move.toml needs only: "
            '[package]\\nname = "helper_pkg"\\nedition = "2024.beta"\\n\\n[addresses]\\nhelper_pkg = "0x0"'
        )

    # Pattern: Legacy system name error [9]
    if "legacy system name" in lower_output:
        enhanced.append(
            "DOCS_HINT: 'MoveStdlib' and 'Sui' are legacy names that cannot be used as dependencies. "
            "Remove them from [dependencies] entirely - just use 'use std::*' and 'use sui::*' in code."
        )

    # Pattern: Git dependency path issues (common with old Sui repo structure) [9]
    if "crates/sui-framework" in lower_output and ("move.toml" in lower_output or "failed to load" in lower_output):
        enhanced.append(
            "DOCS_HINT: The Sui git repo structure has changed. Don't use subdir paths like "
            "'crates/sui-framework'. Instead, omit Sui/MoveStdlib dependencies entirely - they're auto-included."
        )

    # Pattern: move-language/move repo (wrong repo for Sui) [9]
    if "move-language/move" in lower_output or "github.com/move-language" in lower_output:
        enhanced.append(
            "DOCS_HINT: Don't use github.com/move-language/move for Sui projects. "
            "Sui has its own stdlib bundled. Remove MoveStdlib dependency and just use 'use std::*' in code."
        )

    # Pattern: Unresolved addresses [1]
    if "unresolved addresses" in lower_output:
        enhanced.append(
            "DOCS_HINT: Named addresses must be defined in Move.toml [addresses] section. "
            'Example: [addresses]\\nmy_pkg = "0x0"\\nstd = "0x1"\\nsui = "0x2"'
        )

    # Pattern: Duplicate module/address defined [1]
    if "defined more than once" in lower_output or "duplicate module" in lower_output:
        enhanced.append(
            "DOCS_HINT: Each module must have a unique name+address combination. "
            "Check if you're defining addresses like 'std' or 'sui' that conflict with auto-included ones. "
            "For Sui projects, don't define std/sui addresses - they're provided automatically."
        )

    # Pattern: Edition syntax issues [2]
    if "edition" in lower_output and ("expected" in lower_output or "invalid" in lower_output):
        enhanced.append(
            'DOCS_HINT: Move edition goes directly under [package]: edition = "2024.beta" '
            "(not in a separate [edition] section)."
        )

    # Pattern: Address format issues [1]
    if "invalid address" in lower_output or "expected address" in lower_output:
        enhanced.append(
            'DOCS_HINT: Addresses in Move.toml should be hex strings like "0x0" or the full 64-char address. '
            'For local packages, "0x0" is conventional.'
        )

    # =========================================================================
    # MOVE 2024 EDITION BREAKING CHANGES
    # Sources: [2] Move 2024 Migration, [5] Move Book Migration Guide
    # =========================================================================

    # Pattern: Struct visibility in Move 2024 [2]
    if "visibility annotations are required on struct" in lower_output:
        enhanced.append(
            "DOCS_HINT: Move 2024 edition requires visibility on structs. "
            "Use 'public struct MyStruct has copy, drop { ... }' instead of 'struct MyStruct has copy, drop { ... }'"
        )

    # Pattern: Mutable variable in Move 2024 [2]
    if "to use the variable mutably" in lower_output or "declared 'mut'" in lower_output:
        enhanced.append(
            "DOCS_HINT: Move 2024 edition requires explicit 'mut' for mutable variables. "
            "Use 'let mut v = ...' instead of 'let v = ...' when you need to mutate it."
        )

    # Pattern: friend keyword deprecated [2]
    if "friend" in lower_output and ("deprecated" in lower_output or "public(package)" in lower_output):
        enhanced.append(
            "DOCS_HINT: 'friend' declarations are deprecated in Move 2024. "
            "Use 'public(package)' visibility instead: public(package) fun my_func() { ... }"
        )

    # Pattern: Reserved keywords (enum, for, match, mut, type) [2]
    if any(kw in lower_output for kw in ["reserved keyword", "new keywords"]):
        enhanced.append(
            "DOCS_HINT: Move 2024 reserves new keywords: enum, for, match, mut, type. "
            "If you used these as identifiers, rename them or escape with backticks: `enum`, `type`"
        )

    # =========================================================================
    # COIN / OBJECT API ISSUES
    # Sources: [3] Coin Module API, [7] Coin Standard, [8] Create Coins
    # =========================================================================

    # Pattern: Coin::zero requires TxContext [3]
    if "coin::zero" in lower_output or (
        "coin" in lower_output and "zero" in lower_output and "argument" in lower_output
    ):
        enhanced.append(
            "DOCS_HINT: coin::zero<T>() requires a TxContext: let c = coin::zero<SUI>(ctx); // NOT Coin::zero()"
        )

    # Pattern: Wrong Coin API syntax (Coin::method vs coin::method) [3]
    if "coin::" in lower_output and "unexpected name" in lower_output:
        enhanced.append(
            "DOCS_HINT: Use module::function syntax, not Type::method. "
            "Correct: coin::value(&c), coin::zero<T>(ctx), coin::split(&mut c, amount, ctx). "
            "Or use method syntax: c.value(), c.split(amount, ctx)"
        )

    # Pattern: TreasuryCap / mint issues [7] [8]
    if "treasurycap" in lower_output or ("mint" in lower_output and "cap" in lower_output):
        enhanced.append(
            "DOCS_HINT: Minting requires TreasuryCap: coin::mint<T>(&mut cap, amount, ctx). "
            "TreasuryCap is created via coin::create_currency() in the module's init function."
        )

    # Pattern: Object UID issues [6]
    if "uid" in lower_output and ("new" in lower_output or "object::new" in lower_output):
        enhanced.append(
            "DOCS_HINT: Objects with 'key' ability need a UID field created via object::new(ctx): "
            "public struct MyObj has key { id: UID, ... } then id: object::new(ctx)"
        )

    # Pattern: Invalid object construction (Sui E01001) [6]
    if "invalid object construction" in lower_output or "e01001" in lower_output:
        enhanced.append(
            "DOCS_HINT: Object UIDs must come from sui::object::new(ctx). "
            "You cannot reuse or copy UIDs from other objects."
        )

    # =========================================================================
    # TYPE / ABILITY ISSUES
    # Sources: [4] Move Book, [6] Sui Object Model
    # =========================================================================

    # Pattern: Missing abilities [4]
    if "missing ability" in lower_output or "constraint not satisfied" in lower_output:
        enhanced.append(
            "DOCS_HINT: Struct abilities must match usage. Common patterns: "
            "'has key, store' for transferable objects, 'has copy, drop, store' for simple data, "
            "'has drop' for values that can be discarded."
        )

    # Pattern: Drop ability required [4] [6]
    if "drop" in lower_output and ("cannot" in lower_output or "missing" in lower_output or "unused" in lower_output):
        enhanced.append(
            "DOCS_HINT: Values without 'drop' ability must be explicitly consumed (transferred, stored, or destroyed). "
            "Add 'has drop' to the struct or use transfer::transfer/transfer::public_transfer."
        )

    # Pattern: Zero-sized struct [1]
    if "zero" in lower_output and "sized" in lower_output and "struct" in lower_output:
        enhanced.append(
            "DOCS_HINT: Structs must have at least one field. "
            "Add a dummy field if needed: public struct MyWitness has drop { _dummy: bool }"
        )

    # =========================================================================
    # TRANSFER / OWNERSHIP ISSUES
    # Sources: [6] Sui Object Model
    # =========================================================================

    # Pattern: Transfer issues [6]
    if "transfer" in lower_output and ("public_transfer" in lower_output or "cannot be transferred" in lower_output):
        enhanced.append(
            "DOCS_HINT: Objects with only 'key' use transfer::transfer (module-only). "
            "Objects with 'key, store' can use transfer::public_transfer (anyone can transfer)."
        )

    # Pattern: Unused value without drop [1] [6]
    if "unusedvaluewithoutdrop" in lower_output or "unused value" in lower_output:
        enhanced.append(
            "DOCS_HINT: All values must be consumed by end of function. "
            "Either: transfer them (transfer::public_transfer), store them, destroy them, or add 'drop' ability."
        )

    # =========================================================================
    # COMMON SYNTAX / API MISTAKES
    # Sources: [2] Move 2024 Migration, [4] Move Book
    # =========================================================================

    # Pattern: Method syntax available [2]
    if "vector::push_back" in combined_output or "vector::pop_back" in combined_output:
        enhanced.append(
            "DOCS_HINT: Move 2024 supports method syntax. Instead of vector::push_back(&mut v, x), "
            "you can write v.push_back(x). Same for other stdlib functions."
        )

    # Pattern: Use statement issues [2]
    if "unbound module" in lower_output or "unresolved use" in lower_output:
        enhanced.append(
            "DOCS_HINT: Common Sui imports are auto-available in Move 2024: "
            "vector, option::Option, object::{Self, ID, UID}, transfer, tx_context::TxContext. "
            "For others use: 'use sui::coin::{Self, Coin};' or 'use std::string::String;'"
        )
        # Additional hint for target package imports
        enhanced.append(
            "DOCS_HINT: To import from the target package, use the dependency alias 'target_pkg': "
            "'use target_pkg::module_name::TypeName;' (NOT the raw hex address)."
        )

    # Pattern: TxContext usage [4]
    if "txcontext" in lower_output and ("borrow" in lower_output or "reference" in lower_output):
        enhanced.append(
            "DOCS_HINT: Entry functions take TxContext as last param: "
            "public entry fun my_func(arg1: u64, ctx: &mut TxContext). "
            "Use ctx for object::new(ctx), tx_context::sender(ctx), etc."
        )

    return enhanced


def _extract_build_error_summary(stderr: str, stdout: str = "", *, enable_docs_hints: bool = True) -> list[str]:
    """Extract key error lines from Move compiler output for LLM feedback.

    Note: sui move build outputs some errors to stdout (e.g., dependency errors),
    so we check both streams. Also enhances errors with Sui-specific guidance
    to help LLMs recover from common tooling mistakes.

    Args:
        stderr: Build stderr output
        stdout: Build stdout output
        enable_docs_hints: If True (default), append DOCS_HINT guidance for common errors
    """
    errors: list[str] = []
    # Combine both streams - some errors go to stdout
    combined = stderr + "\n" + stdout
    for line in combined.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        # Skip ANSI escape codes for cleaner output
        clean = stripped
        for code in ["\x1b[0m", "\x1b[1m", "\x1b[34m", "\x1b[31m", "\x1b[33m", "\x1b[38;5;9m", "\x1b[38;5;11m"]:
            clean = clean.replace(code, "")
        # Capture error lines - various formats used by Move/Sui
        if any(x in clean.lower() for x in ["error[e", "error:", "error while", "failed to"]):
            errors.append(clean[:300])  # Truncate very long lines
        # Also capture helpful notes/suggestions
        elif "[note]" in clean.lower():
            errors.append(clean[:300])
        # Also capture the actual error message lines (often follow the error code)
        elif errors and clean and not clean.startswith("─") and not clean.startswith("│"):
            # Likely a continuation or explanation
            if len(errors) < 30:  # Limit total errors
                errors.append(f"  {clean[:300]}")

    # Enhance with actionable Sui-specific guidance (simulates docs lookup)
    if enable_docs_hints:
        errors = _enhance_errors_with_sui_guidance(errors, combined)

    return errors[:50]  # Limit to 50 lines for prompt (increased to accommodate hints)


def _sui_move_build_with_bytecode(helper_dir: Path) -> tuple[bool, str, str]:
    # First run regular build to get error messages (--dump-bytecode-as-base64 suppresses them)
    proc = _run(
        ["sui", "move", "build"],
        cwd=helper_dir,
        timeout_s=300,
    )
    if proc.returncode != 0:
        # Return the error output for LLM to see
        return False, proc.stdout, proc.stderr

    # If regular build succeeded, we're done - bytecode is in build/<pkg>/bytecode_modules
    # No need for --dump-bytecode-as-base64 (it requires network and we read bytecode from disk anyway)
    return True, proc.stdout, proc.stderr


def _sanitize_package_name(pkg_name: str) -> str | None:
    """
    Sanitize package name to prevent path traversal attacks.

    Returns sanitized name or None if name is unsafe.

    P0 Security: Validates package name BEFORE any path operations.
    """
    if not isinstance(pkg_name, str):
        return None
    # Reject empty names
    if not pkg_name or not pkg_name.strip():
        return None
    # Reject names with path separators, parent refs, or null bytes
    if "/" in pkg_name or "\\" in pkg_name or ".." in pkg_name or "\0" in pkg_name:
        return None
    # Reject names that start with dots (hidden files)
    if pkg_name.startswith("."):
        return None
    # Reject names with control characters
    if any(ord(c) < 32 for c in pkg_name):
        return None
    # Limit length to prevent filesystem issues
    if len(pkg_name) > 255:
        return None
    return pkg_name


def _find_built_bytecode_dir(helper_dir: Path) -> Path | None:
    """Find the bytecode_modules directory from sui move build output.

    The build output is at build/<package_name>/bytecode_modules, where package_name
    comes from Move.toml.
    """
    toml_path = helper_dir / "Move.toml"
    if not toml_path.is_file():
        return None

    # Use robust tomllib (standard in Python 3.11+)
    pkg_name = "helper_pkg"  # fallback default
    try:
        import tomllib

        with open(toml_path, "rb") as f:
            toml_data = tomllib.load(f)
            raw_name = toml_data.get("package", {}).get("name", pkg_name)
            # P0 SECURITY: Sanitize BEFORE using in any path operations
            sanitized = _sanitize_package_name(raw_name)
            if sanitized is None:
                return None
            pkg_name = sanitized
    except Exception:
        # If TOML is invalid, we don't attempt to guess with regex
        return None

    bytecode_dir = helper_dir / "build" / pkg_name / "bytecode_modules"
    if bytecode_dir.is_dir():
        return bytecode_dir

    # Final fallback: search subdirectories if the exact name match failed
    build_dir = helper_dir / "build"
    if build_dir.is_dir():
        for pkg_dir in sorted(build_dir.iterdir()):
            if pkg_dir.is_dir():
                cand = pkg_dir / "bytecode_modules"
                if cand.is_dir():
                    return cand
    return None


def _run_local_vm_entry(*, helper_dir: Path, call_target: str, out_path: Path) -> tuple[bool, dict[str, Any]]:
    # Leverage the Rust `benchmark-local` command as the local VM execution harness.
    # It executes entry functions when params are empty and tier-b is enabled.
    # We run it over the helper package bytecode corpus and then filter for the requested target.
    build_bytecode = _find_built_bytecode_dir(helper_dir)
    target_corpus = build_bytecode if build_bytecode else (helper_dir / "bytecode_modules")
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


def _extract_module_address_from_bytecode(mv_path: Path) -> str | None:
    """Extract the module's self-address from compiled Move bytecode.

    The address is embedded in the module's CompiledModule structure.
    We use a simple heuristic: run the Rust CLI to get module info.
    Falls back to a hash of the file path if extraction fails.
    """
    # Use sui move disassemble or our own bytecode-json to extract address.
    # For now, use a simpler approach: hash the file content to detect duplicates.
    # The Rust LocalModuleResolver already extracts the real address from bytecode.
    # We just need unique filenames to avoid copy collisions.
    try:
        import hashlib

        content = mv_path.read_bytes()
        return hashlib.sha256(content).hexdigest()[:8]
    except Exception:
        return None


def _build_combined_corpus_dir(*, run_dir: Path, target_pkg_dir: Path, helper_dir: Path) -> Path:
    """Combine target + helper bytecode modules into a single corpus directory.

    Uses content-based naming to avoid collisions when modules have the same name
    but different addresses (e.g., target::cell vs helper_pkg::cell).
    The Rust LocalModuleResolver extracts the actual address from bytecode, so
    file naming is only for avoiding copy collisions.
    """
    combined = run_dir / "combined_corpus" / "bytecode_modules"
    if combined.parent.exists():
        shutil.rmtree(combined.parent)
    combined.mkdir(parents=True, exist_ok=True)

    # Track which module names we've seen to detect collisions
    seen_names: dict[str, Path] = {}  # module_name -> source_path

    def copy_module(mv: Path, source_label: str) -> None:
        """Copy a module, using address prefix if name collision detected."""
        name = mv.name
        if name in seen_names:
            # Collision detected - rename using content hash prefix
            prefix = _extract_module_address_from_bytecode(mv) or "unk"
            new_name = f"{prefix}_{name}"
            # Also rename the already-copied file if it wasn't prefixed
            existing = seen_names[name]
            existing_dest = combined / name
            if existing_dest.exists():
                existing_prefix = _extract_module_address_from_bytecode(existing) or "unk"
                existing_new_name = f"{existing_prefix}_{name}"
                existing_dest.rename(combined / existing_new_name)
            dest = combined / new_name
        else:
            dest = combined / name
            seen_names[name] = mv
        shutil.copy2(mv, dest)

    # Copy target modules first
    target_bytecode = target_pkg_dir / "bytecode_modules"
    if target_bytecode.exists():
        for mv in sorted(target_bytecode.glob("*.mv")):
            copy_module(mv, "target")

    # Copy helper modules (built)
    build_bytecode = _find_built_bytecode_dir(helper_dir)
    helper_mods = build_bytecode if build_bytecode else (helper_dir / "bytecode_modules")
    if helper_mods.exists():
        for mv in sorted(helper_mods.glob("*.mv")):
            copy_module(mv, "helper")

    return combined


def _mm2_map_helper(*, helper_dir: Path, out_path: Path) -> tuple[bool, dict[str, Any]]:
    # Use the Rust no-chain local benchmark as Tier A/B executor for helper package bytecode.
    # This is the current in-repo path to MM2-style local validation (no RPC, no gas).
    tmp_out = out_path.parent / "mm2_benchmark_local.jsonl"
    # `sui move build` emits bytecode under build/<pkg>/bytecode_modules.
    build_bytecode = _find_built_bytecode_dir(helper_dir)
    target_corpus = build_bytecode if build_bytecode else (helper_dir / "bytecode_modules")
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
    # Note: txsim_source.json is optional when using stub-based compilation
    # Compilation success (tier_a_hit) is sufficient to prove type inhabitation
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
    # Note: txsim (execution) is optional when using stub-based compilation.
    # Compilation success with target_pkg imports is sufficient to prove type inhabitation.
    if txsim is not None:
        status = txsim.get("status")
        if status == "tier_b_hit":
            pass  # Execution succeeded - bonus validation
        # Don't fail if txsim didn't achieve tier_b - stub compilation doesn't support execution

    # Real-corpus evidence: require at least one Tier B hit in the combined mapping.
    # IMPORTANT: The tier_b_hit must be for a TARGET PACKAGE module, not just the helper module.
    # Helper-only tier_b_hits don't demonstrate target package type inhabitation.
    try:
        cmm2 = json.loads((run_dir / "mm2_combined_mapping.json").read_text(encoding="utf-8"))
    except Exception:
        cmm2 = None

    # Get target package ID from run config to identify target vs helper modules
    target_pkg_id = None
    try:
        run_cfg = json.loads((run_dir / "run_config.json").read_text(encoding="utf-8"))
        target_pkg_id = run_cfg.get("package_id")
    except Exception:
        pass

    if isinstance(cmm2, dict):
        acc = cmm2.get("accepted")
        if isinstance(acc, list):
            is_fake = (run_dir / "run_config.json").read_text(encoding="utf-8").find("fake_corpus") != -1
            if not is_fake:
                # Check for tier_b_hit that actually accessed TARGET PACKAGE modules.
                # The execution trace (target_modules_accessed) shows which non-framework
                # modules were loaded during execution. We require at least one target
                # package module to be accessed (not just the helper module).

                # Find tier_b_hits with target package modules accessed
                target_pkg_tier_b_hits = []
                for r in acc:
                    if not isinstance(r, dict):
                        continue
                    if r.get("status") != "tier_b_hit":
                        continue

                    tier_b = r.get("tier_b_details", {})
                    if not tier_b.get("execution_success"):
                        continue

                    # Check target_modules_accessed for non-helper, non-framework modules
                    accessed = tier_b.get("target_modules_accessed", [])
                    if not accessed:
                        continue

                    # Filter out helper module (0x0::*) and framework modules
                    target_accesses = [
                        m
                        for m in accessed
                        if not m.startswith("0x0::")  # helper module
                        and not m.startswith("0x1::")  # move-stdlib
                        and not m.startswith("0x2::")  # sui-framework
                        and not m.startswith("0x3::")  # sui-system
                    ]

                    if target_accesses:
                        target_pkg_tier_b_hits.append(
                            {
                                "function": f"{r.get('target_module')}::{r.get('target_function')}",
                                "target_modules": target_accesses,
                            }
                        )

                if not target_pkg_tier_b_hits:
                    ok = False
                    errors.append(
                        f"no tier_b_hit with target package module access. "
                        f"LLM code must execute and access modules from target package ({target_pkg_id or 'unknown'}), "
                        f"not just framework/stdlib types."
                    )
                # else: Success - we have tier_b execution that accessed target package modules
    return {"ok": ok, "errors": errors}


def main(argv: list[str] | None = None) -> int:
    epilog = """
EXAMPLES:

  # Offline test (no API key needed):
  uv run python scripts/e2e_one_package.py \\
      --corpus-root tests/fake_corpus --package-id 0x1

  # Test 5 packages from top25 dataset with real LLM:
  SMI_E2E_REAL_LLM=1 uv run python scripts/e2e_one_package.py \\
      --corpus-root ../sui-packages/packages/mainnet_most_used \\
      --dataset type_inhabitation_top25 --samples 5 \\
      --model google/gemini-3-flash-preview

  # Debug a specific package with verbose output:
  uv run python scripts/e2e_one_package.py \\
      --corpus-root ../sui-packages/packages/mainnet_most_used \\
      --package-id 0x2 --persist-tmp-dir /tmp/debug
"""
    p = argparse.ArgumentParser(
        description="E2E benchmark: Target Package -> LLM Helper Generation -> Move Build -> TX Simulation",
        epilog=epilog,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument(
        "--corpus-root",
        type=Path,
        required=True,
        help="Path to corpus directory containing package subdirs with bytecode_modules/",
    )
    p.add_argument(
        "--dataset",
        type=str,
        default="type_inhabitation_top25",
        help="Dataset name from manifests/datasets/<name>.txt (default: type_inhabitation_top25)",
    )
    p.add_argument("--samples", type=int, default=1, help="Number of samples per package (default: 1)")
    p.add_argument("--package-id", type=str, default=None, help="Specific package ID to test (overrides --dataset)")
    p.add_argument("--dataset-index", type=int, default=0, help="Start index in dataset (default: 0)")
    p.add_argument(
        "--dataset-count", type=int, default=1, help="Number of packages to process from dataset (default: 1)"
    )
    p.add_argument(
        "--per-package-timeout-seconds", type=int, default=120, help="Timeout per package in seconds (default: 120)"
    )
    p.add_argument(
        "--max-attempts", type=int, default=3, help="Max LLM retry attempts for helper generation (default: 3)"
    )
    p.add_argument("--seed", type=int, default=1, help="Random seed for reproducibility (default: 1)")
    p.add_argument(
        "--model", type=str, default=None, help="LLM model to use (default: from SMI_MODEL env var or gpt-5.2)"
    )
    p.add_argument("--enable-dryrun", action="store_true", help="Enable dry-run mode for transaction simulation")
    p.add_argument(
        "--no-docs-hints",
        action="store_true",
        help="Disable DOCS_HINT enhancements in error feedback (enabled by default)",
    )
    p.add_argument(
        "--out-dir",
        type=Path,
        default=BENCH_ROOT / "results",
        help="Output directory for results (default: benchmark/results)",
    )
    p.add_argument(
        "--persist-tmp-dir",
        type=Path,
        default=None,
        help="If set, copy /tmp benchmark-local artifacts and logs into this directory for debugging.",
    )
    p.add_argument(
        "--prompt-file",
        type=Path,
        default=None,
        help="Path to custom prompt template file (default: uses built-in prompt). "
        "Template variables: {{PACKAGE_ID}}, {{INTERFACE_SUMMARY}}, {{MAX_ATTEMPTS}}, {{MOVE_EDITION}}",
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
        pkg_ids = ids[args.dataset_index : end]

    overall_ok = True
    for pkg_id in pkg_ids:
        started_pkg = time.monotonic()
        target_pkg_dir = _find_package_dir_in_corpus(corpus_root=args.corpus_root, package_id=pkg_id)
        # P1 Fix: Use microsecond precision + pid + random suffix to prevent collisions
        # in high-concurrency test environments and parallel CI runs
        stamp = int(time.time() * 1_000_000)
        random_suffix = secrets.token_hex(4)  # 8 hex chars for extra uniqueness
        run_dir = args.out_dir / f"e2e_{stamp}_{os.getpid()}_{random_suffix}_{pkg_id[:10]}"
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
                "max_attempts": args.max_attempts,
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
        # Summarize target package interface as reference/inspiration for the LLM.
        iface_summary = ""
        iface: dict[str, Any] | None = None  # Will be used to generate source stubs for vendoring
        try:
            from smi_bench.inhabit.executable_subset import summarize_interface
            from smi_bench.rust import default_rust_binary, emit_bytecode_json, validate_rust_binary

            rust_bin = default_rust_binary()
            validate_rust_binary(rust_bin)
            iface = emit_bytecode_json(package_dir=target_pkg_dir, rust_bin=rust_bin)
            iface_summary = summarize_interface(iface, max_functions=30, mode="entry_then_public")
            _atomic_write_json(run_dir / "target_interface.json", iface)
        except Exception as e:
            iface_summary = f"<failed to summarize interface: {e}>"

        # Build the instruction from template file or use built-in default
        max_attempts = args.max_attempts
        move_edition = "2024.beta"

        if args.prompt_file and args.prompt_file.exists():
            # Load custom prompt template
            template = args.prompt_file.read_text(encoding="utf-8")
            # Strip comment lines (start with #)
            template_lines = [ln for ln in template.split("\n") if not ln.strip().startswith("#")]
            template = "\n".join(template_lines).strip()
            # Replace template variables
            instruction = (
                template
                .replace("{{PACKAGE_ID}}", pkg_id)
                .replace("{{INTERFACE_SUMMARY}}", iface_summary)
                .replace("{{MAX_ATTEMPTS}}", str(max_attempts))
                .replace("{{MOVE_EDITION}}", move_edition)
            )
        else:
            # Built-in default prompt
            instruction = (
                "GOAL: Create a Move helper package that demonstrates TYPE INHABITATION of the TARGET PACKAGE.\n"
                "Your entry functions MUST create or use types defined in the target package (shown below).\n"
                "Using only stdlib types (vector, option, u64, etc.) does NOT count as success.\n"
                "\n"
                "MOVE.TOML SETUP:\n"
                "- Sui, MoveStdlib, SuiSystem are AUTO-INCLUDED. Do NOT add them.\n"
                "- The target package is pre-configured as dependency 'target_pkg'.\n"
                "- A minimal Move.toml only needs: [package], name, edition, and [addresses]\n"
                "\n"
                f"TARGET PACKAGE TO INHABIT (package_id: {pkg_id}):\n"
                f"{iface_summary}"
                "\n\n"
                "CONSTRAINTS:\n"
                f"- You have {max_attempts} attempts. Build errors will be provided after each attempt.\n"
                "- Your code will be compiled with `sui move build`\n"
                "- Entry functions must be zero-arg (only implicit TxContext allowed)\n"
                f"- Use Move edition {move_edition}\n"
                "- You MUST use at least one type from the target package - stdlib-only code will fail evaluation\n"
                "\n"
                "OUTPUT FORMAT: JSON object with these fields:\n"
                "- move_toml: STRING containing full Move.toml contents\n"
                "- files: OBJECT mapping relative paths to file contents (e.g. 'sources/helper.move': '...')\n"
                "- entrypoints: ARRAY of objects with 'target' field (e.g. 'helper_pkg::helper::my_func')\n"
                "- assumptions: ARRAY of strings explaining your approach"
            )

        prompt = {
            "schema": "helper_pkg_v1",
            "package_id": pkg_id,
            "seed": cfg.seed,
            "reference_interface": iface_summary,
            "constraints": {
                "max_attempts": max_attempts,
                "move_edition": move_edition,
            },
            "instruction": instruction,
        }
        _atomic_write_json(run_dir / "llm_request.json", prompt)

        if timed_out():
            _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["package timeout"]})
            overall_ok = False
            continue

        use_real = os.environ.get("SMI_E2E_REAL_LLM") == "1"
        helper_payload: dict[str, Any] | None = None
        last_errors: list[str] = []
        # P1 Fix: Initialize helper_dir early to avoid unbound variable if validation fails
        helper_dir = run_dir / "helper_pkg"

        # Track context across attempts for progressive disclosure
        last_valid_response: dict[str, Any] | None = None  # Last response that passed JSON validation
        attempt_history: list[str] = []  # Brief summary of each attempt for LLM context

        if use_real:
            for attempt in range(1, max_attempts + 1):
                if timed_out():
                    _atomic_write_json(run_dir / "validation_report.json", {"ok": False, "errors": ["package timeout"]})
                    overall_ok = False
                    break
                try:
                    raw = _call_real_llm_with_repair(model=cfg.model, prompt_obj=prompt, seed=cfg.seed + attempt)
                except Exception as e:
                    last_errors = [f"llm request failed: {e}"]
                    attempt_history.append(f"Attempt {attempt}: LLM request failed - {e}")
                    prompt = dict(prompt)
                    if last_valid_response:
                        prompt["your_previous_response"] = last_valid_response
                    if attempt_history:
                        prompt["attempt_history"] = attempt_history[-5:]  # Last 5 attempts
                    prompt["repair"] = "Return ONLY valid helper_pkg_v1 JSON. Prior request failed: " + str(e)
                    continue

                _atomic_write_json(run_dir / f"llm_response_attempt_{attempt}.json", raw)
                raw = _coerce_model_output_to_helper_payload(raw)
                try:
                    helper_payload = _validate_llm_helper_payload(raw)
                    # Save as last valid response for future attempts
                    last_valid_response = {
                        "move_toml": helper_payload.get("move_toml", ""),
                        "files": helper_payload.get("files", {}),
                    }
                    # NOTE: We no longer enforce that entrypoints match specific targets.
                    # The LLM is free to choose how to inhabit types - we only validate
                    # that the helper package is structurally valid.
                except Exception as e:
                    last_errors = [f"llm payload invalid: {e}"]
                    attempt_history.append(f"Attempt {attempt}: JSON validation failed - {e}")
                    prompt = dict(prompt)
                    if last_valid_response:
                        prompt["your_previous_response"] = last_valid_response
                    if attempt_history:
                        prompt["attempt_history"] = attempt_history[-5:]
                    prompt["repair"] = "Return ONLY valid helper_pkg_v1 JSON. Validation error: " + str(e)
                    continue

                # Attempt build inside retry loop so build errors can be fed back to LLM
                helper_dir = run_dir / "helper_pkg"
                _write_helper_package(helper_dir=helper_dir, payload=helper_payload)
                # Vendor target package with source stubs so LLM can import types via `use target_pkg::module::Type`
                _vendor_target_deps_into_helper(target_pkg_dir=target_pkg_dir, helper_dir=helper_dir, interface=iface)

                ok_build, out_s, err_s = _sui_move_build_with_bytecode(helper_dir)
                _atomic_write_text(run_dir / f"helper_build_stdout_attempt_{attempt}.log", out_s)
                _atomic_write_text(run_dir / f"helper_build_stderr_attempt_{attempt}.log", err_s)

                if not ok_build:
                    # Extract key error lines from build output for LLM feedback
                    # Note: Some errors go to stdout (e.g., dependency errors), so we check both
                    enable_docs_hints = not args.no_docs_hints
                    error_lines = _extract_build_error_summary(err_s, out_s, enable_docs_hints=enable_docs_hints)
                    last_errors = ["build failed", *error_lines]
                    _atomic_write_json(
                        run_dir / f"build_errors_attempt_{attempt}.json",
                        {"errors": error_lines, "stderr": err_s, "stdout": out_s},
                    )

                    # Summarize what failed for history
                    error_summary = error_lines[0] if error_lines else "build failed"
                    attempt_history.append(f"Attempt {attempt}: Build failed - {error_summary[:100]}")

                    prompt = dict(prompt)
                    # Include previous response so LLM knows what it generated and can fix incrementally
                    prompt["your_previous_response"] = last_valid_response
                    if attempt_history:
                        prompt["attempt_history"] = attempt_history[-5:]  # Last 5 attempts for context
                    prompt["repair"] = (
                        "The Move code failed to compile. Fix these build errors, "
                        "then return ONLY helper_pkg_v1 JSON.\n"
                        "IMPORTANT: Keep what worked (e.g., if Move.toml compiled, "
                        "don't change it). Only fix the specific errors.\n\n"
                        "BUILD ERRORS:\n" + "\n".join(error_lines[:40])
                    )
                    helper_payload = None
                    continue

                # Build succeeded - write final logs and break
                _atomic_write_text(run_dir / "helper_build_stdout.log", out_s)
                _atomic_write_text(run_dir / "helper_build_stderr.log", err_s)
                break
        else:
            raw = _stub_llm_helper_payload()
            _atomic_write_json(run_dir / "llm_response.json", raw)
            raw = _coerce_model_output_to_helper_payload(raw)
            try:
                helper_payload = _validate_llm_helper_payload(raw)
            except Exception as e:
                last_errors = [f"llm payload invalid: {e}"]

            # For stub/offline mode, still need to write and build helper
            if helper_payload is not None:
                helper_dir = run_dir / "helper_pkg"
                _write_helper_package(helper_dir=helper_dir, payload=helper_payload)
                # Vendor target package with source stubs so LLM can import types via `use target_pkg::module::Type`
                _vendor_target_deps_into_helper(target_pkg_dir=target_pkg_dir, helper_dir=helper_dir, interface=iface)
                ok_build, out_s, err_s = _sui_move_build_with_bytecode(helper_dir)
                _atomic_write_text(run_dir / "helper_build_stdout.log", out_s)
                _atomic_write_text(run_dir / "helper_build_stderr.log", err_s)
                if not ok_build:
                    last_errors = ["build failed (stub mode)"]
                    helper_payload = None

        if helper_payload is None:
            _atomic_write_json(
                run_dir / "validation_report.json", {"ok": False, "errors": last_errors or ["llm payload invalid"]}
            )
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
        _atomic_write_json(
            run_dir / "mm2_summary.json",
            {"accepted": len(mm2.get("accepted", [])), "rejected": len(mm2.get("rejected", []))},
        )

        ok_tmm2, tmm2 = _mm2_map_target(target_pkg_dir=target_pkg_dir, out_path=run_dir / "mm2_target_mapping.json")
        if not ok_tmm2:
            _atomic_write_json(
                run_dir / "validation_report.json", {"ok": False, "errors": ["target mm2 mapping failed"]}
            )
            _persist_tmp_tree(run_dir=run_dir, tmp_root=tmp_root)
            overall_ok = False
            continue
        _atomic_write_json(
            run_dir / "mm2_target_summary.json",
            {"accepted": len(tmm2.get("accepted", [])), "rejected": len(tmm2.get("rejected", []))},
        )

        combined_corpus_dir = _build_combined_corpus_dir(
            run_dir=run_dir, target_pkg_dir=target_pkg_dir, helper_dir=helper_dir
        )
        ok_cmm2, cmm2 = _mm2_map_combined(
            combined_corpus_dir=combined_corpus_dir, out_path=run_dir / "mm2_combined_mapping.json"
        )
        if not ok_cmm2:
            _atomic_write_json(
                run_dir / "validation_report.json", {"ok": False, "errors": ["combined mm2 mapping failed"]}
            )
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
                if (
                    isinstance(tmm2.get("target"), str)
                    and tmm2.get("target") == "target_pkg"
                    and tgt.startswith(pkg_id + "::")
                ):
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
