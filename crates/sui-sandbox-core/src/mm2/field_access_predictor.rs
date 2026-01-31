//! Predicts dynamic field accesses for PTB commands.
//!
//! This module combines bytecode analysis with type resolution to predict which
//! dynamic fields a MoveCall will access at runtime. This enables precise data
//! prefetching instead of expensive enumeration.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let predictor = FieldAccessPredictor::new();
//!
//! // Load modules for the packages being called
//! predictor.load_modules(package_addr, modules)?;
//!
//! // Predict accesses for a MoveCall
//! let predictions = predictor.predict_accesses(
//!     &package_addr,
//!     "balance_manager",
//!     "withdraw_with_proof",
//!     &["0x5d4b44::deep::DEEP".to_string()],
//! );
//!
//! for pred in predictions {
//!     println!("Will access: {} -> {}", pred.key_type, pred.value_type);
//! }
//! ```

use super::bytecode_analyzer::{
    has_unresolved_params, resolve_type_pattern, BytecodeAnalyzer, DynamicFieldAccessKind,
    FunctionAccessAnalysis,
};
use super::call_graph::CallGraph;
use move_binary_format::file_format::FunctionDefinitionIndex;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use std::collections::{HashMap, HashSet};

/// A concrete prediction of dynamic field access.
#[derive(Debug, Clone)]
pub struct PredictedAccess {
    /// Fully resolved key type (e.g., "0x91bfbc::deepbook::BalanceKey<0x5d4b44::deep::DEEP>")
    pub key_type: String,
    /// Fully resolved value type
    pub value_type: String,
    /// Access kind (borrow, borrow_mut, add, remove, etc.)
    pub kind: DynamicFieldAccessKind,
    /// Confidence level of the prediction
    pub confidence: Confidence,
    /// Source function that performs this access
    pub source_function: String,
    /// Whether this is a direct or transitive access
    pub is_transitive: bool,
}

/// Confidence level for a prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Heuristic guess or transitive with uncertainty
    Low = 0,
    /// Some type parameters unresolved but pattern known
    Medium = 1,
    /// All type parameters fully resolved
    High = 2,
}

/// Predicts dynamic field accesses for function calls.
pub struct FieldAccessPredictor {
    /// Bytecode analyzer for extracting patterns
    analyzer: BytecodeAnalyzer,
    /// Loaded modules by (package_addr, module_name)
    modules: HashMap<(AccountAddress, String), CompiledModule>,
    /// Cache of function analyses by (package_addr, module_name, function_name)
    analysis_cache: HashMap<(AccountAddress, String, String), FunctionAccessAnalysis>,
    /// Maximum depth for transitive analysis
    max_transitive_depth: usize,
    /// Optional call graph for enhanced transitive analysis
    call_graph: Option<CallGraph>,
    /// Whether call graph sink propagation has been run
    call_graph_propagated: bool,
}

impl Default for FieldAccessPredictor {
    fn default() -> Self {
        Self::new()
    }
}

impl FieldAccessPredictor {
    /// Create a new predictor with default settings.
    pub fn new() -> Self {
        Self {
            analyzer: BytecodeAnalyzer::new(),
            modules: HashMap::new(),
            analysis_cache: HashMap::new(),
            max_transitive_depth: 3,
            call_graph: None,
            call_graph_propagated: false,
        }
    }

    /// Create a predictor with custom transitive depth limit.
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            analyzer: BytecodeAnalyzer::new(),
            modules: HashMap::new(),
            analysis_cache: HashMap::new(),
            max_transitive_depth: max_depth,
            call_graph: None,
            call_graph_propagated: false,
        }
    }

    /// Create a predictor with call graph enabled for enhanced transitive analysis.
    ///
    /// The call graph approach traces through all function calls to find paths
    /// to dynamic_field operations, even through wrapper functions like
    /// `table::borrow` or `bag::add`.
    pub fn with_call_graph() -> Self {
        Self {
            analyzer: BytecodeAnalyzer::new(),
            modules: HashMap::new(),
            analysis_cache: HashMap::new(),
            max_transitive_depth: 10, // Higher depth with call graph
            call_graph: Some(CallGraph::new()),
            call_graph_propagated: false,
        }
    }

    /// Enable or disable call graph mode.
    pub fn set_call_graph_enabled(&mut self, enabled: bool) {
        if enabled && self.call_graph.is_none() {
            let mut graph = CallGraph::new();
            // Load existing modules into the graph
            for ((_addr, _name), module) in &self.modules {
                graph.load_module(module.clone());
            }
            self.call_graph = Some(graph);
            self.call_graph_propagated = false;
        } else if !enabled {
            self.call_graph = None;
            self.call_graph_propagated = false;
        }
    }

    /// Check if call graph mode is enabled.
    pub fn is_call_graph_enabled(&self) -> bool {
        self.call_graph.is_some()
    }

    /// Load compiled modules for a package.
    ///
    /// Modules must be loaded before predicting accesses for functions in that package.
    pub fn load_modules(
        &mut self,
        package_addr: AccountAddress,
        modules: Vec<CompiledModule>,
    ) -> Result<(), String> {
        for module in modules {
            let module_name = module.self_id().name().to_string();
            // Also load into call graph if enabled
            if let Some(ref mut graph) = self.call_graph {
                graph.load_module(module.clone());
                self.call_graph_propagated = false; // Need to re-propagate
            }
            self.modules.insert((package_addr, module_name), module);
        }
        Ok(())
    }

    /// Load a single compiled module.
    pub fn load_module(&mut self, module: CompiledModule) {
        let package_addr = *module.self_id().address();
        let module_name = module.self_id().name().to_string();
        // Also load into call graph if enabled
        if let Some(ref mut graph) = self.call_graph {
            graph.load_module(module.clone());
            self.call_graph_propagated = false; // Need to re-propagate
        }
        self.modules.insert((package_addr, module_name), module);
    }

    /// Check if a module is loaded.
    pub fn has_module(&self, package_addr: &AccountAddress, module_name: &str) -> bool {
        self.modules
            .contains_key(&(*package_addr, module_name.to_string()))
    }

    /// Clear all loaded modules and caches.
    pub fn clear(&mut self) {
        self.modules.clear();
        self.analysis_cache.clear();
        self.analyzer.clear_cache();
        if self.call_graph.is_some() {
            self.call_graph = Some(CallGraph::new());
            self.call_graph_propagated = false;
        }
    }

    /// Ensure call graph sink propagation has been performed.
    ///
    /// This should be called after all modules are loaded and before predictions.
    /// It's automatically called by `predict_accesses` if needed.
    pub fn ensure_propagated(&mut self) {
        if let Some(ref mut graph) = self.call_graph {
            if !self.call_graph_propagated {
                graph.propagate_sinks();
                self.call_graph_propagated = true;
            }
        }
    }

    /// Predict dynamic field accesses for a MoveCall.
    ///
    /// # Arguments
    /// * `package_addr` - Package address of the function
    /// * `module_name` - Module containing the function
    /// * `function_name` - Function name
    /// * `type_args` - Concrete type arguments from the PTB
    ///
    /// # Returns
    /// List of predicted dynamic field accesses with resolved types.
    pub fn predict_accesses(
        &mut self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
        type_args: &[String],
    ) -> Vec<PredictedAccess> {
        // If call graph is enabled, use it for enhanced transitive analysis
        if self.call_graph.is_some() {
            self.ensure_propagated();
            return self.predict_with_call_graph(
                package_addr,
                module_name,
                function_name,
                type_args,
            );
        }

        // Fall back to original recursive analysis
        let mut predictions = Vec::new();
        let mut visited = HashSet::new();

        self.predict_accesses_recursive(
            package_addr,
            module_name,
            function_name,
            type_args,
            0,
            &mut predictions,
            &mut visited,
        );

        predictions
    }

    /// Predict accesses using the call graph (enhanced transitive analysis).
    fn predict_with_call_graph(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
        type_args: &[String],
    ) -> Vec<PredictedAccess> {
        let graph = match &self.call_graph {
            Some(g) => g,
            None => return vec![],
        };

        let accesses = graph.predict_accesses(package_addr, module_name, function_name, type_args);

        // Convert ResolvedAccess to PredictedAccess
        accesses
            .into_iter()
            .map(|access| {
                let confidence = match access.confidence {
                    super::call_graph::AccessConfidence::High => Confidence::High,
                    super::call_graph::AccessConfidence::Medium => Confidence::Medium,
                    super::call_graph::AccessConfidence::Low => Confidence::Low,
                };
                PredictedAccess {
                    key_type: access.key_type,
                    value_type: access.value_type,
                    kind: convert_access_kind(access.access_kind),
                    confidence,
                    source_function: format!(
                        "{}::{}::{}",
                        package_addr, module_name, function_name
                    ),
                    is_transitive: access.sink_depth > 0,
                }
            })
            .collect()
    }

    /// Recursive helper for transitive analysis.
    fn predict_accesses_recursive(
        &mut self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
        type_args: &[String],
        depth: usize,
        predictions: &mut Vec<PredictedAccess>,
        visited: &mut HashSet<(AccountAddress, String, String)>,
    ) {
        // Prevent infinite recursion
        let func_key = (
            *package_addr,
            module_name.to_string(),
            function_name.to_string(),
        );
        if visited.contains(&func_key) || depth > self.max_transitive_depth {
            return;
        }
        visited.insert(func_key.clone());

        // Get or compute the analysis for this function
        let analysis = self.get_or_analyze(package_addr, module_name, function_name);
        let analysis = match analysis {
            Some(a) => a,
            None => return, // Module not loaded
        };

        // Process direct dynamic field accesses
        for pattern in &analysis.dynamic_field_accesses {
            let resolved_key = resolve_type_pattern(&pattern.key_type_pattern, type_args);
            let resolved_value = resolve_type_pattern(&pattern.value_type_pattern, type_args);

            let confidence = if has_unresolved_params(&resolved_key) {
                Confidence::Medium
            } else {
                Confidence::High
            };

            predictions.push(PredictedAccess {
                key_type: resolved_key,
                value_type: resolved_value,
                kind: pattern.access_kind,
                confidence,
                source_function: format!("{}::{}::{}", package_addr, module_name, function_name),
                is_transitive: depth > 0,
            });
        }

        // Process transitive calls
        let called_functions = analysis.called_functions.clone();
        for (callee_addr, callee_module, callee_func) in called_functions {
            // Skip framework functions (we handle dynamic_field directly)
            if is_framework_address(&callee_addr) {
                continue;
            }

            // Recurse into called functions
            // Note: type_args propagation is simplified here - in a full implementation
            // we'd need to track how type params flow through the call
            self.predict_accesses_recursive(
                &callee_addr,
                &callee_module,
                &callee_func,
                type_args, // Simplified: pass through same type args
                depth + 1,
                predictions,
                visited,
            );
        }
    }

    /// Get cached analysis or compute it.
    fn get_or_analyze(
        &mut self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> Option<FunctionAccessAnalysis> {
        let cache_key = (
            *package_addr,
            module_name.to_string(),
            function_name.to_string(),
        );

        // Check cache first
        if let Some(cached) = self.analysis_cache.get(&cache_key) {
            return Some(cached.clone());
        }

        // Get the module
        let module_key = (*package_addr, module_name.to_string());
        let module = self.modules.get(&module_key)?;

        // Find the function definition
        let func_def_idx = find_function_def_index(module, function_name)?;

        // Analyze the function
        let analysis = self.analyzer.analyze_function(module, func_def_idx);

        // Cache and return
        self.analysis_cache.insert(cache_key, analysis.clone());
        Some(analysis)
    }

    /// Get statistics about loaded modules and cached analyses.
    pub fn stats(&self) -> PredictorStats {
        let call_graph_stats = self.call_graph.as_ref().map(|g| g.stats());
        PredictorStats {
            modules_loaded: self.modules.len(),
            analyses_cached: self.analysis_cache.len(),
            call_graph_enabled: self.call_graph.is_some(),
            call_graph_functions: call_graph_stats
                .as_ref()
                .map(|s| s.functions_tracked)
                .unwrap_or(0),
            call_graph_sinks: call_graph_stats
                .as_ref()
                .map(|s| s.direct_sinks)
                .unwrap_or(0),
            call_graph_transitive: call_graph_stats
                .map(|s| s.transitive_sink_functions)
                .unwrap_or(0),
        }
    }
}

/// Convert call_graph::DynamicFieldAccessKind to bytecode_analyzer::DynamicFieldAccessKind
fn convert_access_kind(kind: super::call_graph::DynamicFieldAccessKind) -> DynamicFieldAccessKind {
    match kind {
        super::call_graph::DynamicFieldAccessKind::Borrow => DynamicFieldAccessKind::Borrow,
        super::call_graph::DynamicFieldAccessKind::BorrowMut => DynamicFieldAccessKind::BorrowMut,
        super::call_graph::DynamicFieldAccessKind::Add => DynamicFieldAccessKind::Add,
        super::call_graph::DynamicFieldAccessKind::Remove => DynamicFieldAccessKind::Remove,
        super::call_graph::DynamicFieldAccessKind::Exists => DynamicFieldAccessKind::Exists,
        super::call_graph::DynamicFieldAccessKind::FieldInfo => DynamicFieldAccessKind::FieldInfo,
    }
}

/// Statistics about predictor state.
#[derive(Debug, Clone)]
pub struct PredictorStats {
    pub modules_loaded: usize,
    pub analyses_cached: usize,
    /// Whether call graph mode is enabled
    pub call_graph_enabled: bool,
    /// Number of functions tracked in call graph
    pub call_graph_functions: usize,
    /// Number of direct dynamic_field sinks found
    pub call_graph_sinks: usize,
    /// Number of functions that transitively reach sinks
    pub call_graph_transitive: usize,
}

/// Find the function definition index by name.
fn find_function_def_index(
    module: &CompiledModule,
    function_name: &str,
) -> Option<FunctionDefinitionIndex> {
    for (idx, func_def) in module.function_defs().iter().enumerate() {
        let func_handle = &module.function_handles()[func_def.function.0 as usize];
        let name = module.identifier_at(func_handle.name);
        if name.as_str() == function_name {
            return Some(FunctionDefinitionIndex(idx as u16));
        }
    }
    None
}

/// Check if an address is a Sui framework address.
fn is_framework_address(addr: &AccountAddress) -> bool {
    sui_resolver::is_framework_account_address(addr)
}

/// Analyze a batch of MoveCall commands and predict all dynamic field accesses.
///
/// This is the main entry point for PTB analysis.
pub fn predict_accesses_for_calls(
    predictor: &mut FieldAccessPredictor,
    calls: &[(AccountAddress, String, String, Vec<String>)], // (pkg, module, func, type_args)
) -> Vec<PredictedAccess> {
    let mut all_predictions = Vec::new();

    for (pkg, module, func, type_args) in calls {
        let predictions = predictor.predict_accesses(pkg, module, func, type_args);
        all_predictions.extend(predictions);
    }

    // Deduplicate by key type (keeping highest confidence)
    deduplicate_predictions(all_predictions)
}

/// Deduplicate predictions, keeping the highest confidence for each key type.
fn deduplicate_predictions(predictions: Vec<PredictedAccess>) -> Vec<PredictedAccess> {
    let mut by_key: HashMap<String, PredictedAccess> = HashMap::new();

    for pred in predictions {
        let key = pred.key_type.clone();
        match by_key.get(&key) {
            Some(existing) if existing.confidence >= pred.confidence => {
                // Keep existing higher confidence prediction
            }
            _ => {
                by_key.insert(key, pred);
            }
        }
    }

    by_key.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_framework_address() {
        let addr_1 = AccountAddress::from_hex_literal("0x1").unwrap();
        let addr_2 = AccountAddress::from_hex_literal("0x2").unwrap();
        let addr_3 = AccountAddress::from_hex_literal("0x3").unwrap();
        let addr_user = AccountAddress::from_hex_literal("0xabc123").unwrap();

        assert!(is_framework_address(&addr_1));
        assert!(is_framework_address(&addr_2));
        assert!(is_framework_address(&addr_3));
        assert!(!is_framework_address(&addr_user));
    }

    #[test]
    fn test_predictor_stats() {
        let predictor = FieldAccessPredictor::new();
        let stats = predictor.stats();
        assert_eq!(stats.modules_loaded, 0);
        assert_eq!(stats.analyses_cached, 0);
        assert!(!stats.call_graph_enabled);
    }

    #[test]
    fn test_predictor_with_call_graph() {
        let predictor = FieldAccessPredictor::with_call_graph();
        let stats = predictor.stats();
        assert!(stats.call_graph_enabled);
        assert_eq!(stats.call_graph_functions, 0);
        assert_eq!(stats.call_graph_sinks, 0);
    }

    #[test]
    fn test_confidence_ordering() {
        // High > Medium > Low for deduplication
        assert!((Confidence::High as u8) > (Confidence::Medium as u8));
        assert!((Confidence::Medium as u8) > (Confidence::Low as u8));
    }

    #[test]
    fn test_deduplicate_keeps_highest_confidence() {
        let predictions = vec![
            PredictedAccess {
                key_type: "Key<A>".to_string(),
                value_type: "Value".to_string(),
                kind: DynamicFieldAccessKind::Borrow,
                confidence: Confidence::Low,
                source_function: "a::b::c".to_string(),
                is_transitive: true,
            },
            PredictedAccess {
                key_type: "Key<A>".to_string(),
                value_type: "Value".to_string(),
                kind: DynamicFieldAccessKind::Borrow,
                confidence: Confidence::High,
                source_function: "a::b::d".to_string(),
                is_transitive: false,
            },
        ];

        let deduped = deduplicate_predictions(predictions);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].confidence, Confidence::High);
    }
}
