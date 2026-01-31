//! # Predictive Data Prefetching
//!
//! Internal infrastructure for the transaction replay pipeline. This module orchestrates
//! data fetching to ensure all required objects are available before execution.
//!
//! ## Role in the System
//!
//! This is **Layer 2** of the three-layer prefetch pipeline:
//!
//! ```text
//! Layer 1: Ground Truth Prefetch (eager_prefetch.rs)
//!    └─ Fetches objects listed in transaction effects
//!    └─ Coverage: ~60-80% of required objects
//!    └─ Fast and reliable
//!
//! Layer 2: Predictive Prefetch (THIS MODULE)
//!    └─ Analyzes bytecode to predict dynamic field accesses
//!    └─ Catches accesses through wrapper functions (table::borrow, etc.)
//!    └─ Coverage: Improves to ~85-95% for complex DeFi transactions
//!
//! Layer 3: On-Demand Fetch (object_runtime.rs)
//!    └─ Fallback during execution for any missed objects
//!    └─ Slow but catches everything
//! ```
//!
//! ## When to Use
//!
//! - **Simple transactions**: Ground truth alone is usually sufficient
//! - **DeFi protocols**: Enable MM2 prediction for better coverage
//! - **Complex protocols**: Enable call graph analysis for transitive detection
//!
//! ## Strategy
//!
//! 1. **Ground Truth First**: Use `unchanged_loaded_runtime_objects` as the primary source
//! 2. **MM2 Prediction**: Analyze MoveCall bytecode to predict dynamic field accesses
//! 3. **Targeted Fetch**: Fetch predicted dynamic fields by key type
//!
//! ## Example
//!
//! ```rust,ignore
//! use sui_sandbox_core::predictive_prefetch::{
//!     PredictivePrefetcher, PredictivePrefetchConfig,
//! };
//!
//! let config = PredictivePrefetchConfig::with_call_graph(); // Full analysis
//! let mut prefetcher = PredictivePrefetcher::new();
//!
//! let result = prefetcher.prefetch_for_transaction(&grpc, Some(&graphql), &rt, &tx, &config);
//! // result.ground_truth contains objects from tx effects
//! // result.prediction_stats shows MM2 analysis results
//! ```

use crate::mm2::{
    Confidence, DynamicFieldAccessKind, FieldAccessPredictor, KeyValueSynthesizer, PredictedAccess,
};
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use std::collections::{HashMap, HashSet};
use sui_prefetch::{
    ground_truth_prefetch_for_transaction, FetchedPackage, GroundTruthPrefetchConfig,
    GroundTruthPrefetchResult,
};
use sui_resolver::package_upgrades::PackageUpgradeResolver;
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcClient, GrpcTransaction};

/// Configuration for predictive prefetching.
#[derive(Debug, Clone)]
pub struct PredictivePrefetchConfig {
    /// Base configuration for ground-truth prefetch.
    pub base_config: GroundTruthPrefetchConfig,

    /// Whether to enable MM2 bytecode analysis for prediction.
    /// Default: true
    pub enable_mm2_prediction: bool,

    /// Whether to use call graph analysis for enhanced transitive detection.
    ///
    /// When enabled, the predictor builds a complete call graph and propagates
    /// sink status backwards to find all functions that transitively call
    /// dynamic_field operations. This catches accesses through wrappers like
    /// `table::borrow`, `bag::add`, etc.
    ///
    /// Default: false (use original direct-detection mode)
    pub use_call_graph: bool,

    /// Maximum depth for transitive function analysis.
    /// Higher values catch more indirect dynamic field accesses but take longer.
    /// Default: 3 (or 10 when call graph is enabled)
    pub max_transitive_depth: usize,

    /// Minimum confidence level to use a prediction.
    /// Default: Confidence::Medium
    pub min_confidence: Confidence,

    /// Whether to fetch predicted objects even if not in ground truth.
    /// This is useful for forward-looking predictions but may fetch unnecessary data.
    /// Default: false
    pub fetch_predictions_not_in_ground_truth: bool,
}

impl Default for PredictivePrefetchConfig {
    fn default() -> Self {
        Self {
            base_config: GroundTruthPrefetchConfig::default(),
            enable_mm2_prediction: true,
            use_call_graph: false,
            max_transitive_depth: 3,
            min_confidence: Confidence::Medium,
            fetch_predictions_not_in_ground_truth: false,
        }
    }
}

impl PredictivePrefetchConfig {
    /// Create a config with call graph analysis enabled.
    ///
    /// This provides better coverage for dynamic field detection through
    /// wrapper functions but requires building a complete call graph.
    pub fn with_call_graph() -> Self {
        Self {
            base_config: GroundTruthPrefetchConfig::default(),
            enable_mm2_prediction: true,
            use_call_graph: true,
            max_transitive_depth: 10, // Higher depth for call graph mode
            min_confidence: Confidence::Medium,
            fetch_predictions_not_in_ground_truth: false,
        }
    }
}

/// Result of predictive prefetching.
#[derive(Debug, Default)]
pub struct PredictivePrefetchResult {
    /// Base result from ground-truth prefetch.
    pub base_result: GroundTruthPrefetchResult,

    /// Prediction statistics.
    pub prediction_stats: PredictionStats,

    /// Detailed predictions made (for debugging/analysis).
    pub predictions: Vec<PredictedAccessInfo>,
}

/// Statistics about MM2 predictions.
#[derive(Debug, Default, Clone)]
pub struct PredictionStats {
    /// Number of MoveCall commands analyzed.
    pub commands_analyzed: usize,

    /// Number of predictions made.
    pub predictions_made: usize,

    /// Number of predictions that matched ground truth (confirmed useful).
    pub predictions_matched_ground_truth: usize,

    /// Number of predictions matched via synthesized child ID derivation.
    /// This is a subset of predictions_matched_ground_truth where we derived
    /// the exact child ID from the predicted key type (phantom keys).
    pub predictions_matched_via_synthesis: usize,

    /// Number of phantom key predictions (derivable child IDs).
    pub phantom_key_predictions: usize,

    /// Number of high-confidence predictions.
    pub high_confidence_predictions: usize,

    /// Number of medium-confidence predictions.
    pub medium_confidence_predictions: usize,

    /// Number of low-confidence predictions.
    pub low_confidence_predictions: usize,

    /// Packages analyzed via MM2 (from ground truth).
    pub packages_analyzed: usize,

    /// Packages fetched specifically for MM2 analysis (fallback).
    pub packages_fetched_for_mm2: usize,

    /// Functions analyzed via MM2.
    pub functions_analyzed: usize,

    /// MoveCall commands skipped due to missing package bytecode.
    pub commands_skipped_no_bytecode: usize,

    /// Time spent on MM2 analysis (milliseconds).
    pub analysis_time_ms: u64,
}

/// Information about a predicted access (for debugging).
#[derive(Debug, Clone)]
pub struct PredictedAccessInfo {
    /// The predicted key type.
    pub key_type: String,
    /// The predicted value type.
    pub value_type: String,
    /// Access kind (borrow, borrow_mut, add, remove).
    pub kind: DynamicFieldAccessKind,
    /// Confidence level.
    pub confidence: Confidence,
    /// Source function.
    pub source_function: String,
    /// Whether this prediction matched an object in ground truth.
    pub matched_ground_truth: bool,
    /// The matched object ID (if any).
    pub matched_object_id: Option<String>,
    /// Whether this is a phantom key (can derive child ID without runtime value).
    pub is_phantom_key: bool,
    /// Synthesized child ID (if phantom key and parent known).
    pub synthesized_child_id: Option<String>,
}

/// Predictive prefetcher combining ground-truth with MM2 analysis.
pub struct PredictivePrefetcher {
    /// Field access predictor for bytecode analysis.
    predictor: FieldAccessPredictor,
}

impl Default for PredictivePrefetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl PredictivePrefetcher {
    /// Create a new predictive prefetcher.
    pub fn new() -> Self {
        Self {
            predictor: FieldAccessPredictor::new(),
        }
    }

    /// Create with custom transitive depth.
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            predictor: FieldAccessPredictor::with_max_depth(max_depth),
        }
    }

    /// Create with call graph enabled for enhanced transitive analysis.
    ///
    /// The call graph approach traces through all function calls to find paths
    /// to dynamic_field operations, even through wrapper functions like
    /// `table::borrow`, `bag::add`, etc.
    ///
    /// This provides significantly better coverage than the basic approach,
    /// which only detects direct `0x2::dynamic_field::*` calls.
    pub fn with_call_graph() -> Self {
        Self {
            predictor: FieldAccessPredictor::with_call_graph(),
        }
    }

    /// Enable or disable call graph mode.
    ///
    /// When enabled, the predictor will build a complete call graph and use
    /// sink propagation to find all functions that transitively access
    /// dynamic fields.
    pub fn set_call_graph_enabled(&mut self, enabled: bool) {
        self.predictor.set_call_graph_enabled(enabled);
    }

    /// Check if call graph mode is enabled.
    pub fn is_call_graph_enabled(&self) -> bool {
        self.predictor.is_call_graph_enabled()
    }

    /// Clear all cached analyses.
    pub fn clear_cache(&mut self) {
        self.predictor.clear();
    }

    /// Get predictor statistics.
    pub fn predictor_stats(&self) -> crate::mm2::PredictorStats {
        self.predictor.stats()
    }

    /// Prefetch data for a transaction with MM2 prediction.
    ///
    /// This combines ground-truth prefetching with MM2 bytecode analysis:
    /// 1. Run ground-truth prefetch to get all objects from transaction effects
    /// 2. Analyze MoveCall commands using MM2 to predict dynamic field accesses
    /// 3. Match predictions against fetched objects for validation
    /// 4. Optionally fetch additional predicted objects not in ground truth
    pub fn prefetch_for_transaction(
        &mut self,
        grpc: &GrpcClient,
        graphql: Option<&GraphQLClient>,
        rt: &tokio::runtime::Runtime,
        tx: &GrpcTransaction,
        config: &PredictivePrefetchConfig,
    ) -> PredictivePrefetchResult {
        // =========================================================================
        // Phase 1: Ground-truth prefetch (base strategy)
        // =========================================================================
        let base_result =
            ground_truth_prefetch_for_transaction(grpc, graphql, rt, tx, &config.base_config);

        let mut result = PredictivePrefetchResult {
            base_result,
            ..Default::default()
        };

        // =========================================================================
        // Phase 2: MM2 bytecode analysis (if enabled)
        // =========================================================================
        // Collect bytecode-detected upgrade mappings: storage_id -> original_id
        // When a module's bytecode address differs from where we're loading it,
        // this indicates the package has been upgraded.
        let mut bytecode_upgrades: HashMap<String, String> = HashMap::new();

        if config.enable_mm2_prediction {
            let analysis_start = std::time::Instant::now();

            // Enable call graph mode if requested
            if config.use_call_graph && !self.predictor.is_call_graph_enabled() {
                self.predictor.set_call_graph_enabled(true);
            }

            // Load modules from fetched packages into predictor
            for (pkg_id, pkg) in &result.base_result.packages {
                if let Ok(storage_addr) = parse_address(pkg_id) {
                    let modules: Vec<CompiledModule> = pkg
                        .modules
                        .iter()
                        .filter_map(|(_, bytes)| {
                            CompiledModule::deserialize_with_defaults(bytes).ok()
                        })
                        .collect();

                    if !modules.is_empty() {
                        // Detect bytecode address mismatch (upgrade detection)
                        let bytecode_addr = *modules[0].self_id().address();
                        if bytecode_addr != storage_addr {
                            let storage_hex = format!("{:#066x}", storage_addr);
                            let original_hex = format!("{:#066x}", bytecode_addr);
                            bytecode_upgrades.insert(storage_hex, original_hex);
                        }

                        let _ = self.predictor.load_modules(storage_addr, modules);
                        result.prediction_stats.packages_analyzed += 1;
                    }
                }
            }

            // =========================================================================
            // Phase 2b: Fetch missing packages needed for MM2 analysis (fallback)
            // =========================================================================
            let missing_packages = self.find_missing_packages_for_analysis(tx, &result.base_result);

            if !missing_packages.is_empty() {
                let fetched = self.fetch_packages_for_mm2(grpc, rt, &missing_packages);
                for (pkg_id, pkg) in fetched {
                    if let Ok(storage_addr) = parse_address(&pkg_id) {
                        let modules: Vec<CompiledModule> = pkg
                            .modules
                            .iter()
                            .filter_map(|(_, bytes)| {
                                CompiledModule::deserialize_with_defaults(bytes).ok()
                            })
                            .collect();

                        if !modules.is_empty() {
                            // Detect bytecode address mismatch (upgrade detection)
                            let bytecode_addr = *modules[0].self_id().address();
                            if bytecode_addr != storage_addr {
                                let storage_hex = format!("{:#066x}", storage_addr);
                                let original_hex = format!("{:#066x}", bytecode_addr);
                                bytecode_upgrades.insert(storage_hex, original_hex);
                            }

                            let _ = self.predictor.load_modules(storage_addr, modules);
                            result.prediction_stats.packages_fetched_for_mm2 += 1;
                        }
                    }
                    // Also add to base_result so caller has access
                    result.base_result.packages.insert(pkg_id, pkg);
                }
            }

            // Analyze each MoveCall command
            let predictions =
                self.analyze_transaction_commands(tx, config, &mut result.prediction_stats);

            result.prediction_stats.analysis_time_ms = analysis_start.elapsed().as_millis() as u64;
            result.prediction_stats.predictions_made = predictions.len();

            // Categorize predictions by confidence
            for pred in &predictions {
                match pred.confidence {
                    Confidence::High => result.prediction_stats.high_confidence_predictions += 1,
                    Confidence::Medium => {
                        result.prediction_stats.medium_confidence_predictions += 1
                    }
                    Confidence::Low => result.prediction_stats.low_confidence_predictions += 1,
                }
            }

            // =========================================================================
            // Phase 3: Match predictions against ground truth using synthesis
            // =========================================================================
            // Build upgrade resolver to normalize storage_id -> original_id in type strings
            // Include bytecode-detected upgrades (most reliable source)
            let upgrade_resolver = build_upgrade_resolver(&result.base_result, &bytecode_upgrades);
            let ground_truth_types = build_type_index(&result.base_result, &upgrade_resolver);
            let ground_truth_ids = build_id_set(&result.base_result);
            let parent_candidates = collect_parent_candidates(tx, &result.base_result);
            let synthesizer = KeyValueSynthesizer::new();

            for pred in predictions {
                let is_phantom = synthesizer.is_phantom_key(&pred.key_type);
                if is_phantom {
                    result.prediction_stats.phantom_key_predictions += 1;
                }

                let mut info = PredictedAccessInfo {
                    key_type: pred.key_type.clone(),
                    value_type: pred.value_type.clone(),
                    kind: pred.kind,
                    confidence: pred.confidence,
                    source_function: pred.source_function.clone(),
                    matched_ground_truth: false,
                    matched_object_id: None,
                    is_phantom_key: is_phantom,
                    synthesized_child_id: None,
                };

                // Strategy 1: Try to match by type string (original approach)
                if let Some(obj_id) = ground_truth_types.get(&pred.key_type) {
                    info.matched_ground_truth = true;
                    info.matched_object_id = Some(obj_id.clone());
                    result.prediction_stats.predictions_matched_ground_truth += 1;
                }
                // Strategy 2: For phantom keys, try to derive child ID and match
                else if is_phantom {
                    // Try each parent candidate
                    let derived = synthesizer
                        .derive_child_ids_for_parents(&parent_candidates, &pred.key_type);

                    for (_parent, child) in derived {
                        let child_hex = format!("{:#066x}", child);
                        if ground_truth_ids.contains(&child_hex) {
                            info.matched_ground_truth = true;
                            info.matched_object_id = Some(child_hex.clone());
                            info.synthesized_child_id = Some(child_hex);
                            result.prediction_stats.predictions_matched_ground_truth += 1;
                            result.prediction_stats.predictions_matched_via_synthesis += 1;
                            break;
                        }
                    }
                }

                result.predictions.push(info);
            }
        }

        result
    }

    /// Analyze MoveCall commands in a transaction to predict dynamic field accesses.
    fn analyze_transaction_commands(
        &mut self,
        tx: &GrpcTransaction,
        config: &PredictivePrefetchConfig,
        stats: &mut PredictionStats,
    ) -> Vec<PredictedAccess> {
        let mut all_predictions = Vec::new();

        for cmd in &tx.commands {
            if let sui_transport::grpc::GrpcCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                ..
            } = cmd
            {
                stats.commands_analyzed += 1;

                if let Ok(pkg_addr) = parse_address(package) {
                    // Check if module is loaded
                    if !self.predictor.has_module(&pkg_addr, module) {
                        stats.commands_skipped_no_bytecode += 1;
                        continue;
                    }

                    stats.functions_analyzed += 1;

                    // Get predictions for this call
                    let predictions = self.predictor.predict_accesses(
                        &pkg_addr,
                        module,
                        function,
                        type_arguments,
                    );

                    // Filter by minimum confidence
                    for pred in predictions {
                        if pred.confidence >= config.min_confidence {
                            all_predictions.push(pred);
                        }
                    }
                }
            }
        }

        all_predictions
    }

    /// Find packages referenced in MoveCall commands that aren't loaded yet.
    fn find_missing_packages_for_analysis(
        &self,
        tx: &GrpcTransaction,
        base_result: &GroundTruthPrefetchResult,
    ) -> HashSet<String> {
        let mut missing = HashSet::new();

        for cmd in &tx.commands {
            if let sui_transport::grpc::GrpcCommand::MoveCall {
                package,
                module,
                type_arguments,
                ..
            } = cmd
            {
                let pkg_id = normalize_id(package);

                // Check if we already have this package
                if !base_result.packages.contains_key(&pkg_id) {
                    if let Ok(addr) = parse_address(&pkg_id) {
                        if !self.predictor.has_module(&addr, module) {
                            missing.insert(pkg_id.clone());
                        }
                    }
                }

                // Also check type arguments for package references
                for type_arg in type_arguments {
                    for extracted_pkg in extract_packages_from_type(type_arg) {
                        if !base_result.packages.contains_key(&extracted_pkg) {
                            missing.insert(extracted_pkg);
                        }
                    }
                }
            }
        }

        missing
    }

    /// Fetch packages specifically for MM2 analysis.
    fn fetch_packages_for_mm2(
        &self,
        grpc: &GrpcClient,
        rt: &tokio::runtime::Runtime,
        package_ids: &HashSet<String>,
    ) -> Vec<(String, FetchedPackage)> {
        let mut fetched = Vec::new();

        for pkg_id in package_ids {
            let fetch_result = rt.block_on(async { grpc.get_object(pkg_id).await });

            if let Ok(Some(obj)) = fetch_result {
                if let Some(modules) = obj.package_modules {
                    let linkage = obj
                        .package_linkage
                        .as_ref()
                        .map(|l| {
                            l.iter()
                                .map(|link| {
                                    (
                                        normalize_id(&link.original_id),
                                        normalize_id(&link.upgraded_id),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    let pkg = FetchedPackage {
                        package_id: pkg_id.clone(),
                        version: obj.version,
                        modules,
                        linkage,
                        original_id: obj.package_original_id,
                    };
                    fetched.push((pkg_id.clone(), pkg));
                }
            }
        }

        fetched
    }
}

/// Build a PackageUpgradeResolver from ground truth prefetch results and bytecode analysis.
///
/// This collects upgrade mappings from:
/// 1. Bytecode address analysis (most reliable - compares module self-address vs storage location)
/// 2. FetchedPackage.original_id -> package_id (storage_id)
/// 3. FetchedPackage.linkage tables
/// 4. discovered_linkage_upgrades
///
/// The resolver enables normalizing storage_id addresses (from GraphQL) to
/// original_id addresses (used in bytecode), which is essential for matching
/// MM2 predictions against ground truth objects.
fn build_upgrade_resolver(
    result: &GroundTruthPrefetchResult,
    bytecode_upgrades: &HashMap<String, String>,
) -> PackageUpgradeResolver {
    let mut resolver = PackageUpgradeResolver::new();

    // Register bytecode-detected upgrades first (most reliable source)
    // These are detected by comparing module.self_id().address() with the storage address
    for (storage_id, original_id) in bytecode_upgrades {
        resolver.register_package(storage_id, original_id);
    }

    // Register packages with their original_id -> storage_id mapping (if provided by gRPC)
    for (storage_id, pkg) in &result.packages {
        if let Some(original_id) = &pkg.original_id {
            // This package was upgraded: original_id is stable, storage_id is current location
            resolver.register_package(storage_id, original_id);
        }

        // Also register linkage entries from this package's dependency table
        for (original, upgraded) in &pkg.linkage {
            resolver.register_linkage(original, upgraded);
        }
    }

    // Register linkage upgrades discovered during fetch
    for (original, upgraded) in &result.discovered_linkage_upgrades {
        resolver.register_linkage(original, upgraded);
    }

    resolver
}

/// Build an index of type strings to object IDs from prefetch results.
///
/// This allows matching predictions (which are type-based) to actual objects.
/// Type strings are normalized to use original_id addresses so that predictions
/// (which use original_id from bytecode) can match ground truth objects
/// (which may use storage_id from GraphQL).
fn build_type_index(
    result: &GroundTruthPrefetchResult,
    resolver: &PackageUpgradeResolver,
) -> HashMap<String, String> {
    let mut index = HashMap::new();

    for (obj_id, obj) in &result.objects {
        if let Some(type_str) = &obj.type_string {
            // Normalize type string: storage_id -> original_id
            let normalized_type = resolver.normalize_type_string(type_str);

            // For dynamic fields, the type is usually Field<K, V>
            // Extract the key type from the Field wrapper
            if let Some(key_type) = extract_key_type_from_field(&normalized_type) {
                index.insert(key_type, obj_id.clone());
            }
            // Also index by full normalized type
            index.insert(normalized_type, obj_id.clone());
        }
    }

    for (obj_id, obj) in &result.supplemental_objects {
        if let Some(type_str) = &obj.type_string {
            let normalized_type = resolver.normalize_type_string(type_str);
            if let Some(key_type) = extract_key_type_from_field(&normalized_type) {
                index.insert(key_type, obj_id.clone());
            }
            index.insert(normalized_type, obj_id.clone());
        }
    }

    index
}

/// Build a set of all object IDs in ground truth (for fast lookup).
fn build_id_set(result: &GroundTruthPrefetchResult) -> HashSet<String> {
    let mut ids = HashSet::new();

    for obj_id in result.objects.keys() {
        ids.insert(normalize_id(obj_id));
    }

    for obj_id in result.supplemental_objects.keys() {
        ids.insert(normalize_id(obj_id));
    }

    ids
}

/// Collect potential parent object IDs from transaction inputs and prefetch results.
///
/// These are candidates for deriving dynamic field child IDs.
/// This includes both top-level object IDs and nested UIDs extracted from object BCS.
fn collect_parent_candidates(
    tx: &GrpcTransaction,
    result: &GroundTruthPrefetchResult,
) -> Vec<AccountAddress> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    // Add input objects as parent candidates
    for input in &tx.inputs {
        if let sui_transport::grpc::GrpcInput::Object { object_id, .. }
        | sui_transport::grpc::GrpcInput::SharedObject { object_id, .. } = input
        {
            if let Ok(addr) = parse_address(object_id) {
                if seen.insert(addr) {
                    candidates.push(addr);
                }
            }
        }
    }

    // Add fetched objects (top-level IDs)
    for obj_id in result.objects.keys() {
        if let Ok(addr) = parse_address(obj_id) {
            if seen.insert(addr) {
                candidates.push(addr);
            }
        }
    }

    // Extract nested UIDs from object BCS data
    // This is crucial for finding parent UIDs that are fields inside objects
    // (e.g., the `balances` Bag UID inside a BalanceManager)
    for obj in result.objects.values() {
        for nested_uid in extract_nested_uids_from_bcs(&obj.bcs_bytes) {
            if seen.insert(nested_uid) {
                candidates.push(nested_uid);
            }
        }
    }

    // Also check supplemental objects
    for obj in result.supplemental_objects.values() {
        for nested_uid in extract_nested_uids_from_bcs(&obj.bcs_bytes) {
            if seen.insert(nested_uid) {
                candidates.push(nested_uid);
            }
        }
    }

    candidates
}

/// Extract potential nested UIDs from object BCS data.
///
/// In Sui, a UID is a struct `{ id: ID }` where `ID = { bytes: address }`.
/// In BCS, this is simply 32 bytes representing the address.
///
/// Strategy:
/// 1. Every object starts with its own UID (first 32 bytes)
/// 2. Nested objects (like Bag, Table, etc.) contain UIDs at known offsets
/// 3. We scan for all 32-byte sequences that look like valid UIDs
///
/// Heuristics for valid UIDs:
/// - Not all zeros (0x0 is reserved)
/// - Not the same as common framework addresses (0x1, 0x2, 0x3)
/// - Must have non-trivial entropy (not repetitive patterns)
fn extract_nested_uids_from_bcs(bcs: &[u8]) -> Vec<AccountAddress> {
    let mut uids = Vec::new();

    // Need at least 32 bytes for a UID
    if bcs.len() < 32 {
        return uids;
    }

    // The first 32 bytes are the object's own UID - skip it since we already
    // have top-level object IDs as candidates
    let start_offset = 32;

    // Scan at 32-byte boundaries first (most common case)
    // UIDs in structs are typically aligned
    for offset in (start_offset..bcs.len().saturating_sub(31)).step_by(32) {
        if let Some(addr) = try_parse_uid_at_offset(bcs, offset) {
            uids.push(addr);
        }
    }

    // Also scan at non-aligned offsets for nested structs
    // This catches UIDs that follow variable-length fields (like vectors)
    // We check at every 8-byte boundary for efficiency
    for offset in (start_offset..bcs.len().saturating_sub(31)).step_by(8) {
        // Skip if this is a 32-byte aligned position (already checked)
        if offset % 32 == 0 {
            continue;
        }
        if let Some(addr) = try_parse_uid_at_offset(bcs, offset) {
            // Verify this isn't a duplicate
            if !uids.contains(&addr) {
                uids.push(addr);
            }
        }
    }

    uids
}

/// Try to parse a UID at a specific offset in BCS data.
///
/// Returns Some(addr) if the 32 bytes at this offset look like a valid UID.
fn try_parse_uid_at_offset(bcs: &[u8], offset: usize) -> Option<AccountAddress> {
    if offset + 32 > bcs.len() {
        return None;
    }

    let bytes: [u8; 32] = bcs[offset..offset + 32].try_into().ok()?;
    let addr = AccountAddress::new(bytes);

    // Filter out invalid/unlikely UIDs
    if !is_likely_valid_uid(&addr) {
        return None;
    }

    Some(addr)
}

/// Check if an address looks like a valid UID.
///
/// Filters out:
/// - Zero address (0x0)
/// - Framework addresses (0x1, 0x2, 0x3)
/// - Addresses with low entropy (repetitive patterns, likely data not UIDs)
fn is_likely_valid_uid(addr: &AccountAddress) -> bool {
    let bytes = addr.as_ref();

    // Check for all zeros
    if bytes.iter().all(|&b| b == 0) {
        return false;
    }

    // Check for framework addresses (0x1, 0x2, 0x3)
    // These have 31 leading zeros followed by 1, 2, or 3
    let leading_zeros = bytes.iter().take(31).filter(|&&b| b == 0).count();
    if leading_zeros == 31 && bytes[31] <= 3 {
        return false;
    }

    // Check for low entropy (repetitive patterns)
    // A valid UID should have reasonable distribution of byte values
    let unique_bytes: HashSet<u8> = bytes.iter().copied().collect();
    if unique_bytes.len() < 8 {
        // Fewer than 8 unique bytes is suspicious for a hash-derived ID
        // However, we allow it if the address has clear randomness
        // Check if it's a pattern like 0x010101... or 0xffffff...
        let first = bytes[0];
        let all_same = bytes.iter().all(|&b| b == first);
        if all_same {
            return false;
        }
    }

    // Check for common non-UID patterns
    // u64 values often appear in BCS and might accidentally form 32 bytes
    // They typically have many leading/trailing zeros
    let leading_zeros_count = bytes.iter().take_while(|&&b| b == 0).count();
    let trailing_zeros_count = bytes.iter().rev().take_while(|&&b| b == 0).count();
    if leading_zeros_count > 24 || trailing_zeros_count > 24 {
        // More than 24 leading/trailing zeros is unlikely for a real UID
        return false;
    }

    true
}

/// Extract the key type from a Field<K, V> type string.
///
/// Example: "0x2::dynamic_field::Field<0xabc::mod::Key<T>, 0xdef::mod::Value>"
/// Returns: "0xabc::mod::Key<T>"
fn extract_key_type_from_field(type_str: &str) -> Option<String> {
    // Check if this is a Field type
    if !type_str.contains("::dynamic_field::Field<")
        && !type_str.contains("::dynamic_object_field::Field<")
    {
        return None;
    }

    // Find the opening angle bracket after "Field"
    let field_idx = type_str.find("::Field<")?;
    let start = field_idx + "::Field<".len();

    // Find the matching comma that separates K from V
    let chars: Vec<char> = type_str[start..].chars().collect();
    let mut depth = 0;
    let mut comma_idx = None;

    for (i, c) in chars.iter().enumerate() {
        match c {
            '<' => depth += 1,
            '>' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                comma_idx = Some(i);
                break;
            }
            _ => {}
        }
    }

    comma_idx.map(|idx| chars[..idx].iter().collect::<String>().trim().to_string())
}

/// Parse a hex address string to AccountAddress.
/// Delegates to the canonical implementation in sui-resolver.
fn parse_address(addr: &str) -> Result<AccountAddress, String> {
    sui_resolver::parse_address(addr).ok_or_else(|| format!("Invalid address: {}", addr))
}

/// Normalize an object ID to lowercase with 0x prefix and full 64 chars.
/// Delegates to the canonical implementation in sui-resolver.
fn normalize_id(id: &str) -> String {
    sui_resolver::normalize_id(id)
}

/// Extract package IDs from a type string.
fn extract_packages_from_type(type_str: &str) -> Vec<String> {
    let mut packages = Vec::new();

    // Look for 0x followed by hex chars
    let mut i = 0;
    let chars: Vec<char> = type_str.chars().collect();

    while i < chars.len().saturating_sub(2) {
        if chars[i] == '0' && chars[i + 1] == 'x' {
            let mut end = i + 2;
            while end < chars.len() && chars[end].is_ascii_hexdigit() {
                end += 1;
            }
            if end > i + 2 {
                let addr: String = chars[i..end].iter().collect();
                let normalized = normalize_id(&addr);
                // Skip framework addresses (0x1, 0x2, 0x3)
                if !is_framework_address(&normalized) && !packages.contains(&normalized) {
                    packages.push(normalized);
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }

    packages
}

/// Check if an address is a framework address (0x1, 0x2, 0x3).
fn is_framework_address(addr: &str) -> bool {
    sui_resolver::is_framework_address(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_key_type_from_field() {
        let field_type = "0x2::dynamic_field::Field<0xabc::mod::Key<u64>, 0xdef::mod::Value>";
        let key = extract_key_type_from_field(field_type);
        assert_eq!(key, Some("0xabc::mod::Key<u64>".to_string()));
    }

    #[test]
    fn test_extract_key_type_nested() {
        let field_type =
            "0x2::dynamic_field::Field<0xabc::mod::Key<0xdef::token::TOKEN>, Balance<u64>>";
        let key = extract_key_type_from_field(field_type);
        assert_eq!(
            key,
            Some("0xabc::mod::Key<0xdef::token::TOKEN>".to_string())
        );
    }

    #[test]
    fn test_extract_key_type_not_field() {
        let not_field = "0x2::coin::Coin<0x2::sui::SUI>";
        let key = extract_key_type_from_field(not_field);
        assert_eq!(key, None);
    }

    #[test]
    fn test_config_defaults() {
        let config = PredictivePrefetchConfig::default();
        assert!(config.enable_mm2_prediction);
        assert_eq!(config.max_transitive_depth, 3);
        assert_eq!(config.min_confidence, Confidence::Medium);
        assert!(!config.fetch_predictions_not_in_ground_truth);
    }

    #[test]
    fn test_prefetcher_creation() {
        let prefetcher = PredictivePrefetcher::new();
        let stats = prefetcher.predictor_stats();
        assert_eq!(stats.modules_loaded, 0);
        assert_eq!(stats.analyses_cached, 0);
    }
}
