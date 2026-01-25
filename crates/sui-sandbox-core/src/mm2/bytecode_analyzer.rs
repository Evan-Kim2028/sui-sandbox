//! Bytecode instruction analysis for dynamic field access prediction.
//!
//! This module analyzes Move bytecode to predict which dynamic fields a function
//! will access at runtime. This enables more precise data prefetching for transaction
//! replay by identifying dynamic_field calls and their key/value type patterns.
//!
//! ## Strategy
//!
//! 1. Walk bytecode instructions to find `Call` and `CallGeneric` instructions
//! 2. Identify calls to `sui::dynamic_field::*` functions
//! 3. Extract key and value type patterns from generic instantiations
//! 4. Track type parameter usage to enable resolution at call sites
//!
//! ## Usage
//!
//! ```rust,ignore
//! let analyzer = BytecodeAnalyzer::new();
//! let analysis = analyzer.analyze_function(&compiled_module, func_def_idx)?;
//!
//! // Check what dynamic fields this function might access
//! for access in &analysis.dynamic_field_accesses {
//!     println!("Key type: {}, Value type: {}", access.key_type_pattern, access.value_type_pattern);
//! }
//! ```

use move_binary_format::{
    file_format::{
        Bytecode, DatatypeHandleIndex, FunctionDefinitionIndex, FunctionHandleIndex,
        SignatureIndex, SignatureToken,
    },
    CompiledModule,
};
use move_core_types::account_address::AccountAddress;
use std::collections::HashMap;

// Well-known addresses for dynamic field modules
const SUI_FRAMEWORK_ADDR: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000002";

/// Patterns of dynamic field access found in bytecode.
#[derive(Debug, Clone)]
pub struct DynamicFieldAccessPattern {
    /// The key type pattern (may contain type parameters like T0, T1)
    pub key_type_pattern: String,
    /// The value type pattern
    pub value_type_pattern: String,
    /// The dynamic_field function being called
    pub access_kind: DynamicFieldAccessKind,
    /// Source location hint (instruction index)
    pub instruction_index: usize,
}

/// Kind of dynamic field access operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicFieldAccessKind {
    /// borrow_child_object - read access
    Borrow,
    /// borrow_child_object_mut - write access
    BorrowMut,
    /// add_child_object - create
    Add,
    /// remove_child_object - delete
    Remove,
    /// has_child_object / has_child_object_with_ty - existence check
    Exists,
    /// field_info - get field metadata
    FieldInfo,
}

impl DynamicFieldAccessKind {
    /// Returns true if this is a mutating operation.
    pub fn is_mutating(&self) -> bool {
        matches!(self, Self::BorrowMut | Self::Add | Self::Remove)
    }
}

/// Result of analyzing a function's bytecode for dynamic field accesses.
#[derive(Debug, Default, Clone)]
pub struct FunctionAccessAnalysis {
    /// Dynamic field access patterns found in this function
    pub dynamic_field_accesses: Vec<DynamicFieldAccessPattern>,
    /// Other functions called (for transitive analysis)
    /// Format: (package_addr, module_name, function_name)
    pub called_functions: Vec<(AccountAddress, String, String)>,
    /// Type parameter indices used in dynamic field keys
    /// This helps determine which type args affect field access
    pub key_type_params_used: Vec<usize>,
}

/// Analyzer for Move bytecode instructions.
///
/// Walks bytecode to identify dynamic field operations and their type patterns.
/// Results are cached for efficiency when analyzing the same function multiple times.
pub struct BytecodeAnalyzer {
    /// Cache of analyzed functions: (module_id, func_def_idx) -> analysis
    cache: HashMap<(AccountAddress, String, u16), FunctionAccessAnalysis>,
}

impl Default for BytecodeAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl BytecodeAnalyzer {
    /// Create a new bytecode analyzer.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Analyze a function for dynamic field access patterns.
    ///
    /// # Arguments
    /// * `module` - The compiled module containing the function
    /// * `func_def_idx` - Index of the function definition to analyze
    ///
    /// # Returns
    /// Analysis results including dynamic field accesses and called functions.
    pub fn analyze_function(
        &mut self,
        module: &CompiledModule,
        func_def_idx: FunctionDefinitionIndex,
    ) -> FunctionAccessAnalysis {
        let module_addr = *module.self_id().address();
        let module_name = module.self_id().name().to_string();
        let cache_key = (module_addr, module_name.clone(), func_def_idx.0);

        // Check cache first
        if let Some(cached) = self.cache.get(&cache_key) {
            return cached.clone();
        }

        let analysis = self.analyze_function_impl(module, func_def_idx);

        // Cache the result
        self.cache.insert(cache_key, analysis.clone());
        analysis
    }

    /// Clear the analysis cache.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Internal implementation of function analysis.
    fn analyze_function_impl(
        &self,
        module: &CompiledModule,
        func_def_idx: FunctionDefinitionIndex,
    ) -> FunctionAccessAnalysis {
        let mut analysis = FunctionAccessAnalysis::default();

        let func_def = &module.function_defs()[func_def_idx.0 as usize];

        // Native functions have no bytecode
        let code = match &func_def.code {
            Some(code) => code,
            None => return analysis,
        };

        // Walk all instructions
        for (idx, instruction) in code.code.iter().enumerate() {
            match instruction {
                Bytecode::Call(func_handle_idx) => {
                    self.analyze_call(module, *func_handle_idx, None, idx, &mut analysis);
                }
                Bytecode::CallGeneric(func_inst_idx) => {
                    let func_inst = &module.function_instantiations()[func_inst_idx.0 as usize];
                    self.analyze_call(
                        module,
                        func_inst.handle,
                        Some(func_inst.type_parameters),
                        idx,
                        &mut analysis,
                    );
                }
                _ => {}
            }
        }

        // Deduplicate key type params
        analysis.key_type_params_used.sort();
        analysis.key_type_params_used.dedup();

        analysis
    }

    /// Analyze a function call instruction.
    fn analyze_call(
        &self,
        module: &CompiledModule,
        func_handle_idx: FunctionHandleIndex,
        type_params: Option<SignatureIndex>,
        instruction_idx: usize,
        analysis: &mut FunctionAccessAnalysis,
    ) {
        let func_handle = &module.function_handles()[func_handle_idx.0 as usize];
        let module_handle = &module.module_handles()[func_handle.module.0 as usize];

        // Get module address and name
        let callee_addr = module.address_identifier_at(module_handle.address);
        let callee_module_name = module.identifier_at(module_handle.name).to_string();
        let func_name = module.identifier_at(func_handle.name).to_string();

        // Record all called functions for transitive analysis
        analysis.called_functions.push((
            *callee_addr,
            callee_module_name.clone(),
            func_name.clone(),
        ));

        // Check if this is a dynamic_field call
        let callee_addr_str = format!("{:#066x}", callee_addr);
        if callee_addr_str != SUI_FRAMEWORK_ADDR {
            return;
        }

        // Check for dynamic_field or dynamic_object_field modules
        if callee_module_name != "dynamic_field" && callee_module_name != "dynamic_object_field" {
            return;
        }

        // Determine the access kind based on function name
        let access_kind = match func_name.as_str() {
            "borrow_child_object" => DynamicFieldAccessKind::Borrow,
            "borrow_child_object_mut" => DynamicFieldAccessKind::BorrowMut,
            "add_child_object" => DynamicFieldAccessKind::Add,
            "remove_child_object" => DynamicFieldAccessKind::Remove,
            "has_child_object" | "has_child_object_with_ty" => DynamicFieldAccessKind::Exists,
            "field_info" | "field_info_mut" => DynamicFieldAccessKind::FieldInfo,
            // Higher-level wrappers that we should also track
            "borrow" | "borrow_mut" | "add" | "remove" | "exists_" | "exists_with_type" => {
                match func_name.as_str() {
                    "borrow" => DynamicFieldAccessKind::Borrow,
                    "borrow_mut" => DynamicFieldAccessKind::BorrowMut,
                    "add" => DynamicFieldAccessKind::Add,
                    "remove" => DynamicFieldAccessKind::Remove,
                    _ => DynamicFieldAccessKind::Exists,
                }
            }
            _ => return, // Not a dynamic field access we care about
        };

        // Extract type parameters if this is a generic call
        let (key_type_pattern, value_type_pattern) = if let Some(sig_idx) = type_params {
            self.extract_type_patterns(module, sig_idx, analysis)
        } else {
            ("unknown".to_string(), "unknown".to_string())
        };

        analysis
            .dynamic_field_accesses
            .push(DynamicFieldAccessPattern {
                key_type_pattern,
                value_type_pattern,
                access_kind,
                instruction_index: instruction_idx,
            });
    }

    /// Extract key and value type patterns from generic instantiation.
    ///
    /// For dynamic_field functions, the signature is typically:
    /// - borrow_child_object<K: copy + drop + store, V: key + store>
    /// - The first type param is the key type, second is value type
    fn extract_type_patterns(
        &self,
        module: &CompiledModule,
        sig_idx: SignatureIndex,
        analysis: &mut FunctionAccessAnalysis,
    ) -> (String, String) {
        let signature = &module.signatures()[sig_idx.0 as usize];

        let key_type = signature
            .0
            .first()
            .map(|t| self.format_signature_token(module, t, analysis))
            .unwrap_or_else(|| "unknown".to_string());

        let value_type = signature
            .0
            .get(1)
            .map(|t| self.format_signature_token(module, t, analysis))
            .unwrap_or_else(|| "unknown".to_string());

        (key_type, value_type)
    }

    /// Format a SignatureToken as a type string.
    ///
    /// Type parameters are formatted as T0, T1, etc. and tracked in the analysis
    /// for later resolution at call sites.
    fn format_signature_token(
        &self,
        module: &CompiledModule,
        token: &SignatureToken,
        analysis: &mut FunctionAccessAnalysis,
    ) -> String {
        match token {
            SignatureToken::Bool => "bool".to_string(),
            SignatureToken::U8 => "u8".to_string(),
            SignatureToken::U16 => "u16".to_string(),
            SignatureToken::U32 => "u32".to_string(),
            SignatureToken::U64 => "u64".to_string(),
            SignatureToken::U128 => "u128".to_string(),
            SignatureToken::U256 => "u256".to_string(),
            SignatureToken::Address => "address".to_string(),
            SignatureToken::Signer => "signer".to_string(),
            SignatureToken::Vector(inner) => {
                format!(
                    "vector<{}>",
                    self.format_signature_token(module, inner, analysis)
                )
            }
            SignatureToken::Datatype(datatype_handle_idx) => {
                self.format_datatype_handle(module, *datatype_handle_idx)
            }
            SignatureToken::DatatypeInstantiation(instantiation) => {
                let (datatype_handle_idx, type_args) = instantiation.as_ref();
                let base = self.format_datatype_handle(module, *datatype_handle_idx);
                let args: Vec<String> = type_args
                    .iter()
                    .map(|t| self.format_signature_token(module, t, analysis))
                    .collect();
                format!("{}<{}>", base, args.join(", "))
            }
            SignatureToken::Reference(inner) => {
                format!("&{}", self.format_signature_token(module, inner, analysis))
            }
            SignatureToken::MutableReference(inner) => {
                format!(
                    "&mut {}",
                    self.format_signature_token(module, inner, analysis)
                )
            }
            SignatureToken::TypeParameter(idx) => {
                // Track that this type parameter is used in a key position
                analysis.key_type_params_used.push(*idx as usize);
                format!("T{}", idx)
            }
        }
    }

    /// Format a datatype handle as a fully qualified type string.
    fn format_datatype_handle(&self, module: &CompiledModule, idx: DatatypeHandleIndex) -> String {
        let datatype_handle = module.datatype_handle_at(idx);
        let module_handle = module.module_handle_at(datatype_handle.module);

        let addr = module.address_identifier_at(module_handle.address);
        let module_name = module.identifier_at(module_handle.name);
        let datatype_name = module.identifier_at(datatype_handle.name);

        format!("{:#066x}::{}::{}", addr, module_name, datatype_name)
    }
}

/// Resolve type parameters in a pattern using concrete type arguments.
///
/// # Arguments
/// * `pattern` - Type pattern containing T0, T1, etc.
/// * `type_args` - Concrete types to substitute (index matches parameter number)
///
/// # Returns
/// Fully resolved type string with all type parameters replaced.
///
/// # Example
/// ```rust,ignore
/// let resolved = resolve_type_pattern(
///     "0xabc::module::Key<T0>",
///     &["0xdef::token::TOKEN".to_string()]
/// );
/// assert_eq!(resolved, "0xabc::module::Key<0xdef::token::TOKEN>");
/// ```
pub fn resolve_type_pattern(pattern: &str, type_args: &[String]) -> String {
    let mut result = pattern.to_string();

    // Replace type parameters in reverse order to handle T10+ correctly
    for i in (0..type_args.len()).rev() {
        let placeholder = format!("T{}", i);
        result = result.replace(&placeholder, &type_args[i]);
    }

    result
}

/// Check if a resolved type still contains unresolved type parameters.
pub fn has_unresolved_params(type_str: &str) -> bool {
    // Look for patterns like T0, T1, T2, etc.
    let mut chars = type_str.chars().peekable();
    while let Some(c) = chars.next() {
        if c == 'T' {
            if let Some(&next) = chars.peek() {
                if next.is_ascii_digit() {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_type_pattern_simple() {
        let pattern = "0xabc::module::Key<T0>";
        let type_args = vec!["0xdef::token::TOKEN".to_string()];
        let resolved = resolve_type_pattern(pattern, &type_args);
        assert_eq!(resolved, "0xabc::module::Key<0xdef::token::TOKEN>");
    }

    #[test]
    fn test_resolve_type_pattern_multiple() {
        let pattern = "0xabc::module::Pair<T0, T1>";
        let type_args = vec!["0xdef::a::A".to_string(), "0xdef::b::B".to_string()];
        let resolved = resolve_type_pattern(pattern, &type_args);
        assert_eq!(resolved, "0xabc::module::Pair<0xdef::a::A, 0xdef::b::B>");
    }

    #[test]
    fn test_resolve_type_pattern_no_params() {
        let pattern = "0xabc::module::Concrete";
        let type_args: Vec<String> = vec![];
        let resolved = resolve_type_pattern(pattern, &type_args);
        assert_eq!(resolved, "0xabc::module::Concrete");
    }

    #[test]
    fn test_has_unresolved_params() {
        assert!(has_unresolved_params("Key<T0>"));
        assert!(has_unresolved_params("Pair<T0, T1>"));
        assert!(has_unresolved_params("Complex<u64, T2>"));
        assert!(!has_unresolved_params("Key<u64>"));
        assert!(!has_unresolved_params("0xabc::module::TOKEN"));
        // Edge case: TOKEN shouldn't match
        assert!(!has_unresolved_params("TOKEN"));
    }

    #[test]
    fn test_access_kind_mutating() {
        assert!(!DynamicFieldAccessKind::Borrow.is_mutating());
        assert!(DynamicFieldAccessKind::BorrowMut.is_mutating());
        assert!(DynamicFieldAccessKind::Add.is_mutating());
        assert!(DynamicFieldAccessKind::Remove.is_mutating());
        assert!(!DynamicFieldAccessKind::Exists.is_mutating());
        assert!(!DynamicFieldAccessKind::FieldInfo.is_mutating());
    }
}
