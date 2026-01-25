//! Call graph analysis for transitive dynamic field access detection.
//!
//! This module builds a complete call graph from compiled modules and uses sink
//! propagation to identify all functions that transitively call dynamic_field
//! operations. This enables discovery of dynamic field accesses through wrapper
//! functions like `table::borrow`, `bag::add`, etc. without hardcoding.
//!
//! ## Strategy
//!
//! 1. Build a complete call graph from all loaded modules
//! 2. Mark known dynamic_field::* functions as "sinks"
//! 3. Propagate sink status backwards through callers (BFS)
//! 4. Track type parameter flow through call chains
//!
//! ## Example
//!
//! ```text
//! user_code::withdraw()
//!     └─► deepbook::balance_manager::withdraw_with_proof()
//!             └─► sui::table::borrow()
//!                     └─► sui::dynamic_field::borrow_child_object()  [SINK]
//! ```
//!
//! After propagation, all four functions are marked as "reaches sink".

use move_binary_format::{
    file_format::{Bytecode, SignatureIndex, SignatureToken},
    CompiledModule,
};
use move_core_types::account_address::AccountAddress;
use std::collections::{HashMap, VecDeque};

/// Unique identifier for a function.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionKey {
    pub package: AccountAddress,
    pub module: String,
    pub function: String,
}

impl FunctionKey {
    pub fn new(package: AccountAddress, module: impl Into<String>, function: impl Into<String>) -> Self {
        Self {
            package,
            module: module.into(),
            function: function.into(),
        }
    }

    /// Check if this is a Sui framework address (0x1, 0x2, 0x3).
    pub fn is_framework(&self) -> bool {
        let bytes = self.package.to_vec();
        bytes.iter().take(31).all(|&b| b == 0) && bytes[31] <= 3
    }
}

impl std::fmt::Display for FunctionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}::{}", self.package, self.module, self.function)
    }
}

/// Type parameter mapping through a call edge.
///
/// Tracks how type parameters from the caller map to type parameters
/// in the callee, enabling type argument resolution through call chains.
#[derive(Debug, Clone)]
pub struct TypeParamMapping {
    /// Maps callee type param index -> resolved type or caller type param
    pub mappings: Vec<TypeParamResolution>,
}

/// Resolution of a type parameter at a call site.
#[derive(Debug, Clone)]
pub enum TypeParamResolution {
    /// Resolved to a concrete type string
    Concrete(String),
    /// Refers to caller's type parameter by index
    CallerTypeParam(usize),
    /// Unknown/unresolved
    Unknown,
}

/// Information about a path from a function to a dynamic_field sink.
#[derive(Debug, Clone)]
pub struct SinkPath {
    /// The sink function (e.g., dynamic_field::borrow_child_object)
    pub sink: FunctionKey,
    /// The kind of dynamic field access at the sink
    pub access_kind: DynamicFieldAccessKind,
    /// Type parameter indices from the ORIGINAL caller that affect the key type
    pub key_type_params: Vec<usize>,
    /// Pattern for the key type (may contain T0, T1, etc.)
    pub key_type_pattern: String,
    /// Pattern for the value type
    pub value_type_pattern: String,
    /// Number of hops to reach the sink
    pub depth: usize,
}

/// Kind of dynamic field access operation (re-exported from bytecode_analyzer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicFieldAccessKind {
    Borrow,
    BorrowMut,
    Add,
    Remove,
    Exists,
    FieldInfo,
}

impl DynamicFieldAccessKind {
    pub fn is_mutating(&self) -> bool {
        matches!(self, Self::BorrowMut | Self::Add | Self::Remove)
    }
}

/// A call edge in the graph.
#[derive(Debug, Clone)]
pub struct CallEdge {
    /// The callee function
    pub callee: FunctionKey,
    /// Type parameter mapping from caller to callee
    pub type_mapping: TypeParamMapping,
    /// Instruction index where the call occurs
    pub instruction_index: usize,
}

/// Complete call graph with sink propagation.
pub struct CallGraph {
    /// Forward edges: caller -> list of callees with type mappings
    calls: HashMap<FunctionKey, Vec<CallEdge>>,
    /// Reverse edges: callee -> list of callers
    callers: HashMap<FunctionKey, Vec<FunctionKey>>,
    /// Known dynamic_field sinks with their access patterns
    dynamic_field_sinks: HashMap<FunctionKey, SinkInfo>,
    /// Functions that transitively reach sinks, with paths
    transitive_sinks: HashMap<FunctionKey, Vec<SinkPath>>,
    /// All loaded modules for type resolution
    modules: HashMap<(AccountAddress, String), CompiledModule>,
}

/// Information about a dynamic_field sink function.
#[derive(Debug, Clone)]
struct SinkInfo {
    access_kind: DynamicFieldAccessKind,
    /// Which type parameter is the key (usually 0)
    key_type_param_index: usize,
    /// Which type parameter is the value (usually 1)
    value_type_param_index: usize,
}

// Well-known addresses
const SUI_FRAMEWORK_ADDR: &str = "0x0000000000000000000000000000000000000000000000000000000000000002";

impl Default for CallGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl CallGraph {
    /// Create a new empty call graph.
    pub fn new() -> Self {
        Self {
            calls: HashMap::new(),
            callers: HashMap::new(),
            dynamic_field_sinks: HashMap::new(),
            transitive_sinks: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    /// Load a compiled module into the graph.
    ///
    /// This extracts all call edges from the module's functions.
    pub fn load_module(&mut self, module: CompiledModule) {
        let package_addr = *module.self_id().address();
        let module_name = module.self_id().name().to_string();

        // Process each function definition
        for (_func_def_idx, func_def) in module.function_defs().iter().enumerate() {
            let func_handle = &module.function_handles()[func_def.function.0 as usize];
            let func_name = module.identifier_at(func_handle.name).to_string();

            let caller_key = FunctionKey::new(package_addr, module_name.clone(), func_name);

            // Extract call edges from bytecode
            if let Some(code) = &func_def.code {
                for (instr_idx, instruction) in code.code.iter().enumerate() {
                    if let Some(edge) = self.extract_call_edge(&module, instruction, instr_idx) {
                        // Add forward edge
                        self.calls
                            .entry(caller_key.clone())
                            .or_default()
                            .push(edge.clone());

                        // Add reverse edge
                        self.callers
                            .entry(edge.callee.clone())
                            .or_default()
                            .push(caller_key.clone());

                        // Check if this is a dynamic_field sink
                        self.check_and_register_sink(&edge.callee);
                    }
                }
            }
        }

        // Store module for later type resolution
        self.modules.insert((package_addr, module_name), module);
    }

    /// Load multiple modules.
    pub fn load_modules(&mut self, modules: Vec<CompiledModule>) {
        for module in modules {
            self.load_module(module);
        }
    }

    /// Extract a call edge from an instruction.
    fn extract_call_edge(
        &self,
        module: &CompiledModule,
        instruction: &Bytecode,
        instr_idx: usize,
    ) -> Option<CallEdge> {
        match instruction {
            Bytecode::Call(func_handle_idx) => {
                let callee = self.resolve_callee(module, *func_handle_idx)?;
                Some(CallEdge {
                    callee,
                    type_mapping: TypeParamMapping { mappings: vec![] },
                    instruction_index: instr_idx,
                })
            }
            Bytecode::CallGeneric(func_inst_idx) => {
                let func_inst = &module.function_instantiations()[func_inst_idx.0 as usize];
                let callee = self.resolve_callee(module, func_inst.handle)?;
                let type_mapping = self.extract_type_mapping(module, func_inst.type_parameters);
                Some(CallEdge {
                    callee,
                    type_mapping,
                    instruction_index: instr_idx,
                })
            }
            _ => None,
        }
    }

    /// Resolve a function handle to a FunctionKey.
    fn resolve_callee(
        &self,
        module: &CompiledModule,
        func_handle_idx: move_binary_format::file_format::FunctionHandleIndex,
    ) -> Option<FunctionKey> {
        let func_handle = &module.function_handles()[func_handle_idx.0 as usize];
        let module_handle = &module.module_handles()[func_handle.module.0 as usize];

        let callee_addr = *module.address_identifier_at(module_handle.address);
        let callee_module = module.identifier_at(module_handle.name).to_string();
        let callee_func = module.identifier_at(func_handle.name).to_string();

        Some(FunctionKey::new(callee_addr, callee_module, callee_func))
    }

    /// Extract type parameter mapping from a generic call.
    fn extract_type_mapping(
        &self,
        module: &CompiledModule,
        sig_idx: SignatureIndex,
    ) -> TypeParamMapping {
        let signature = &module.signatures()[sig_idx.0 as usize];
        let mappings: Vec<TypeParamResolution> = signature
            .0
            .iter()
            .map(|token| self.resolve_type_param(module, token))
            .collect();

        TypeParamMapping { mappings }
    }

    /// Resolve a signature token to a type parameter resolution.
    fn resolve_type_param(
        &self,
        module: &CompiledModule,
        token: &SignatureToken,
    ) -> TypeParamResolution {
        match token {
            SignatureToken::TypeParameter(idx) => TypeParamResolution::CallerTypeParam(*idx as usize),
            _ => {
                let type_str = self.format_signature_token(module, token);
                TypeParamResolution::Concrete(type_str)
            }
        }
    }

    /// Format a signature token as a type string.
    fn format_signature_token(&self, module: &CompiledModule, token: &SignatureToken) -> String {
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
                format!("vector<{}>", self.format_signature_token(module, inner))
            }
            SignatureToken::Datatype(idx) => self.format_datatype_handle(module, *idx),
            SignatureToken::DatatypeInstantiation(inst) => {
                let (idx, type_args) = inst.as_ref();
                let base = self.format_datatype_handle(module, *idx);
                let args: Vec<String> = type_args
                    .iter()
                    .map(|t| self.format_signature_token(module, t))
                    .collect();
                format!("{}<{}>", base, args.join(", "))
            }
            SignatureToken::Reference(inner) => {
                format!("&{}", self.format_signature_token(module, inner))
            }
            SignatureToken::MutableReference(inner) => {
                format!("&mut {}", self.format_signature_token(module, inner))
            }
            SignatureToken::TypeParameter(idx) => format!("T{}", idx),
        }
    }

    /// Format a datatype handle as a fully qualified type string.
    fn format_datatype_handle(
        &self,
        module: &CompiledModule,
        idx: move_binary_format::file_format::DatatypeHandleIndex,
    ) -> String {
        let datatype_handle = module.datatype_handle_at(idx);
        let module_handle = module.module_handle_at(datatype_handle.module);

        let addr = module.address_identifier_at(module_handle.address);
        let module_name = module.identifier_at(module_handle.name);
        let datatype_name = module.identifier_at(datatype_handle.name);

        format!("{:#066x}::{}::{}", addr, module_name, datatype_name)
    }

    /// Check if a function is a known dynamic_field sink and register it.
    fn check_and_register_sink(&mut self, callee: &FunctionKey) {
        let addr_str = format!("{:#066x}", callee.package);
        if addr_str != SUI_FRAMEWORK_ADDR {
            return;
        }

        if callee.module != "dynamic_field" && callee.module != "dynamic_object_field" {
            return;
        }

        let (access_kind, key_idx, val_idx) = match callee.function.as_str() {
            "borrow_child_object" | "borrow" => (DynamicFieldAccessKind::Borrow, 0, 1),
            "borrow_child_object_mut" | "borrow_mut" => (DynamicFieldAccessKind::BorrowMut, 0, 1),
            "add_child_object" | "add" => (DynamicFieldAccessKind::Add, 0, 1),
            "remove_child_object" | "remove" => (DynamicFieldAccessKind::Remove, 0, 1),
            "has_child_object" | "has_child_object_with_ty" | "exists_" | "exists_with_type" => {
                (DynamicFieldAccessKind::Exists, 0, 1)
            }
            "field_info" | "field_info_mut" => (DynamicFieldAccessKind::FieldInfo, 0, 1),
            _ => return,
        };

        self.dynamic_field_sinks.insert(
            callee.clone(),
            SinkInfo {
                access_kind,
                key_type_param_index: key_idx,
                value_type_param_index: val_idx,
            },
        );
    }

    /// Propagate sink status backwards through the call graph using BFS.
    ///
    /// After this, `transitive_sinks` will contain all functions that can
    /// reach a dynamic_field sink, along with the paths to those sinks.
    pub fn propagate_sinks(&mut self) {
        // Start with direct callers of sinks
        let mut queue: VecDeque<(FunctionKey, SinkPath)> = VecDeque::new();

        // Initialize with direct sink callers
        for (sink_key, sink_info) in &self.dynamic_field_sinks {
            let path = SinkPath {
                sink: sink_key.clone(),
                access_kind: sink_info.access_kind,
                key_type_params: vec![sink_info.key_type_param_index],
                key_type_pattern: format!("T{}", sink_info.key_type_param_index),
                value_type_pattern: format!("T{}", sink_info.value_type_param_index),
                depth: 0,
            };

            // The sink itself reaches itself at depth 0
            self.transitive_sinks
                .entry(sink_key.clone())
                .or_default()
                .push(path.clone());

            // Queue all direct callers
            if let Some(callers) = self.callers.get(sink_key) {
                for caller in callers {
                    let caller_path = SinkPath {
                        depth: 1,
                        ..path.clone()
                    };
                    queue.push_back((caller.clone(), caller_path));
                }
            }
        }

        // BFS propagation
        let max_depth = 10; // Prevent infinite loops
        while let Some((func_key, path)) = queue.pop_front() {
            if path.depth > max_depth {
                continue;
            }

            // Check if we already have a shorter path to this sink
            let dominated = self
                .transitive_sinks
                .get(&func_key)
                .map(|paths| paths.iter().any(|p| p.sink == path.sink && p.depth <= path.depth))
                .unwrap_or(false);

            if dominated {
                continue;
            }

            // Add this path (before remapping - the path represents the callee's perspective)
            self.transitive_sinks
                .entry(func_key.clone())
                .or_default()
                .push(path.clone());

            // Queue callers with remapped type patterns
            // We need to find which edge from caller leads to func_key and use its type mapping
            if let Some(callers) = self.callers.get(&func_key).cloned() {
                for caller in callers {
                    // Find the edge from caller to func_key to get type mapping
                    let type_mapping = self.calls.get(&caller)
                        .and_then(|edges| {
                            edges.iter()
                                .find(|e| e.callee == func_key)
                                .map(|e| &e.type_mapping)
                        });

                    // Create caller's path with remapped types
                    let caller_path = if let Some(mapping) = type_mapping {
                        SinkPath {
                            sink: path.sink.clone(),
                            access_kind: path.access_kind,
                            key_type_params: self.remap_type_params(&path.key_type_params, mapping),
                            key_type_pattern: self.remap_type_pattern(&path.key_type_pattern, mapping),
                            value_type_pattern: self.remap_type_pattern(&path.value_type_pattern, mapping),
                            depth: path.depth + 1,
                        }
                    } else {
                        SinkPath {
                            depth: path.depth + 1,
                            ..path.clone()
                        }
                    };

                    queue.push_back((caller, caller_path));
                }
            }
        }
    }

    /// Remap type parameter indices through a call edge.
    fn remap_type_params(
        &self,
        params: &[usize],
        mapping: &TypeParamMapping,
    ) -> Vec<usize> {
        params
            .iter()
            .filter_map(|&idx| {
                if idx < mapping.mappings.len() {
                    match &mapping.mappings[idx] {
                        TypeParamResolution::CallerTypeParam(caller_idx) => Some(*caller_idx),
                        _ => None, // Concrete types don't propagate
                    }
                } else {
                    Some(idx) // No mapping, keep original
                }
            })
            .collect()
    }

    /// Remap type pattern through a call edge.
    fn remap_type_pattern(&self, pattern: &str, mapping: &TypeParamMapping) -> String {
        let mut result = pattern.to_string();

        // Replace type parameters in reverse order (T10 before T1)
        for (idx, resolution) in mapping.mappings.iter().enumerate().rev() {
            let placeholder = format!("T{}", idx);
            let replacement = match resolution {
                TypeParamResolution::Concrete(s) => s.clone(),
                TypeParamResolution::CallerTypeParam(caller_idx) => format!("T{}", caller_idx),
                TypeParamResolution::Unknown => placeholder.clone(),
            };
            result = result.replace(&placeholder, &replacement);
        }

        result
    }

    /// Check if a function reaches any dynamic_field sink.
    pub fn reaches_sink(&self, func_key: &FunctionKey) -> bool {
        self.transitive_sinks.contains_key(func_key)
    }

    /// Get sink paths for a function.
    pub fn get_sink_paths(&self, func_key: &FunctionKey) -> Option<&Vec<SinkPath>> {
        self.transitive_sinks.get(func_key)
    }

    /// Predict dynamic field accesses for a function call with concrete type args.
    ///
    /// This is the main entry point for predictions using the call graph.
    pub fn predict_accesses(
        &self,
        package: &AccountAddress,
        module: &str,
        function: &str,
        type_args: &[String],
    ) -> Vec<ResolvedAccess> {
        let func_key = FunctionKey::new(*package, module, function);

        let paths = match self.get_sink_paths(&func_key) {
            Some(paths) => paths,
            None => return vec![],
        };

        let mut accesses = Vec::new();

        for path in paths {
            // Resolve type parameters using concrete type_args
            let resolved_key = resolve_pattern(&path.key_type_pattern, type_args);
            let resolved_value = resolve_pattern(&path.value_type_pattern, type_args);

            let confidence = if has_unresolved_params(&resolved_key) {
                AccessConfidence::Medium
            } else {
                AccessConfidence::High
            };

            accesses.push(ResolvedAccess {
                key_type: resolved_key,
                value_type: resolved_value,
                access_kind: path.access_kind,
                confidence,
                sink_depth: path.depth,
            });
        }

        // Deduplicate by key type
        deduplicate_accesses(accesses)
    }

    /// Get statistics about the call graph.
    pub fn stats(&self) -> CallGraphStats {
        CallGraphStats {
            modules_loaded: self.modules.len(),
            functions_tracked: self.calls.len(),
            call_edges: self.calls.values().map(|v| v.len()).sum(),
            direct_sinks: self.dynamic_field_sinks.len(),
            transitive_sink_functions: self.transitive_sinks.len(),
        }
    }
}

/// A resolved dynamic field access prediction.
#[derive(Debug, Clone)]
pub struct ResolvedAccess {
    /// Fully resolved key type
    pub key_type: String,
    /// Fully resolved value type
    pub value_type: String,
    /// Access kind
    pub access_kind: DynamicFieldAccessKind,
    /// Confidence level
    pub confidence: AccessConfidence,
    /// How many call hops to reach the actual dynamic_field call
    pub sink_depth: usize,
}

/// Confidence level for a resolved access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessConfidence {
    Low = 0,
    Medium = 1,
    High = 2,
}

/// Statistics about the call graph.
#[derive(Debug, Clone)]
pub struct CallGraphStats {
    pub modules_loaded: usize,
    pub functions_tracked: usize,
    pub call_edges: usize,
    pub direct_sinks: usize,
    pub transitive_sink_functions: usize,
}

/// Resolve type parameters in a pattern.
fn resolve_pattern(pattern: &str, type_args: &[String]) -> String {
    let mut result = pattern.to_string();
    for (i, arg) in type_args.iter().enumerate().rev() {
        let placeholder = format!("T{}", i);
        result = result.replace(&placeholder, arg);
    }
    result
}

/// Check if a type string has unresolved parameters.
fn has_unresolved_params(type_str: &str) -> bool {
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

/// Deduplicate accesses by key type, keeping highest confidence.
fn deduplicate_accesses(accesses: Vec<ResolvedAccess>) -> Vec<ResolvedAccess> {
    let mut by_key: HashMap<String, ResolvedAccess> = HashMap::new();

    for access in accesses {
        let key = access.key_type.clone();
        match by_key.get(&key) {
            Some(existing) if existing.confidence >= access.confidence => {}
            _ => {
                by_key.insert(key, access);
            }
        }
    }

    by_key.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_key_framework_detection() {
        let framework_1 = FunctionKey::new(
            AccountAddress::from_hex_literal("0x1").unwrap(),
            "module",
            "func",
        );
        let framework_2 = FunctionKey::new(
            AccountAddress::from_hex_literal("0x2").unwrap(),
            "module",
            "func",
        );
        let user = FunctionKey::new(
            AccountAddress::from_hex_literal("0xabc123").unwrap(),
            "module",
            "func",
        );

        assert!(framework_1.is_framework());
        assert!(framework_2.is_framework());
        assert!(!user.is_framework());
    }

    #[test]
    fn test_resolve_pattern() {
        assert_eq!(
            resolve_pattern("Key<T0>", &["u64".to_string()]),
            "Key<u64>"
        );
        assert_eq!(
            resolve_pattern("Pair<T0, T1>", &["A".to_string(), "B".to_string()]),
            "Pair<A, B>"
        );
        assert_eq!(
            resolve_pattern("NoParams", &[]),
            "NoParams"
        );
    }

    #[test]
    fn test_has_unresolved_params() {
        assert!(has_unresolved_params("Key<T0>"));
        assert!(has_unresolved_params("T1"));
        assert!(!has_unresolved_params("Key<u64>"));
        assert!(!has_unresolved_params("TOKEN"));
    }

    #[test]
    fn test_call_graph_stats_empty() {
        let graph = CallGraph::new();
        let stats = graph.stats();
        assert_eq!(stats.modules_loaded, 0);
        assert_eq!(stats.functions_tracked, 0);
        assert_eq!(stats.direct_sinks, 0);
    }

    #[test]
    fn test_access_confidence_ordering() {
        assert!(AccessConfidence::High > AccessConfidence::Medium);
        assert!(AccessConfidence::Medium > AccessConfidence::Low);
    }
}
