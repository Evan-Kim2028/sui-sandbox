//! Eager data prefetching for PTB replay.
//!
//! This module provides the ground-truth-first prefetching strategy:
//!
//! - **Ground-Truth-First** (recommended): Uses `unchanged_loaded_runtime_objects` as the
//!   authoritative source of what objects need to be fetched. This is faster and more accurate
//!   because we already know exact versions from the transaction effects.
//!
//! ## Ground-Truth Strategy
//!
//! The transaction's `unchanged_loaded_runtime_objects` field tells us exactly which objects
//! (including dynamically-accessed children) were loaded during execution. This is our "ground truth"
//! for what needs to be prefetched.
//!
//! Key advantages:
//! - **Exact versions**: No need to guess or discover versions
//! - **Complete coverage**: Includes all dynamic field children that were accessed
//! - **Faster**: Direct gRPC fetch, skip GraphQL discovery round-trip
//! - **More reliable**: No version mismatch issues from GraphQL returning current state

use std::collections::{HashMap, HashSet};

use sui_resolver::address::normalize_address;
use sui_sandbox_types::{FetchedObject, FetchedPackage};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcClient, GrpcObject, GrpcTransaction};

// =============================================================================
// Ground-Truth-First Prefetch (Recommended)
// =============================================================================

/// Configuration for ground-truth-first prefetching.
#[derive(Debug, Clone)]
pub struct GroundTruthPrefetchConfig {
    /// Maximum concurrent gRPC requests for parallel fetching.
    /// Recommended: 10-20 for most endpoints.
    pub fetch_concurrency: usize,

    /// Whether to use GraphQL as supplemental discovery for objects
    /// not found in ground truth. Default: false.
    pub enable_supplemental_graphql: bool,

    /// Maximum depth for supplemental GraphQL discovery (if enabled).
    pub supplemental_max_depth: usize,

    /// Maximum fields per object for supplemental discovery (if enabled).
    pub supplemental_max_fields: usize,

    /// Pre-known linkage upgrades from previous transactions.
    /// Maps original_package_id -> upgraded_package_id.
    pub known_linkage_upgrades: HashMap<String, String>,
}

impl Default for GroundTruthPrefetchConfig {
    fn default() -> Self {
        Self {
            fetch_concurrency: 5, // Conservative rate limit (~10 req/s max)
            enable_supplemental_graphql: false,
            supplemental_max_depth: 4,
            supplemental_max_fields: 100,
            known_linkage_upgrades: HashMap::new(),
        }
    }
}

/// Result of ground-truth-first prefetching.
#[derive(Debug, Default)]
pub struct GroundTruthPrefetchResult {
    /// Objects fetched directly from ground truth (by object_id).
    pub objects: HashMap<String, FetchedObject>,

    /// Packages with their modules (by package_id).
    pub packages: HashMap<String, FetchedPackage>,

    /// Objects discovered via supplemental GraphQL (if enabled).
    pub supplemental_objects: HashMap<String, FetchedObject>,

    /// Linkage upgrades discovered during fetch.
    /// Maps original_package_id -> upgraded_package_id.
    pub discovered_linkage_upgrades: HashMap<String, String>,

    /// Detailed statistics.
    pub stats: GroundTruthPrefetchStats,
}

// FetchedObject and FetchedPackage are imported from sui-sandbox-types

/// Statistics for ground-truth prefetching.
#[derive(Debug, Default)]
pub struct GroundTruthPrefetchStats {
    /// Number of objects in ground truth (unchanged_loaded_runtime_objects + changed_objects + etc.)
    pub ground_truth_count: usize,
    /// Number of objects successfully fetched from ground truth.
    pub ground_truth_fetched: usize,
    /// Failed fetches with (object_id, error_message).
    pub ground_truth_failures: Vec<(String, String)>,

    /// Number of packages found (either from ground truth or explicit commands).
    pub packages_found: usize,
    /// Number of packages successfully fetched.
    pub packages_fetched: usize,
    /// Transitive package dependencies discovered.
    pub packages_transitive: usize,

    /// Supplemental GraphQL discovery stats (if enabled).
    pub supplemental_discovered: usize,
    pub supplemental_fetched: usize,

    /// Timing information.
    pub fetch_time_ms: u64,
}

/// Ground-truth-first prefetch for a transaction.
///
/// This function fetches all objects needed to replay a transaction by:
/// 1. Collecting all object IDs and versions from transaction effects (ground truth)
/// 2. Fetching all objects in parallel via gRPC at their exact historical versions
/// 3. Extracting package dependencies and following linkage upgrades
/// 4. Optionally supplementing with GraphQL discovery for edge cases
///
/// # Arguments
/// * `grpc` - gRPC client for fetching objects
/// * `graphql` - Optional GraphQL client for supplemental discovery
/// * `rt` - Tokio runtime for async operations
/// * `tx` - The transaction to prefetch data for
/// * `config` - Prefetch configuration
pub fn ground_truth_prefetch_for_transaction(
    grpc: &GrpcClient,
    graphql: Option<&GraphQLClient>,
    rt: &tokio::runtime::Runtime,
    tx: &GrpcTransaction,
    config: &GroundTruthPrefetchConfig,
) -> GroundTruthPrefetchResult {
    let start = std::time::Instant::now();
    let mut result = GroundTruthPrefetchResult::default();

    // =========================================================================
    // Phase 1: Collect ground truth - all known object versions
    // =========================================================================
    let mut ground_truth: HashMap<String, u64> = HashMap::new();

    // From unchanged_loaded_runtime_objects - THE key source
    // These are dynamic field children and other objects accessed during execution
    for (id, ver) in &tx.unchanged_loaded_runtime_objects {
        ground_truth.insert(normalize_address(id), *ver);
    }

    // From changed_objects (INPUT version, before modification)
    for (id, ver) in &tx.changed_objects {
        ground_truth.insert(normalize_address(id), *ver);
    }

    // From unchanged_consensus_objects (shared objects read but not modified)
    for (id, ver) in &tx.unchanged_consensus_objects {
        ground_truth.insert(normalize_address(id), *ver);
    }

    // From transaction inputs
    for input in &tx.inputs {
        match input {
            sui_transport::grpc::GrpcInput::Object {
                object_id, version, ..
            } => {
                ground_truth
                    .entry(normalize_address(object_id))
                    .or_insert(*version);
            }
            sui_transport::grpc::GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                ground_truth
                    .entry(normalize_address(object_id))
                    .or_insert(*initial_version);
            }
            sui_transport::grpc::GrpcInput::Receiving {
                object_id, version, ..
            } => {
                ground_truth
                    .entry(normalize_address(object_id))
                    .or_insert(*version);
            }
            sui_transport::grpc::GrpcInput::Pure { .. } => {}
        }
    }

    result.stats.ground_truth_count = ground_truth.len();

    // =========================================================================
    // Phase 2: Parallel fetch all ground truth objects
    // =========================================================================
    let object_versions: Vec<(String, u64)> = ground_truth.into_iter().collect();

    let fetch_results = rt.block_on(async {
        grpc.batch_fetch_objects_at_versions(&object_versions, config.fetch_concurrency)
            .await
    });

    // Process fetch results
    let mut all_linkage: HashMap<String, String> = config.known_linkage_upgrades.clone();
    // Track package_id -> required_version from linkage tables
    let mut linkage_versions: HashMap<String, u64> = HashMap::new();

    for (obj_id, fetch_result) in fetch_results {
        match fetch_result {
            Ok(Some(obj)) => {
                result.stats.ground_truth_fetched += 1;

                // Check if this is a package
                if let Some(modules) = &obj.package_modules {
                    let pkg = FetchedPackage {
                        package_id: obj_id.clone(),
                        version: obj.version,
                        modules: modules.clone(),
                        linkage: extract_linkage_map(&obj),
                        original_id: obj.package_original_id.clone(),
                    };

                    // Accumulate linkage for dependency resolution (with versions)
                    let linkage_with_vers = extract_linkage_with_versions(&obj);
                    for (orig, (upgraded, version)) in &linkage_with_vers {
                        all_linkage.insert(orig.clone(), upgraded.clone());
                        result
                            .discovered_linkage_upgrades
                            .insert(orig.clone(), upgraded.clone());
                        // Track the required version for upgraded packages
                        linkage_versions.insert(upgraded.clone(), *version);
                    }

                    result.packages.insert(obj_id.clone(), pkg);
                    result.stats.packages_found += 1;
                    result.stats.packages_fetched += 1;
                } else if let Some(bcs) = obj.bcs {
                    // Regular object
                    let mut fetched = FetchedObject::new(obj_id.clone(), obj.version, bcs);
                    if let Some(ts) = obj.type_string {
                        fetched = fetched.with_type(ts);
                    }
                    result.objects.insert(obj_id.clone(), fetched);
                }
            }
            Ok(None) => {
                result
                    .stats
                    .ground_truth_failures
                    .push((obj_id, "Object not found".to_string()));
            }
            Err(e) => {
                result
                    .stats
                    .ground_truth_failures
                    .push((obj_id, e.to_string()));
            }
        }
    }

    // =========================================================================
    // Phase 3: Extract and fetch package dependencies from multiple sources
    // =========================================================================
    let mut packages_to_fetch: HashSet<String> = HashSet::new();

    // Source 1: MoveCall commands (direct package calls)
    for cmd in &tx.commands {
        if let sui_transport::grpc::GrpcCommand::MoveCall {
            package,
            type_arguments,
            ..
        } = cmd
        {
            let pkg_id = normalize_address(package);
            if !result.packages.contains_key(&pkg_id) {
                packages_to_fetch.insert(pkg_id);
            }

            // Extract packages from type arguments
            for type_arg in type_arguments {
                for pkg_id in extract_packages_from_type(type_arg) {
                    if !result.packages.contains_key(&pkg_id) {
                        packages_to_fetch.insert(pkg_id);
                    }
                }
            }
        }

        // Also handle MakeMoveVec element_type
        if let sui_transport::grpc::GrpcCommand::MakeMoveVec {
            element_type: Some(elem_type),
            ..
        } = cmd
        {
            for pkg_id in extract_packages_from_type(elem_type) {
                if !result.packages.contains_key(&pkg_id) {
                    packages_to_fetch.insert(pkg_id);
                }
            }
        }
    }

    // Source 2: Object types from fetched objects
    // Objects may have types like `0xabc::module::Struct<0xdef::token::TOKEN>`
    // We need to fetch both 0xabc and 0xdef packages
    for obj in result.objects.values() {
        if let Some(ref type_str) = obj.type_string {
            for pkg_id in extract_packages_from_type(type_str) {
                if !result.packages.contains_key(&pkg_id) {
                    packages_to_fetch.insert(pkg_id);
                }
            }
        }
    }

    // Source 3: Linkage upgrades - also fetch upgraded packages
    // This ensures we have both original and upgraded versions
    for (original, upgraded) in &all_linkage {
        if !result.packages.contains_key(upgraded) {
            packages_to_fetch.insert(upgraded.clone());
        }
        if !result.packages.contains_key(original) {
            packages_to_fetch.insert(original.clone());
        }
    }

    // Fetch missing packages (with limited transitive dependency resolution)
    // NOTE: The bytecode dependency extraction is broken (picks up garbage addresses),
    // so we limit to depth=1 which only fetches directly-called packages.
    let mut fetched_packages: HashSet<String> = result.packages.keys().cloned().collect();
    let mut depth = 0;
    // Now using proper move-binary-format parsing, we can safely enable transitive resolution
    // Limit to 5 levels which should cover most realistic dependency chains
    const MAX_PACKAGE_DEPTH: usize = 5;

    while !packages_to_fetch.is_empty() && depth < MAX_PACKAGE_DEPTH {
        let to_fetch: Vec<String> = packages_to_fetch
            .iter()
            .filter(|id| !fetched_packages.contains(*id))
            .cloned()
            .collect();

        if to_fetch.is_empty() {
            break;
        }

        // Check if any package has an upgraded address via linkage
        // Use linkage_versions to fetch at the correct historical version
        let mut fetch_pairs: Vec<(String, Option<u64>)> = Vec::new();
        for pkg_id in &to_fetch {
            // Try upgraded address first if known
            if let Some(upgraded) = all_linkage.get(pkg_id) {
                if !fetched_packages.contains(upgraded) {
                    // Use version from linkage if available
                    let version = linkage_versions.get(upgraded).copied();
                    fetch_pairs.push((upgraded.clone(), version));
                }
            }
            // Also try the original package ID with its known version
            let version = linkage_versions.get(pkg_id).copied();
            fetch_pairs.push((pkg_id.clone(), version));
        }

        // Fetch packages (sequentially for now, packages are usually few)
        // Use get_object_at_version to respect historical versions from linkage
        let pkg_results = rt.block_on(async {
            let mut results = Vec::new();
            for (id, version) in &fetch_pairs {
                let res = grpc.get_object_at_version(id, *version).await;
                results.push((id.clone(), res));
            }
            results
        });

        let mut new_deps: HashSet<String> = HashSet::new();

        for (pkg_id, fetch_result) in pkg_results {
            match fetch_result {
                Ok(Some(obj)) if obj.package_modules.is_some() => {
                    let linkage = extract_linkage_map(&obj);
                    let linkage_with_vers = extract_linkage_with_versions(&obj);
                    let modules = obj.package_modules.unwrap();

                    // Accumulate linkage and queue upgraded packages (with versions)
                    for (orig, (upgraded, version)) in &linkage_with_vers {
                        all_linkage.insert(orig.clone(), upgraded.clone());
                        result
                            .discovered_linkage_upgrades
                            .insert(orig.clone(), upgraded.clone());
                        // Track the required version for upgraded packages
                        linkage_versions.insert(upgraded.clone(), *version);

                        // Also fetch the upgraded package if not already fetched
                        if !fetched_packages.contains(upgraded)
                            && !result.packages.contains_key(upgraded)
                        {
                            new_deps.insert(upgraded.clone());
                        }
                    }

                    // Extract dependencies from bytecode using comprehensive extraction
                    for (_name, bytecode) in &modules {
                        // Use the more comprehensive extraction that includes struct/function handles
                        let deps = extract_all_package_references(bytecode);
                        for dep in deps {
                            let dep_norm = normalize_address(&dep);
                            // Follow linkage to upgraded address
                            let actual_dep =
                                all_linkage.get(&dep_norm).cloned().unwrap_or(dep_norm);
                            if !fetched_packages.contains(&actual_dep)
                                && !result.packages.contains_key(&actual_dep)
                            {
                                new_deps.insert(actual_dep);
                            }
                        }
                    }

                    // Check if this is an original package (version 1) that might have upgrades
                    // The linkage table of this package won't point to its own upgrades,
                    // but OTHER packages that depend on it will have the upgrade info
                    // We'll handle this in a post-processing step

                    let pkg = FetchedPackage {
                        package_id: pkg_id.clone(),
                        version: obj.version,
                        modules,
                        linkage,
                        original_id: obj.package_original_id,
                    };

                    result.packages.insert(pkg_id.clone(), pkg);
                    fetched_packages.insert(pkg_id);
                    result.stats.packages_fetched += 1;
                    if depth > 0 {
                        result.stats.packages_transitive += 1;
                    }
                }
                _ => {
                    fetched_packages.insert(pkg_id); // Mark as attempted
                }
            }
        }

        packages_to_fetch = new_deps;
        depth += 1;
    }

    // =========================================================================
    // Phase 3b: Upgrade Resolution for Directly-Called Packages
    // =========================================================================
    // For packages that are called directly (from MoveCall), check if we have
    // the upgraded version. The linkage tables from OTHER packages tell us
    // what storage_id contains the upgraded code.
    //
    // Strategy: Look through all linkage entries we've collected. If a linkage
    // entry points FROM a directly-called package TO an upgraded storage_id,
    // and we don't have that upgraded package, fetch it.
    let mut upgrade_fetch_queue: HashSet<(String, u64)> = HashSet::new(); // (pkg_id, version)

    // Collect all directly-called packages (from MoveCall commands)
    let mut directly_called: HashSet<String> = HashSet::new();
    for cmd in &tx.commands {
        if let sui_transport::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
            directly_called.insert(normalize_address(package));
        }
    }

    // Check each directly-called package
    for pkg_runtime_id in &directly_called {
        // Look for linkage entries where original_id matches this package
        // These entries come from packages that DEPEND on the directly-called package
        for (_orig, (upgraded, version)) in &linkage_versions
            .iter()
            .filter_map(|(upgraded, ver)| {
                // Check if any linkage maps to this upgraded_id
                all_linkage
                    .iter()
                    .find(|(orig, upg)| {
                        *upg == upgraded && normalize_address(orig) == *pkg_runtime_id
                    })
                    .map(|(orig, _)| (orig.clone(), (upgraded.clone(), *ver)))
            })
            .collect::<Vec<_>>()
        {
            // We found that pkg_runtime_id has an upgrade at 'upgraded' with 'version'
            if !result.packages.contains_key(upgraded) && !fetched_packages.contains(upgraded) {
                upgrade_fetch_queue.insert((upgraded.clone(), *version));
            }
        }
    }

    // Also check: for any package we fetched at version 1 that is directly called,
    // see if another package's linkage tells us about its upgrade
    for pkg in result.packages.values() {
        if pkg.version == 1 && directly_called.contains(&pkg.package_id) {
            // This is a version 1 package that's directly called
            // Check if any linkage entry points to an upgrade of it
            for (orig, upgraded) in &all_linkage {
                if normalize_address(orig) == normalize_address(&pkg.package_id) {
                    // Found an upgrade! Get the version from linkage_versions
                    if let Some(&version) = linkage_versions.get(upgraded) {
                        if !result.packages.contains_key(upgraded) {
                            upgrade_fetch_queue.insert((upgraded.clone(), version));
                        }
                    }
                }
            }
        }
    }

    // Fetch any upgraded packages we found
    if !upgrade_fetch_queue.is_empty() {
        let upgrade_results = rt.block_on(async {
            let mut results = Vec::new();
            for (pkg_id, version) in &upgrade_fetch_queue {
                let res = grpc.get_object_at_version(pkg_id, Some(*version)).await;
                results.push((pkg_id.clone(), version, res));
            }
            results
        });

        for (pkg_id, _version, fetch_result) in upgrade_results {
            if let Ok(Some(obj)) = fetch_result {
                if let Some(ref modules) = obj.package_modules {
                    let linkage = extract_linkage_map(&obj);

                    let pkg = FetchedPackage {
                        package_id: pkg_id.clone(),
                        version: obj.version,
                        modules: modules.clone(),
                        linkage,
                        original_id: obj.package_original_id,
                    };

                    result.packages.insert(pkg_id.clone(), pkg);
                    result.stats.packages_fetched += 1;
                }
            }
        }
    }

    // =========================================================================
    // Phase 3c: GraphQL-based Upgrade Resolution for Directly-Called Packages
    // =========================================================================
    // For packages that are:
    // 1. Directly called (in MoveCall commands)
    // 2. At version 1 (original)
    // We need to discover if there's an upgraded version using GraphQL's
    // packageVersionsAfter query, since linkage tables only exist in
    // DEPENDENT packages, not the upgraded package itself.
    if let Some(gql) = graphql {
        let mut graphql_upgrade_queue: Vec<(String, String)> = Vec::new(); // (original_id, latest_addr)

        for pkg_runtime_id in &directly_called {
            // Check if we have this package at version 1
            if let Some(pkg) = result.packages.get(pkg_runtime_id) {
                if pkg.version == 1 {
                    // This is a version 1 package that's directly called
                    // Use GraphQL to discover upgrades
                    if let Ok(Some((latest_addr, _version))) =
                        gql.get_latest_package_upgrade(pkg_runtime_id)
                    {
                        let latest_norm = normalize_address(&latest_addr);
                        if !result.packages.contains_key(&latest_norm) {
                            graphql_upgrade_queue.push((pkg_runtime_id.clone(), latest_norm));
                        }
                    }
                }
            }
        }

        // Fetch the upgraded packages discovered via GraphQL
        if !graphql_upgrade_queue.is_empty() {
            let upgrade_results = rt.block_on(async {
                let mut results = Vec::new();
                for (original_id, latest_addr) in &graphql_upgrade_queue {
                    let res = grpc.get_object(latest_addr).await;
                    results.push((original_id.clone(), latest_addr.clone(), res));
                }
                results
            });

            for (original_id, latest_addr, fetch_result) in upgrade_results {
                if let Ok(Some(obj)) = fetch_result {
                    if let Some(ref modules) = obj.package_modules {
                        let linkage = extract_linkage_map(&obj);

                        let pkg = FetchedPackage {
                            package_id: latest_addr.clone(),
                            version: obj.version,
                            modules: modules.clone(),
                            linkage,
                            original_id: obj.package_original_id.clone(),
                        };

                        // Record the upgrade mapping
                        result
                            .discovered_linkage_upgrades
                            .insert(original_id.clone(), latest_addr.clone());

                        result.packages.insert(latest_addr, pkg);
                        result.stats.packages_fetched += 1;
                    }
                }
            }
        }
    }

    // =========================================================================
    // Phase 4: Supplemental GraphQL discovery (optional)
    // =========================================================================
    if config.enable_supplemental_graphql {
        if let Some(gql) = graphql {
            let supplemental = discover_supplemental_objects(
                gql,
                grpc,
                rt,
                &result.objects,
                config.supplemental_max_depth,
                config.supplemental_max_fields,
            );

            result.stats.supplemental_discovered = supplemental.len();

            // Fetch supplemental objects
            let supp_pairs: Vec<(String, u64)> = supplemental.into_iter().collect();
            let supp_results = rt.block_on(async {
                grpc.batch_fetch_objects_at_versions(&supp_pairs, config.fetch_concurrency)
                    .await
            });

            for (obj_id, fetch_result) in supp_results {
                if let Ok(Some(obj)) = fetch_result {
                    if let Some(bcs) = obj.bcs {
                        let mut fetched = FetchedObject::new(obj_id.clone(), obj.version, bcs);
                        if let Some(ts) = obj.type_string {
                            fetched = fetched.with_type(ts);
                        }
                        result.supplemental_objects.insert(obj_id, fetched);
                        result.stats.supplemental_fetched += 1;
                    }
                }
            }
        }
    }

    result.stats.fetch_time_ms = start.elapsed().as_millis() as u64;
    result
}

/// Discover additional objects via GraphQL that weren't in ground truth.
fn discover_supplemental_objects(
    graphql: &GraphQLClient,
    grpc: &GrpcClient,
    rt: &tokio::runtime::Runtime,
    existing_objects: &HashMap<String, FetchedObject>,
    max_depth: usize,
    max_fields: usize,
) -> HashMap<String, u64> {
    let mut discovered: HashMap<String, u64> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut to_visit: Vec<(String, usize)> =
        existing_objects.keys().map(|id| (id.clone(), 0)).collect();

    while let Some((parent_id, depth)) = to_visit.pop() {
        if visited.contains(&parent_id) || depth > max_depth {
            continue;
        }
        visited.insert(parent_id.clone());

        // Discover dynamic fields via GraphQL
        let dfs = match graphql.fetch_dynamic_fields(&parent_id, max_fields) {
            Ok(fields) => fields,
            Err(_) => continue,
        };

        for df in dfs {
            if let Some(child_id) = df.object_id {
                let normalized = normalize_address(&child_id);
                if !existing_objects.contains_key(&normalized)
                    && !discovered.contains_key(&normalized)
                {
                    // Try to get version from gRPC
                    if let Ok(Some(obj)) = rt.block_on(async { grpc.get_object(&normalized).await })
                    {
                        discovered.insert(normalized.clone(), obj.version);
                    } else if let Some(ver) = df.version {
                        discovered.insert(normalized.clone(), ver);
                    }

                    // Queue for recursive exploration
                    if depth < max_depth {
                        to_visit.push((normalized, depth + 1));
                    }
                }
            }
        }
    }

    discovered
}

/// Extract linkage map from a GrpcObject.
fn extract_linkage_map(obj: &GrpcObject) -> HashMap<String, String> {
    obj.package_linkage
        .as_ref()
        .map(|linkage| {
            linkage
                .iter()
                .map(|l| {
                    (
                        normalize_address(&l.original_id),
                        normalize_address(&l.upgraded_id),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Extract linkage map with versions from a GrpcObject.
/// Returns HashMap<original_id, (upgraded_id, upgraded_version)>
fn extract_linkage_with_versions(obj: &GrpcObject) -> HashMap<String, (String, u64)> {
    obj.package_linkage
        .as_ref()
        .map(|linkage| {
            linkage
                .iter()
                .map(|l| {
                    (
                        normalize_address(&l.original_id),
                        (normalize_address(&l.upgraded_id), l.upgraded_version),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

// Note: For basic dependency extraction from bytecode, use sui_sandbox_core::utilities::extract_dependencies_from_bytecode

/// Extract ALL package addresses referenced in bytecode, including:
/// - Module handles (direct dependencies)
/// - Struct handles (type references)
/// - Function handles (called functions from other modules)
///
/// This is more comprehensive than extract_dependencies_from_bytecode and helps
/// catch transitive dependencies that might be missed.
fn extract_all_package_references(bytecode: &[u8]) -> Vec<String> {
    use move_binary_format::CompiledModule;

    let module = match CompiledModule::deserialize_with_defaults(bytecode) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };

    let mut refs = Vec::new();
    let mut seen = HashSet::new();

    // Helper to add an address
    let mut add_addr = |addr: &move_core_types::account_address::AccountAddress| {
        let addr_str = addr.to_hex_literal();
        if !is_framework_address(&addr_str) {
            let normalized = normalize_address(&addr_str);
            if seen.insert(normalized.clone()) {
                refs.push(normalized);
            }
        }
    };

    // 1. Module handles - direct module dependencies
    for handle in &module.module_handles {
        add_addr(&module.address_identifiers[handle.address.0 as usize]);
    }

    // 2. Datatype handles - type references (important for deserialization)
    for handle in &module.datatype_handles {
        let module_handle = &module.module_handles[handle.module.0 as usize];
        add_addr(&module.address_identifiers[module_handle.address.0 as usize]);
    }

    // 3. Function handles - called functions
    for handle in &module.function_handles {
        let module_handle = &module.module_handles[handle.module.0 as usize];
        add_addr(&module.address_identifiers[module_handle.address.0 as usize]);
    }

    refs
}

// is_framework_address is imported from sui_resolver::address
use sui_resolver::address::is_framework_address;

// =============================================================================
// Transaction Analysis (shared utilities)
// =============================================================================

/// Analyze a transaction to determine what objects will be accessed.
pub fn analyze_transaction_access_patterns(tx: &GrpcTransaction) -> TransactionAccessAnalysis {
    let mut analysis = TransactionAccessAnalysis::default();

    for input in &tx.inputs {
        match input {
            sui_transport::grpc::GrpcInput::Object { object_id, .. } => {
                analysis
                    .explicit_objects
                    .insert(normalize_address(object_id));
            }
            sui_transport::grpc::GrpcInput::SharedObject { object_id, .. } => {
                analysis.shared_objects.insert(normalize_address(object_id));
            }
            sui_transport::grpc::GrpcInput::Receiving { object_id, .. } => {
                analysis
                    .receiving_objects
                    .insert(normalize_address(object_id));
            }
            sui_transport::grpc::GrpcInput::Pure { .. } => {}
        }
    }

    for cmd in &tx.commands {
        if let sui_transport::grpc::GrpcCommand::MoveCall {
            package,
            type_arguments,
            ..
        } = cmd
        {
            analysis.packages.insert(normalize_address(package));

            for type_arg in type_arguments {
                for pkg_id in extract_packages_from_type(type_arg) {
                    analysis.packages.insert(pkg_id);
                }
            }
        }
    }

    for (id, _) in &tx.unchanged_loaded_runtime_objects {
        analysis
            .runtime_loaded_objects
            .insert(normalize_address(id));
    }

    analysis
}

#[derive(Debug, Default)]
pub struct TransactionAccessAnalysis {
    pub explicit_objects: HashSet<String>,
    pub shared_objects: HashSet<String>,
    pub receiving_objects: HashSet<String>,
    pub packages: HashSet<String>,
    pub runtime_loaded_objects: HashSet<String>,
}

impl TransactionAccessAnalysis {
    pub fn all_objects(&self) -> HashSet<String> {
        let mut all = HashSet::new();
        all.extend(self.explicit_objects.iter().cloned());
        all.extend(self.shared_objects.iter().cloned());
        all.extend(self.receiving_objects.iter().cloned());
        all.extend(self.runtime_loaded_objects.iter().cloned());
        all
    }

    pub fn hidden_access_count(&self) -> usize {
        let explicit: HashSet<_> = self
            .explicit_objects
            .iter()
            .chain(self.shared_objects.iter())
            .chain(self.receiving_objects.iter())
            .collect();

        self.runtime_loaded_objects
            .iter()
            .filter(|id| !explicit.contains(id))
            .count()
    }
}

// =============================================================================
// Shared Utilities (imported from sui_resolver::address)
// =============================================================================

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
                if !is_framework_address(&addr) {
                    packages.push(normalize_address(&addr));
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }

    packages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_address() {
        assert_eq!(
            normalize_address("0xABC"),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
        assert_eq!(
            normalize_address("ABC"),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
        assert_eq!(
            normalize_address("0x0000000000000000000000000000000000000000000000000000000000000002"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_extract_packages_from_type() {
        let packages =
            extract_packages_from_type("0x2::coin::Coin<0xabc123def456::mytoken::TOKEN>");
        // 0x2 is framework, should be excluded
        assert!(!packages.iter().any(|p| p.ends_with("2")));
        // Should find the user package
        assert!(packages.iter().any(|p| p.contains("abc123def456")));
    }

    #[test]
    fn test_is_framework_address() {
        assert!(is_framework_address("0x1"));
        assert!(is_framework_address("0x2"));
        assert!(is_framework_address("0x3"));
        assert!(is_framework_address(
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        ));
        assert!(!is_framework_address("0x4"));
        assert!(!is_framework_address("0xabc"));
    }

    #[test]
    fn test_ground_truth_config_default() {
        let config = GroundTruthPrefetchConfig::default();
        assert_eq!(config.fetch_concurrency, 5);
        assert!(!config.enable_supplemental_graphql);
    }
}
