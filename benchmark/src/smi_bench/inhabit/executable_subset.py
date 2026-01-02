from __future__ import annotations

from dataclasses import dataclass, field

DUMMY_ADDRESS = "0x" + ("1" * 64)
STDLIB_ADDRESS = "0x" + ("0" * 63) + "1"
SUI_FRAMEWORK_ADDRESS = "0x" + ("0" * 63) + "2"
SUI_SYSTEM_STATE_OBJECT_ID = "0x5"
SUI_CLOCK_OBJECT_ID = "0x6"
SUI_AUTHENTICATOR_STATE_OBJECT_ID = "0x7"
SUI_RANDOM_OBJECT_ID = "0x8"
SUI_DENY_LIST_OBJECT_ID = "0x403"
SUI_COIN_REGISTRY_OBJECT_ID = "0xc"
SUI_MODULE = "sui"
SUI_STRUCT = "SUI"
COIN_MODULE = "coin"
COIN_STRUCT = "Coin"
STD_ASCII_MODULE = "ascii"
STD_STRING_MODULE = "string"
STD_OPTION_MODULE = "option"
SUI_URL_MODULE = "url"


class ExclusionReason:
    NOT_PUBLIC_ENTRY = "not_public_entry"
    HAS_TYPE_PARAMS = "has_type_params"
    UNSUPPORTED_PARAM_TYPE = "unsupported_param_type"
    NO_CANDIDATES = "no_candidates"
    INTERFACE_INVALID = "interface_missing_or_invalid"


@dataclass(frozen=True)
class SelectStats:
    packages_total: int = 0
    packages_selected: int = 0
    packages_failed_interface: int = 0
    packages_no_candidates: int = 0
    candidate_functions_total: int = 0
    rejection_reasons_counts: dict[str, int] = field(default_factory=dict)


@dataclass(frozen=True)
class PackageViability:
    public_entry_total: int
    public_entry_no_type_params_total: int
    public_entry_no_type_params_supported_args_total: int


@dataclass
class FunctionAnalysis:
    is_runnable: bool
    reasons: list[str]
    ptb_calls: list[dict] = field(default_factory=list)
    final_args: list[dict] = field(default_factory=list)
    ptb_type_args: list[str] = field(default_factory=list)


@dataclass
class PackageAnalysis:
    package_id: str
    candidates_ok: list[list[dict]]  # list of PTB call sequences
    candidates_rejected: list[dict]  # list of {"target": "...", "reasons": [...]}
    reasons_summary: dict[str, int]


def build_constructor_index(modules: dict) -> dict[str, list[str]]:
    """
    Build an index of public functions that return a specific struct type.
    Map: "0xADDR::mod::Struct" -> ["0xADDR::mod::func", ...]
    """
    index: dict[str, list[str]] = {}

    for module_name, mod in modules.items():
        if not isinstance(mod, dict):
            continue
        funs = mod.get("functions")
        if not isinstance(funs, dict):
            continue

        # We also need the package address to construct full types
        # Usually modules in interface_json don't store self-address explicitly in 'modules' keys,
        # but the struct types inside do.
        # Or we use the package_id from the outer scope?
        # Actually, `mod` usually has "address" field in bytecode json.
        mod_addr = mod.get("address", DUMMY_ADDRESS)

        for fun_name, f in funs.items():
            if not isinstance(f, dict):
                continue
            if f.get("visibility") != "public":
                continue

            returns = f.get("returns")
            if not isinstance(returns, list) or len(returns) != 1:
                continue

            ret_type = returns[0]
            if not isinstance(ret_type, dict) or ret_type.get("kind") != "datatype":
                continue

            type_str = json_type_to_string(ret_type)
            # Only index if it's a struct defined in THIS package (heuristic)
            # or broadly any struct.
            # Let's index any struct return.

            target = f"{mod_addr}::{module_name}::{fun_name}"
            if type_str not in index:
                index[type_str] = []
            index[type_str].append(target)

    return index


def _is_tx_context_ref_param(t: dict) -> bool:
    """
    True if `t` is `&TxContext` or `&mut TxContext`.

    Canonical Type (from docs/SCHEMA.md):

      {"kind":"ref","mutable":bool,"to":{"kind":"datatype","address":"0x..","module":"tx_context","name":"TxContext",...}}
    """
    if not isinstance(t, dict):
        return False
    if t.get("kind") != "ref":
        return False
    to = t.get("to")
    if not isinstance(to, dict) or to.get("kind") != "datatype":
        return False
    if to.get("module") != "tx_context" or to.get("name") != "TxContext":
        return False
    addr = to.get("address")
    if not isinstance(addr, str):
        return False
    addr = addr.lower()
    return addr.startswith("0x") and len(addr) == 66 and addr.endswith("02")


def _is_sui_type(t: dict) -> bool:
    return (
        isinstance(t, dict)
        and t.get("kind") == "datatype"
        and t.get("address") == SUI_FRAMEWORK_ADDRESS
        and t.get("module") == SUI_MODULE
        and t.get("name") == SUI_STRUCT
        and t.get("type_args") == []
    )


def _is_coin_sui_type(t: dict) -> bool:
    return (
        isinstance(t, dict)
        and t.get("kind") == "datatype"
        and t.get("address") == SUI_FRAMEWORK_ADDRESS
        and t.get("module") == COIN_MODULE
        and t.get("name") == COIN_STRUCT
        and isinstance(t.get("type_args"), list)
        and len(t.get("type_args")) == 1
        and _is_sui_type(t.get("type_args")[0])
    )


def strip_implicit_tx_context_params(params: list[dict]) -> list[dict]:
    if not params:
        return []
    last = params[-1]
    if _is_tx_context_ref_param(last):
        return list(params[:-1])
    return list(params)


def json_type_to_string(t: dict) -> str:
    """
    Reconstruct a Move type string (e.g., '0x2::sui::SUI') from canonical JSON.
    """
    kind = t.get("kind")
    if kind == "bool":
        return "bool"
    if kind == "u8":
        return "u8"
    if kind == "u16":
        return "u16"
    if kind == "u32":
        return "u32"
    if kind == "u64":
        return "u64"
    if kind == "u128":
        return "u128"
    if kind == "u256":
        return "u256"
    if kind == "address":
        return "address"
    if kind == "signer":
        return "signer"
    if kind == "vector":
        inner = t.get("type")
        return f"vector<{json_type_to_string(inner)}>" if isinstance(inner, dict) else "vector"
    if kind == "datatype":
        addr = t.get("address", "0x0")
        mod = t.get("module", "?")
        name = t.get("name", "?")
        args = t.get("type_args", [])
        base = f"{addr}::{mod}::{name}"
        if args and isinstance(args, list):
            arg_strs = [json_type_to_string(a) for a in args if isinstance(a, dict)]
            return f"{base}<{', '.join(arg_strs)}>"
        return base
    if kind == "ref":
        to = t.get("to")
        mut = "&mut " if t.get("mutable") else "&"
        return f"{mut}{json_type_to_string(to)}" if isinstance(to, dict) else f"{mut}unknown"
    return "unknown"


def summarize_interface(interface_json: dict, max_functions: int = 200) -> str:
    """
    Generate a concise human-readable summary of public/entry functions.
    """
    modules = interface_json.get("modules")
    if not isinstance(modules, dict):
        return "No modules found."

    pkg_id = interface_json.get("package_id", "0x0")
    lines = []
    lines.append(f"Package: {pkg_id}")
    lines.append("")

    total_funs_added = 0

    # Sort modules to be deterministic
    for module_name in sorted(modules.keys()):
        if total_funs_added >= max_functions:
            break
            
        mod = modules.get(module_name)
        if not isinstance(mod, dict):
            continue
        funs = mod.get("functions")
        if not isinstance(funs, dict):
            continue
        
        module_lines = []
        # Prioritize entry functions, then public functions
        sorted_funs = sorted(
            funs.items(), 
            key=lambda x: (not x[1].get("is_entry", False), x[0])
        )
        
        for fun_name, f in sorted_funs:
            if total_funs_added >= max_functions:
                break
            if f.get("visibility") != "public" and f.get("is_entry") is not True:
                continue
            
            # Signature construction
            vis = f.get("visibility", "private")
            entry = " entry" if f.get("is_entry") else ""
            params = f.get("params", [])
            param_strs = [json_type_to_string(p) for p in params if isinstance(p, dict)]
            sig = f"{vis}{entry} fun {fun_name}({', '.join(param_strs)})"
            
            type_params = f.get("type_params", [])
            if type_params:
                sig = f"{vis}{entry} fun {fun_name}<{len(type_params)} type params>({', '.join(param_strs)})"
            
            module_lines.append(f"  - {sig}")
            total_funs_added += 1
        
        if module_lines:
            lines.append(f"Module: {module_name}")
            lines.extend(module_lines)
            lines.append("")

    if total_funs_added >= max_functions:
        lines.append(f"... (truncated after {max_functions} functions)")

    return "\n".join(lines)


def construct_arg(
    t: dict,
    next_result_idx: int,
    constructor_index: dict[str, list[str]] | None = None,
    modules_map: dict | None = None,
    recursion_depth: int = 0,
) -> tuple[list[dict], dict] | None:
    """
    Generate valid PTB args for a type, potentially using setup calls.
    Returns (setup_calls, arg_value) or None if unsupported.
    """
    if recursion_depth > 3:
        return None

    # Try pure/object args first (legacy logic)
    pure = type_to_default_ptb_arg(t)
    if pure is not None:
        return [], pure

    # Try construction logic
    kind = t.get("kind")

    if kind == "ref":
        # If we can construct the inner type, we can pass it (VM allows borrow of result).
        inner = t.get("to")
        if isinstance(inner, dict):
            return construct_arg(inner, next_result_idx, constructor_index, modules_map, recursion_depth)

    if kind == "datatype":
        addr = t.get("address")
        mod = t.get("module")
        name = t.get("name")
        type_str = json_type_to_string(t)

        # 0x1::string::String
        if addr == STDLIB_ADDRESS and mod == STD_STRING_MODULE and name == "String":
            # call 0x1::string::utf8(b"sui") -> Result
            return [
                {
                    "target": f"{STDLIB_ADDRESS}::string::utf8",
                    "type_args": [],
                    "args": [{"vector_u8_utf8": "sui"}],
                }
            ], {"result": next_result_idx}

        # 0x1::ascii::String
        if addr == STDLIB_ADDRESS and mod == STD_ASCII_MODULE and name == "String":
            # call 0x1::ascii::string(b"sui") -> Result
            return [
                {
                    "target": f"{STDLIB_ADDRESS}::ascii::string",
                    "type_args": [],
                    "args": [{"vector_u8_utf8": "sui"}],
                }
            ], {"result": next_result_idx}

        # 0x2::url::Url
        if addr == SUI_FRAMEWORK_ADDRESS and mod == SUI_URL_MODULE and name == "Url":
            return [
                {
                    "target": (f"{SUI_FRAMEWORK_ADDRESS}::{SUI_URL_MODULE}::new_unsafe_from_bytes"),
                    "type_args": [],
                    "args": [{"vector_u8_utf8": "https://sui.io"}],
                }
            ], {"result": next_result_idx}

        # 0x1::option::Option<T>
        if addr == STDLIB_ADDRESS and mod == STD_OPTION_MODULE and name == "Option":
            # call 0x1::option::none<T>() -> Result
            type_args = t.get("type_args", [])
            if not isinstance(type_args, list) or len(type_args) != 1:
                return None
            inner_type_str = json_type_to_string(type_args[0])
            if type_args[0].get("kind") == "type_param":
                inner_type_str = f"{SUI_FRAMEWORK_ADDRESS}::{SUI_MODULE}::{SUI_STRUCT}"

            return [
                {
                    "target": f"{STDLIB_ADDRESS}::option::none",
                    "type_args": [inner_type_str],
                    "args": [],
                }
            ], {"result": next_result_idx}

        # Constructor Discovery
        if constructor_index and modules_map:
            # Look up potential constructors
            constructors = constructor_index.get(type_str, [])
            for c_target in constructors:
                # Parse target to find module/function def
                # Expected: 0xADDR::mod::func
                parts = c_target.split("::")
                if len(parts) != 3:
                    continue
                # module name is parts[1], function is parts[2]
                c_mod_name = parts[1]
                c_fun_name = parts[2]

                c_mod = modules_map.get(c_mod_name)
                if not c_mod:
                    continue
                c_f = c_mod.get("functions", {}).get(c_fun_name)
                if not c_f:
                    continue

                # Recursively analyze this constructor
                # We need to pass recursion_depth + 1
                # And we need to accumulate setup calls

                # Check constraints: no type params (or fillable), supported args
                # We can reuse analyze_function?
                # analyze_function returns FunctionAnalysis.
                # But analyze_function currently builds `ptb_calls` for *that* function.
                # We need to chain it.

                # We can call analyze_function recursively?
                # But analyze_function calls construct_arg.
                # We need to pass the context.

                # Refactor: construct_arg needs to call analyze_function logic for the constructor.
                # But avoiding circular import or complex dependency.
                # I'll implement simple checks here.

                if c_f.get("type_params"):
                    # Skip generic constructors for now (unless we fill them?)
                    # Let's skip to keep it simple.
                    continue

                c_params = c_f.get("params", [])
                c_params = strip_implicit_tx_context_params(c_params)

                c_setup_calls = []
                c_final_args = []
                c_ok = True

                # Current offset for result index in the constructor chain
                # We start with next_result_idx.
                # The constructor's setup calls will consume indices starting at next_result_idx.
                # The constructor *itself* will be at index (next_result_idx + len(c_setup_calls)).
                # BUT wait, the recursive calls will return setup_calls.

                current_idx = next_result_idx

                for p in c_params:
                    # RECURSE
                    res = construct_arg(p, current_idx, constructor_index, modules_map, recursion_depth + 1)
                    if res is None:
                        c_ok = False
                        break

                    sub_setup, sub_arg = res
                    c_setup_calls.extend(sub_setup)
                    c_final_args.append(sub_arg)
                    current_idx += len(sub_setup)

                if c_ok:
                    # Found a valid constructor!
                    # Add the constructor call itself
                    constructor_call = {
                        "target": c_target,
                        "type_args": [],  # assumed empty
                        "args": c_final_args,
                    }
                    c_setup_calls.append(constructor_call)

                    # The result of this constructor is the last call in c_setup_calls.
                    # Index = next_result_idx + len(c_setup_calls) - 1
                    res_idx = next_result_idx + len(c_setup_calls) - 1

                    return c_setup_calls, {"result": res_idx}

        # Fallback: Emit a placeholder for any other struct.
        return [], {"$smi_placeholder": type_str}

    return None


def type_to_default_ptb_arg(t: dict) -> dict | None:
    """
    Convert a canonical `Type` (docs/SCHEMA.md) into a default PTB arg spec
    supported by `smi_tx_sim` (Rust).
    """
    kind = t.get("kind") if isinstance(t, dict) else None
    if kind == "ref":
        to = t.get("to")
        if isinstance(to, dict) and to.get("kind") == "datatype":
            addr = to.get("address")
            mod = to.get("module")
            name = to.get("name")
            if (
                addr == SUI_FRAMEWORK_ADDRESS
                and mod == "clock"
                and name == "Clock"
                and isinstance(t.get("mutable"), bool)
            ):
                # Clock is a shared system object at 0x6.
                return {"shared_object": {"id": SUI_CLOCK_OBJECT_ID, "mutable": bool(t["mutable"])}}

            if (
                addr == SUI_FRAMEWORK_ADDRESS
                and mod == "random"
                and name == "Random"
                and isinstance(t.get("mutable"), bool)
            ):
                # Random is a shared system object at 0x8.
                return {"shared_object": {"id": SUI_RANDOM_OBJECT_ID, "mutable": bool(t["mutable"])}}

            if (
                addr == SUI_FRAMEWORK_ADDRESS
                and mod == "deny_list"
                and name == "DenyList"
                and isinstance(t.get("mutable"), bool)
            ):
                # DenyList is a shared system object at 0x403.
                return {"shared_object": {"id": SUI_DENY_LIST_OBJECT_ID, "mutable": bool(t["mutable"])}}

            if _is_coin_sui_type(to):
                # Coin<SUI> is owned by sender; resolve via RPC selection.
                return {"sender_sui_coin": {"index": 0, "exclude_gas": True}}
        return None
    if kind == "datatype" and _is_coin_sui_type(t):
        return {"sender_sui_coin": {"index": 0, "exclude_gas": True}}
    if kind == "bool":
        return {"bool": False}
    if kind == "u8":
        return {"u8": 1}
    if kind == "u16":
        return {"u16": 1}
    if kind == "u32":
        return {"u32": 1}
    if kind == "u64":
        return {"u64": 1}
    if kind == "address":
        return {"address": DUMMY_ADDRESS}
    if kind == "vector":
        inner = t.get("type")
        if isinstance(inner, dict) and inner.get("kind") == "u8":
            return {"vector_u8_hex": "0x01"}
        if isinstance(inner, dict) and inner.get("kind") == "bool":
            return {"vector_bool": [False]}
        if isinstance(inner, dict) and inner.get("kind") == "u16":
            return {"vector_u16": [1]}
        if isinstance(inner, dict) and inner.get("kind") == "u32":
            return {"vector_u32": [1]}
        if isinstance(inner, dict) and inner.get("kind") == "u64":
            return {"vector_u64": [1]}
        if isinstance(inner, dict) and inner.get("kind") == "address":
            return {"vector_address": [DUMMY_ADDRESS]}
        return None
    return None


def analyze_function(
    f: dict, constructor_index: dict[str, list[str]] | None = None, modules_map: dict | None = None
) -> FunctionAnalysis:
    reasons = []

    if f.get("visibility") != "public" or f.get("is_entry") is not True:
        reasons.append(ExclusionReason.NOT_PUBLIC_ENTRY)

    type_params = f.get("type_params")
    ptb_type_args: list[str] = []
    if isinstance(type_params, list) and type_params:
        # Heuristic: fill all type params with 0x2::sui::SUI
        for _ in type_params:
            ptb_type_args.append(f"{SUI_FRAMEWORK_ADDRESS}::{SUI_MODULE}::{SUI_STRUCT}")

    params = f.get("params")
    ptb_calls = []
    call_args = []

    if isinstance(params, list):
        params = strip_implicit_tx_context_params([p for p in params if isinstance(p, dict)])
        for p in params:
            # We pass index/map context here
            res = construct_arg(p, len(ptb_calls), constructor_index, modules_map, 0)
            if res is None:
                reasons.append(ExclusionReason.UNSUPPORTED_PARAM_TYPE)
                break

            setup, arg_val = res
            ptb_calls.extend(setup)
            call_args.append(arg_val)

    if reasons:
        return FunctionAnalysis(is_runnable=False, reasons=reasons)

    return FunctionAnalysis(
        is_runnable=True, reasons=[], ptb_calls=ptb_calls, final_args=call_args, ptb_type_args=ptb_type_args
    )


def analyze_package(interface_json: dict) -> PackageAnalysis:
    pkg_id = interface_json.get("package_id")
    if not isinstance(pkg_id, str) or not pkg_id:
        pkg_id = "0x0"

    modules = interface_json.get("modules")
    if not isinstance(modules, dict):
        return PackageAnalysis(
            package_id=pkg_id,
            candidates_ok=[],
            candidates_rejected=[],
            reasons_summary={ExclusionReason.INTERFACE_INVALID: 1},
        )

    candidates_ok = []
    candidates_rejected = []
    reasons_summary = {}

    # Build index
    constructor_index = build_constructor_index(modules)

    for module_name in sorted(modules.keys()):
        mod = modules.get(module_name)
        if not isinstance(mod, dict):
            continue
        funs = mod.get("functions")
        if not isinstance(funs, dict):
            continue
        for fun_name in sorted(funs.keys()):
            f = funs.get(fun_name)
            if not isinstance(f, dict):
                continue

            target = f"{pkg_id}::{module_name}::{fun_name}"
            # Pass context
            analysis = analyze_function(f, constructor_index, modules)

            if analysis.is_runnable:
                # Construct the full call sequence
                calls = list(analysis.ptb_calls)
                calls.append({"target": target, "type_args": analysis.ptb_type_args, "args": analysis.final_args})
                candidates_ok.append(calls)
            else:
                candidates_rejected.append({"target": target, "reasons": analysis.reasons})
                for r in analysis.reasons:
                    reasons_summary[r] = reasons_summary.get(r, 0) + 1

    if not candidates_ok and not candidates_rejected:
        # Empty package or no functions?
        # Counts as no candidates if it had modules but no functions analysis returned anything?
        # If it had modules, we traversed them.
        pass

    if not candidates_ok:
        reasons_summary[ExclusionReason.NO_CANDIDATES] = 1

    return PackageAnalysis(
        package_id=pkg_id,
        candidates_ok=candidates_ok,
        candidates_rejected=candidates_rejected,
        reasons_summary=reasons_summary,
    )


def compute_package_viability(interface_json: dict) -> PackageViability:
    """
    Compute conservative viability counts for Phase II executable-subset selection.

    This is intentionally "planfile-only" viability: public entry functions with no type params
    and only supported pure args (after stripping trailing TxContext ref).
    """
    modules = interface_json.get("modules")
    if not isinstance(modules, dict):
        return PackageViability(
            public_entry_total=0,
            public_entry_no_type_params_total=0,
            public_entry_no_type_params_supported_args_total=0,
        )

    public_entry_total = 0
    public_entry_no_type_params_total = 0
    public_entry_supported_total = 0

    for module_name in modules.keys():
        mod = modules.get(module_name)
        if not isinstance(mod, dict):
            continue
        funs = mod.get("functions")
        if not isinstance(funs, dict):
            continue
        for fun_name in funs.keys():
            f = funs.get(fun_name)
            if not isinstance(f, dict):
                continue
            if f.get("visibility") != "public" or f.get("is_entry") is not True:
                continue
            public_entry_total += 1

            type_params = f.get("type_params")
            if isinstance(type_params, list) and type_params:
                continue
            public_entry_no_type_params_total += 1

            params = f.get("params")
            if not isinstance(params, list):
                continue
            params = strip_implicit_tx_context_params([p for p in params if isinstance(p, dict)])
            ok = True
            for p in params:
                if type_to_default_ptb_arg(p) is None:
                    ok = False
                    break
            if ok:
                public_entry_supported_total += 1

    return PackageViability(
        public_entry_total=public_entry_total,
        public_entry_no_type_params_total=public_entry_no_type_params_total,
        public_entry_no_type_params_supported_args_total=public_entry_supported_total,
    )


def select_executable_ptb_spec(
    *,
    interface_json: dict,
    max_calls_per_package: int = 1,
) -> tuple[dict | None, list[dict]]:
    """
    Select a deterministic "executable subset" PTB spec from a package interface.

    Current policy (intentionally conservative):
    - `public entry` functions only
    - no type parameters
    - only "pure" arg types supported by `smi_tx_sim`
    - implicit trailing `&mut TxContext` is stripped

    Returns:
    - `ptb_spec` (or None if no candidates)
    - `selected_calls` (for reporting/debug)
    """
    pkg_id = interface_json.get("package_id")
    if not isinstance(pkg_id, str) or not pkg_id:
        pkg_id = "0x0"

    modules = interface_json.get("modules")
    if not isinstance(modules, dict):
        return None, []

    calls: list[dict] = []
    for module_name in sorted(modules.keys()):
        mod = modules.get(module_name)
        if not isinstance(mod, dict):
            continue
        funs = mod.get("functions")
        if not isinstance(funs, dict):
            continue
        for fun_name in sorted(funs.keys()):
            f = funs.get(fun_name)
            if not isinstance(f, dict):
                continue
            if f.get("visibility") != "public" or f.get("is_entry") is not True:
                continue
            type_params = f.get("type_params")
            if isinstance(type_params, list) and type_params:
                continue
            params = f.get("params")
            if not isinstance(params, list):
                continue
            params = strip_implicit_tx_context_params([p for p in params if isinstance(p, dict)])
            args: list[dict] = []
            ok = True
            for p in params:
                arg = type_to_default_ptb_arg(p)
                if arg is None:
                    ok = False
                    break
                args.append(arg)
            if not ok:
                continue
            calls.append({"target": f"{pkg_id}::{module_name}::{fun_name}", "type_args": [], "args": args})
            if len(calls) >= max_calls_per_package:
                return {"calls": calls}, calls

    if not calls:
        return None, []
    return {"calls": calls}, calls
