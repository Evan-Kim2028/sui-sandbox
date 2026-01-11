//! Static bytecode analysis to extract function calls from a module.
//!
//! This provides function-level tracing without relying on VM runtime tracing,
//! which has internal assertion issues with the current Sui/Move version.

use move_binary_format::file_format::{Bytecode, CompiledModule, FunctionDefinition, FunctionHandleIndex};
use serde::Serialize;
use std::collections::BTreeSet;

/// Represents a static function call found in bytecode
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct StaticFunctionCall {
    /// Module containing the called function (e.g., "0x1::vector")
    pub target_module: String,
    /// Function name
    pub function_name: String,
    /// Whether this is a generic function call
    pub is_generic: bool,
}

/// Analyze a compiled module to extract all function calls it makes.
/// Returns a list of unique (module, function) pairs that are called.
pub fn extract_function_calls(module: &CompiledModule) -> Vec<StaticFunctionCall> {
    let mut calls: BTreeSet<StaticFunctionCall> = BTreeSet::new();
    
    // Iterate through all function definitions
    for func_def in &module.function_defs {
        if let Some(code) = &func_def.code {
            // Analyze each instruction
            for instruction in &code.code {
                if let Some(call) = extract_call_from_instruction(instruction, module) {
                    calls.insert(call);
                }
            }
        }
    }
    
    calls.into_iter().collect()
}

/// Analyze a specific function definition to extract its calls
pub fn extract_function_calls_from_function(
    module: &CompiledModule,
    func_def: &FunctionDefinition,
) -> Vec<StaticFunctionCall> {
    let mut calls: BTreeSet<StaticFunctionCall> = BTreeSet::new();
    
    if let Some(code) = &func_def.code {
        for instruction in &code.code {
            if let Some(call) = extract_call_from_instruction(instruction, module) {
                calls.insert(call);
            }
        }
    }
    
    calls.into_iter().collect()
}

/// Extract a function call from a bytecode instruction
fn extract_call_from_instruction(
    instruction: &Bytecode,
    module: &CompiledModule,
) -> Option<StaticFunctionCall> {
    match instruction {
        Bytecode::Call(handle_idx) => {
            Some(resolve_function_handle(*handle_idx, module, false))
        }
        Bytecode::CallGeneric(inst_idx) => {
            let inst = &module.function_instantiations[inst_idx.0 as usize];
            Some(resolve_function_handle(inst.handle, module, true))
        }
        _ => None,
    }
}

/// Resolve a function handle index to a StaticFunctionCall
fn resolve_function_handle(
    handle_idx: FunctionHandleIndex,
    module: &CompiledModule,
    is_generic: bool,
) -> StaticFunctionCall {
    let handle = &module.function_handles[handle_idx.0 as usize];
    let module_handle = &module.module_handles[handle.module.0 as usize];
    
    let address = module.address_identifier_at(module_handle.address);
    let module_name = module.identifier_at(module_handle.name);
    let function_name = module.identifier_at(handle.name);
    
    StaticFunctionCall {
        target_module: format!("{}::{}", address.to_hex_literal(), module_name),
        function_name: function_name.to_string(),
        is_generic,
    }
}

/// Filter function calls to only those from a specific package address
pub fn filter_calls_by_package(
    calls: &[StaticFunctionCall],
    package_address: &str,
) -> Vec<StaticFunctionCall> {
    calls
        .iter()
        .filter(|c| c.target_module.starts_with(package_address))
        .cloned()
        .collect()
}

/// Filter out framework calls (0x1, 0x2, 0x3)
pub fn filter_non_framework_calls(calls: &[StaticFunctionCall]) -> Vec<StaticFunctionCall> {
    calls
        .iter()
        .filter(|c| {
            !c.target_module.starts_with("0x1::") &&
            !c.target_module.starts_with("0x2::") &&
            !c.target_module.starts_with("0x3::")
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_filter_non_framework() {
        let calls = vec![
            StaticFunctionCall {
                target_module: "0x1::vector".to_string(),
                function_name: "push_back".to_string(),
                is_generic: true,
            },
            StaticFunctionCall {
                target_module: "0x2::object".to_string(),
                function_name: "new".to_string(),
                is_generic: false,
            },
            StaticFunctionCall {
                target_module: "0xabc::my_module".to_string(),
                function_name: "do_something".to_string(),
                is_generic: false,
            },
        ];
        
        let filtered = filter_non_framework_calls(&calls);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].target_module, "0xabc::my_module");
    }
}
