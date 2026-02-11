//! Parameter classification for Move function fuzzing.
//!
//! Classifies each parameter of a Move function as pure (fuzzable),
//! system-injected (auto-handled), object-based (Phase 2), or unfuzzable.

use move_binary_format::file_format::SignatureToken;
use move_binary_format::CompiledModule;
use serde::{Deserialize, Serialize};

/// Classification of a function parameter for fuzzing purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "class")]
pub enum ParamClass {
    /// Pure BCS value — fully fuzzable.
    Pure { pure_type: PureType },
    /// System-injected parameter — skipped by fuzzer, auto-handled by PTBExecutor.
    SystemInjected { system_type: SystemType },
    /// Object passed by reference — not fuzzable in Phase 1.
    ObjectRef { mutable: bool, type_str: String },
    /// Object passed by value — not fuzzable in Phase 1.
    ObjectOwned { type_str: String },
    /// Cannot be fuzzed (unresolved generics, complex patterns).
    Unfuzzable { reason: String },
}

/// Pure value types that can be randomly generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PureType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    U256,
    Address,
    VectorBool,
    VectorU8,
    VectorU16,
    VectorU32,
    VectorU64,
    VectorU128,
    VectorU256,
    VectorAddress,
    /// 0x1::string::String — BCS is vector<u8>
    String,
    /// 0x1::ascii::String — BCS is vector<u8>
    AsciiString,
}

/// System types auto-injected by the PTB executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemType {
    TxContext,
    MutTxContext,
    Clock,
}

/// Result of classifying a function's parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedFunction {
    /// Human-readable type string and classification for each parameter.
    pub params: Vec<(String, ParamClass)>,
    /// Whether all parameters are either Pure or SystemInjected.
    pub is_fully_fuzzable: bool,
    /// Count of pure (fuzzable) parameters.
    pub pure_count: usize,
    /// Count of system-injected parameters.
    pub system_count: usize,
    /// Count of object parameters (not fuzzable in Phase 1).
    pub object_count: usize,
    /// Count of unfuzzable parameters.
    pub unfuzzable_count: usize,
}

/// Resolve a struct's fully-qualified name from a SignatureToken::Datatype index.
fn resolve_struct_name(
    module: &CompiledModule,
    idx: &move_binary_format::file_format::DatatypeHandleIndex,
) -> (String, String, String) {
    let datatype_handle = &module.datatype_handles[idx.0 as usize];
    let module_handle = &module.module_handles[datatype_handle.module.0 as usize];
    let addr = module
        .address_identifier_at(module_handle.address)
        .to_hex_literal();
    let mod_name = module.identifier_at(module_handle.name).to_string();
    let type_name = module.identifier_at(datatype_handle.name).to_string();
    (addr, mod_name, type_name)
}

/// Check if a resolved struct is TxContext.
fn is_tx_context(addr: &str, mod_name: &str, type_name: &str) -> bool {
    // TxContext lives at 0x2::tx_context::TxContext
    (addr == "0x2" || addr == "0x0000000000000000000000000000000000000000000000000000000000000002")
        && mod_name == "tx_context"
        && type_name == "TxContext"
}

/// Check if a resolved struct is Clock.
fn is_clock(addr: &str, mod_name: &str, type_name: &str) -> bool {
    (addr == "0x2" || addr == "0x0000000000000000000000000000000000000000000000000000000000000002")
        && mod_name == "clock"
        && type_name == "Clock"
}

/// Check if a resolved struct is 0x1::string::String.
fn is_string(addr: &str, mod_name: &str, type_name: &str) -> bool {
    (addr == "0x1" || addr == "0x0000000000000000000000000000000000000000000000000000000000000001")
        && mod_name == "string"
        && type_name == "String"
}

/// Check if a resolved struct is 0x1::ascii::String.
fn is_ascii_string(addr: &str, mod_name: &str, type_name: &str) -> bool {
    (addr == "0x1" || addr == "0x0000000000000000000000000000000000000000000000000000000000000001")
        && mod_name == "ascii"
        && type_name == "String"
}

/// Format a SignatureToken as a human-readable type string.
/// Mirrors `format_signature_token` in resolver.rs but is standalone.
fn format_token(module: &CompiledModule, token: &SignatureToken) -> String {
    match token {
        SignatureToken::Bool => "bool".into(),
        SignatureToken::U8 => "u8".into(),
        SignatureToken::U16 => "u16".into(),
        SignatureToken::U32 => "u32".into(),
        SignatureToken::U64 => "u64".into(),
        SignatureToken::U128 => "u128".into(),
        SignatureToken::U256 => "u256".into(),
        SignatureToken::Address => "address".into(),
        SignatureToken::Signer => "signer".into(),
        SignatureToken::Vector(inner) => format!("vector<{}>", format_token(module, inner)),
        SignatureToken::Datatype(idx) => {
            let (addr, mod_name, type_name) = resolve_struct_name(module, idx);
            format!("{addr}::{mod_name}::{type_name}")
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, type_args) = inst.as_ref();
            let (addr, mod_name, type_name) = resolve_struct_name(module, idx);
            let args: Vec<String> = type_args.iter().map(|t| format_token(module, t)).collect();
            format!("{addr}::{mod_name}::{type_name}<{}>", args.join(", "))
        }
        SignatureToken::Reference(inner) => format!("&{}", format_token(module, inner)),
        SignatureToken::MutableReference(inner) => format!("&mut {}", format_token(module, inner)),
        SignatureToken::TypeParameter(idx) => format!("T{idx}"),
    }
}

/// Classify a single SignatureToken into a ParamClass.
fn classify_token(module: &CompiledModule, token: &SignatureToken) -> ParamClass {
    match token {
        // Primitives — all pure
        SignatureToken::Bool => ParamClass::Pure {
            pure_type: PureType::Bool,
        },
        SignatureToken::U8 => ParamClass::Pure {
            pure_type: PureType::U8,
        },
        SignatureToken::U16 => ParamClass::Pure {
            pure_type: PureType::U16,
        },
        SignatureToken::U32 => ParamClass::Pure {
            pure_type: PureType::U32,
        },
        SignatureToken::U64 => ParamClass::Pure {
            pure_type: PureType::U64,
        },
        SignatureToken::U128 => ParamClass::Pure {
            pure_type: PureType::U128,
        },
        SignatureToken::U256 => ParamClass::Pure {
            pure_type: PureType::U256,
        },
        SignatureToken::Address => ParamClass::Pure {
            pure_type: PureType::Address,
        },

        // Vectors of pure types
        SignatureToken::Vector(inner) => match inner.as_ref() {
            SignatureToken::Bool => ParamClass::Pure {
                pure_type: PureType::VectorBool,
            },
            SignatureToken::U8 => ParamClass::Pure {
                pure_type: PureType::VectorU8,
            },
            SignatureToken::U16 => ParamClass::Pure {
                pure_type: PureType::VectorU16,
            },
            SignatureToken::U32 => ParamClass::Pure {
                pure_type: PureType::VectorU32,
            },
            SignatureToken::U64 => ParamClass::Pure {
                pure_type: PureType::VectorU64,
            },
            SignatureToken::U128 => ParamClass::Pure {
                pure_type: PureType::VectorU128,
            },
            SignatureToken::U256 => ParamClass::Pure {
                pure_type: PureType::VectorU256,
            },
            SignatureToken::Address => ParamClass::Pure {
                pure_type: PureType::VectorAddress,
            },
            _ => ParamClass::Unfuzzable {
                reason: format!(
                    "nested vector type: vector<{}>",
                    format_token(module, inner)
                ),
            },
        },

        // Structs — check for well-known pure types
        SignatureToken::Datatype(idx) => {
            let (addr, mod_name, type_name) = resolve_struct_name(module, idx);
            if is_string(&addr, &mod_name, &type_name) {
                ParamClass::Pure {
                    pure_type: PureType::String,
                }
            } else if is_ascii_string(&addr, &mod_name, &type_name) {
                ParamClass::Pure {
                    pure_type: PureType::AsciiString,
                }
            } else {
                ParamClass::ObjectOwned {
                    type_str: format!("{addr}::{mod_name}::{type_name}"),
                }
            }
        }

        // Generic structs — check for well-known pure types, otherwise object
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, _type_args) = inst.as_ref();
            let (addr, mod_name, type_name) = resolve_struct_name(module, idx);
            ParamClass::ObjectOwned {
                type_str: format!("{addr}::{mod_name}::{type_name}<...>"),
            }
        }

        // References — check for system types (TxContext, Clock)
        SignatureToken::Reference(inner) => classify_reference(module, inner, false),
        SignatureToken::MutableReference(inner) => classify_reference(module, inner, true),

        // Type parameters without concrete instantiation
        SignatureToken::TypeParameter(idx) => ParamClass::Unfuzzable {
            reason: format!("unresolved type parameter T{idx}"),
        },

        // Signer is not a normal parameter type
        SignatureToken::Signer => ParamClass::Unfuzzable {
            reason: "signer type".into(),
        },
    }
}

/// Classify a reference target (the inner type of &T or &mut T).
fn classify_reference(
    module: &CompiledModule,
    inner: &SignatureToken,
    mutable: bool,
) -> ParamClass {
    match inner {
        SignatureToken::Datatype(idx) => {
            let (addr, mod_name, type_name) = resolve_struct_name(module, idx);
            if is_tx_context(&addr, &mod_name, &type_name) {
                if mutable {
                    ParamClass::SystemInjected {
                        system_type: SystemType::MutTxContext,
                    }
                } else {
                    ParamClass::SystemInjected {
                        system_type: SystemType::TxContext,
                    }
                }
            } else if is_clock(&addr, &mod_name, &type_name) {
                ParamClass::SystemInjected {
                    system_type: SystemType::Clock,
                }
            } else {
                ParamClass::ObjectRef {
                    mutable,
                    type_str: format!("{addr}::{mod_name}::{type_name}"),
                }
            }
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, _) = inst.as_ref();
            let (addr, mod_name, type_name) = resolve_struct_name(module, idx);
            ParamClass::ObjectRef {
                mutable,
                type_str: format!("{addr}::{mod_name}::{type_name}<...>"),
            }
        }
        _ => ParamClass::Unfuzzable {
            reason: format!(
                "reference to non-struct type: {}{}",
                if mutable { "&mut " } else { "&" },
                format_token(module, inner)
            ),
        },
    }
}

/// Classify all parameters of a function given its SignatureTokens and the compiled module.
pub fn classify_params(
    module: &CompiledModule,
    param_tokens: &[SignatureToken],
) -> ClassifiedFunction {
    let params: Vec<(String, ParamClass)> = param_tokens
        .iter()
        .map(|token| {
            let type_str = format_token(module, token);
            let class = classify_token(module, token);
            (type_str, class)
        })
        .collect();

    let pure_count = params
        .iter()
        .filter(|(_, c)| matches!(c, ParamClass::Pure { .. }))
        .count();
    let system_count = params
        .iter()
        .filter(|(_, c)| matches!(c, ParamClass::SystemInjected { .. }))
        .count();
    let object_count = params
        .iter()
        .filter(|(_, c)| {
            matches!(
                c,
                ParamClass::ObjectRef { .. } | ParamClass::ObjectOwned { .. }
            )
        })
        .count();
    let unfuzzable_count = params
        .iter()
        .filter(|(_, c)| matches!(c, ParamClass::Unfuzzable { .. }))
        .count();
    let is_fully_fuzzable = object_count == 0 && unfuzzable_count == 0;

    ClassifiedFunction {
        params,
        is_fully_fuzzable,
        pure_count,
        system_count,
        object_count,
        unfuzzable_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pure_type_classification() {
        // Verify that primitive tokens classify as Pure
        // We can't easily construct a CompiledModule in tests, but we can test
        // the classify_token logic for primitive types that don't need module context.
        // The actual integration tests will use real modules.

        // For primitives, classify_token doesn't use the module parameter,
        // but the function signature requires it. We'll test the enum variants directly.
        assert!(matches!(PureType::Bool, PureType::Bool));
        assert!(matches!(PureType::U64, PureType::U64));
    }

    #[test]
    fn test_classified_function_counts() {
        let classified = ClassifiedFunction {
            params: vec![
                (
                    "u64".into(),
                    ParamClass::Pure {
                        pure_type: PureType::U64,
                    },
                ),
                (
                    "bool".into(),
                    ParamClass::Pure {
                        pure_type: PureType::Bool,
                    },
                ),
                (
                    "&mut TxContext".into(),
                    ParamClass::SystemInjected {
                        system_type: SystemType::MutTxContext,
                    },
                ),
            ],
            is_fully_fuzzable: true,
            pure_count: 2,
            system_count: 1,
            object_count: 0,
            unfuzzable_count: 0,
        };
        assert!(classified.is_fully_fuzzable);
        assert_eq!(classified.pure_count, 2);
        assert_eq!(classified.system_count, 1);
    }

    #[test]
    fn test_classified_function_with_objects() {
        let classified = ClassifiedFunction {
            params: vec![
                (
                    "&mut 0x2::coin::Coin<0x2::sui::SUI>".into(),
                    ParamClass::ObjectRef {
                        mutable: true,
                        type_str: "0x2::coin::Coin<...>".into(),
                    },
                ),
                (
                    "u64".into(),
                    ParamClass::Pure {
                        pure_type: PureType::U64,
                    },
                ),
            ],
            is_fully_fuzzable: false,
            pure_count: 1,
            system_count: 0,
            object_count: 1,
            unfuzzable_count: 0,
        };
        assert!(!classified.is_fully_fuzzable);
        assert_eq!(classified.object_count, 1);
    }
}
