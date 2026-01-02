from __future__ import annotations

from smi_bench.inhabit.executable_subset import (
    COIN_MODULE,
    COIN_STRUCT,
    DUMMY_ADDRESS,
    SUI_CLOCK_OBJECT_ID,
    SUI_FRAMEWORK_ADDRESS,
    SUI_MODULE,
    SUI_STRUCT,
    analyze_package,
    select_executable_ptb_spec,
    strip_implicit_tx_context_params,
    type_to_default_ptb_arg,
)


def test_strip_implicit_tx_context_params_drops_trailing_ref() -> None:
    tx_ctx = {
        "kind": "ref",
        "mutable": True,
        "to": {
            "kind": "datatype",
            "address": "0x" + ("0" * 62) + "02",
            "module": "tx_context",
            "name": "TxContext",
            "type_args": [],
        },
    }
    params = [{"kind": "u64"}, tx_ctx]
    assert strip_implicit_tx_context_params(params) == [{"kind": "u64"}]


def test_type_to_default_ptb_arg_supports_selected_pure_types() -> None:
    assert type_to_default_ptb_arg({"kind": "bool"}) == {"bool": False}
    assert type_to_default_ptb_arg({"kind": "u8"}) == {"u8": 1}
    assert type_to_default_ptb_arg({"kind": "u16"}) == {"u16": 1}
    assert type_to_default_ptb_arg({"kind": "u32"}) == {"u32": 1}
    assert type_to_default_ptb_arg({"kind": "u64"}) == {"u64": 1}
    assert type_to_default_ptb_arg({"kind": "address"}) == {"address": DUMMY_ADDRESS}
    assert type_to_default_ptb_arg({"kind": "vector", "type": {"kind": "u8"}}) == {"vector_u8_hex": "0x01"}
    assert type_to_default_ptb_arg({"kind": "vector", "type": {"kind": "bool"}}) == {"vector_bool": [False]}
    assert type_to_default_ptb_arg({"kind": "vector", "type": {"kind": "u16"}}) == {"vector_u16": [1]}
    assert type_to_default_ptb_arg({"kind": "vector", "type": {"kind": "u32"}}) == {"vector_u32": [1]}
    assert type_to_default_ptb_arg({"kind": "vector", "type": {"kind": "u64"}}) == {"vector_u64": [1]}
    assert type_to_default_ptb_arg({"kind": "vector", "type": {"kind": "address"}}) == {
        "vector_address": [DUMMY_ADDRESS]
    }
    assert type_to_default_ptb_arg(
        {
            "kind": "ref",
            "mutable": False,
            "to": {
                "kind": "datatype",
                "address": SUI_FRAMEWORK_ADDRESS,
                "module": "clock",
                "name": "Clock",
                "type_args": [],
            },
        }
    ) == {"shared_object": {"id": SUI_CLOCK_OBJECT_ID, "mutable": False}}

    coin_sui = {
        "kind": "datatype",
        "address": SUI_FRAMEWORK_ADDRESS,
        "module": COIN_MODULE,
        "name": COIN_STRUCT,
        "type_args": [
            {
                "kind": "datatype",
                "address": SUI_FRAMEWORK_ADDRESS,
                "module": SUI_MODULE,
                "name": SUI_STRUCT,
                "type_args": [],
            }
        ],
    }
    assert type_to_default_ptb_arg(coin_sui) == {"sender_sui_coin": {"index": 0, "exclude_gas": True}}


def test_analyze_package_chains_string_construction() -> None:
    pkg = "0x" + ("1" * 64)
    iface = {
        "package_id": pkg,
        "modules": {
            "m": {
                "address": pkg,
                "functions": {
                    "takes_string": {
                        "visibility": "public",
                        "is_entry": True,
                        "type_params": [],
                        "params": [
                            {
                                "kind": "datatype",
                                "address": "0x0000000000000000000000000000000000000000000000000000000000000001",
                                "module": "string",
                                "name": "String",
                                "type_args": [],
                            }
                        ],
                        "returns": [],
                    }
                },
            }
        },
    }
    analysis = analyze_package(iface)
    assert len(analysis.candidates_ok) == 1
    plan = analysis.candidates_ok[0]
    # Expect 2 calls: utf8() then takes_string()
    assert len(plan) == 2
    assert "string::utf8" in plan[0]["target"]
    assert plan[1]["target"] == f"{pkg}::m::takes_string"
    assert plan[1]["args"] == [{"result": 0}]


def test_analyze_package_recursive_constructor() -> None:
    pkg = "0x" + ("1" * 64)
    iface = {
        "package_id": pkg,
        "modules": {
            "m": {
                "address": pkg,
                "functions": {
                    "new_widget": {
                        "visibility": "public",
                        "is_entry": False,
                        "type_params": [],
                        "params": [{"kind": "u64"}],
                        "returns": [
                            {"kind": "datatype", "address": pkg, "module": "m", "name": "Widget", "type_args": []}
                        ],
                    },
                    "use_widget": {
                        "visibility": "public",
                        "is_entry": True,
                        "type_params": [],
                        "params": [
                            {"kind": "datatype", "address": pkg, "module": "m", "name": "Widget", "type_args": []}
                        ],
                        "returns": [],
                    },
                },
            }
        },
    }
    analysis = analyze_package(iface)
    assert len(analysis.candidates_ok) == 1
    plan = analysis.candidates_ok[0]
    # Expect 2 calls: new_widget() then use_widget()
    assert len(plan) == 2
    assert plan[0]["target"] == f"{pkg}::m::new_widget"
    assert plan[1]["target"] == f"{pkg}::m::use_widget"
    assert plan[1]["args"] == [{"result": 0}]


def test_analyze_package_fills_type_params_with_sui() -> None:
    pkg = "0x" + ("1" * 64)
    iface = {
        "package_id": pkg,
        "modules": {
            "m": {
                "address": pkg,
                "functions": {
                    "generic_func": {
                        "visibility": "public",
                        "is_entry": True,
                        "type_params": [{"constraints": []}],
                        "params": [{"kind": "u64"}],
                        "returns": [],
                    }
                },
            }
        },
    }
    analysis = analyze_package(iface)
    assert len(analysis.candidates_ok) == 1
    plan = analysis.candidates_ok[0]
    assert plan[0]["type_args"] == [f"{SUI_FRAMEWORK_ADDRESS}::{SUI_MODULE}::{SUI_STRUCT}"]


def test_select_executable_ptb_spec_picks_public_entry_no_generics() -> None:
    pkg = "0x" + ("1" * 64)
    iface = {
        "schema_version": 1,
        "package_id": pkg,
        "module_names": ["m"],
        "modules": {
            "m": {
                "address": pkg,
                "structs": {},
                "functions": {
                    # Not entry.
                    "not_entry": {
                        "visibility": "public",
                        "is_entry": False,
                        "is_native": False,
                        "type_params": [],
                        "params": [],
                        "returns": [],
                        "acquires": [],
                    },
                    # Entry but has generics (skip).
                    "generic_entry": {
                        "visibility": "public",
                        "is_entry": True,
                        "is_native": False,
                        "type_params": [{"constraints": []}],
                        "params": [],
                        "returns": [],
                        "acquires": [],
                    },
                    # Entry but unsupported args (datatype).
                    "needs_object": {
                        "visibility": "public",
                        "is_entry": True,
                        "is_native": False,
                        "type_params": [],
                        "params": [{"kind": "datatype", "address": pkg, "module": "m", "name": "Obj", "type_args": []}],
                        "returns": [],
                        "acquires": [],
                    },
                    # The executable one.
                    "ok": {
                        "visibility": "public",
                        "is_entry": True,
                        "is_native": False,
                        "type_params": [],
                        "params": [
                            {"kind": "u64"},
                            {
                                "kind": "ref",
                                "mutable": True,
                                "to": {
                                    "kind": "datatype",
                                    "address": "0x" + ("0" * 62) + "02",
                                    "module": "tx_context",
                                    "name": "TxContext",
                                    "type_args": [],
                                },
                            },
                        ],
                        "returns": [],
                        "acquires": [],
                    },
                },
            }
        },
    }
    ptb, calls = select_executable_ptb_spec(interface_json=iface, max_calls_per_package=1)
    assert ptb == {"calls": [{"target": f"{pkg}::m::ok", "type_args": [], "args": [{"u64": 1}]}]}
    assert calls == [{"target": f"{pkg}::m::ok", "type_args": [], "args": [{"u64": 1}]}]
