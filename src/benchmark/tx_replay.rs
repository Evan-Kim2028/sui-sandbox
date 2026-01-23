//! Transaction Replay Module
//!
//! This module provides types and utilities for replaying Sui transactions
//! in the local Move VM sandbox. This enables:
//!
//! 1. **Validation**: Compare local execution with on-chain effects
//! 2. **Training Data**: Generate input/output pairs for LLM training
//! 3. **Testing**: Use real transaction patterns for testing
//!
//! ## Architecture
//!
//! Transactions are fetched via GraphQL (see `DataFetcher`) and cached locally.
//! The cached transactions can then be replayed using the `FetchedTransaction::replay()` method.
//!
//! ```text
//! GraphQL → CachedTransaction → PTBCommands → LocalExecution → CompareEffects
//! ```
//!
//! ## Usage
//!
//! See `examples/` for complete transaction replay examples.
//! Requires a `.tx-cache` directory with cached transaction data.
//!
//! ## Known Limitations: Dynamic Field Traversal
//!
//! Some DeFi protocols (Cetus, Turbos) use `skip_list` data structures that store
//! tick data as dynamic fields. These present a replay challenge:
//!
//! 1. The skip_list computes tick indices at runtime during traversal
//! 2. Each computed index becomes a dynamic field lookup via `derive_dynamic_field_id()`
//! 3. We can pre-fetch known dynamic fields, but not indices computed during execution
//!
//! **Example**: A Cetus swap traverses: `head(0) → 481316 → 512756 → tail(887272)`.
//! If the swap needs tick `500000`, this is computed at runtime and we can't know
//! to pre-fetch it without simulating the entire traversal.
//!
//! **Workarounds**:
//! - Cache all dynamic field children at transaction time
//! - Use synthetic/mocked transactions for testing (see `synthetic_ptb_case_study.rs`)
//! - Pre-fetch all known tick indices for a pool

// Re-export everything from sui-sandbox-core::tx_replay
pub use sui_sandbox_core::tx_replay::*;

// Local imports for DataFetcher-dependent functions
use anyhow::Result;
use base64::Engine;

// ============================================================================
// DataFetcher-dependent Functions
// ============================================================================
// These functions depend on crate::data_fetcher::DataFetcher which is only
// available in the main crate.

/// Extract package addresses that a module depends on from its bytecode.
///
/// This parses the CompiledModule to find all module_handles, which reference
/// other modules that this module depends on.
fn extract_dependencies_from_bytecode(
    bytecode: &[u8],
) -> Vec<move_core_types::account_address::AccountAddress> {
    use move_binary_format::CompiledModule;
    use move_core_types::account_address::AccountAddress;
    use std::collections::BTreeSet;

    // Framework addresses to skip
    let framework_addrs: BTreeSet<AccountAddress> = [
        AccountAddress::from_hex_literal("0x1").unwrap(),
        AccountAddress::from_hex_literal("0x2").unwrap(),
        AccountAddress::from_hex_literal("0x3").unwrap(),
    ]
    .into_iter()
    .collect();

    let mut deps = Vec::new();

    if let Ok(module) = CompiledModule::deserialize_with_defaults(bytecode) {
        for handle in &module.module_handles {
            let addr = *module.address_identifier_at(handle.address);
            // Skip framework modules
            if !framework_addrs.contains(&addr) {
                deps.push(addr);
            }
        }
    }

    deps
}

/// Extract all unique dependency addresses from a set of packages.
/// packages is HashMap<String, Vec<(module_name, bytecode_base64)>>
fn extract_all_dependencies(
    packages: &std::collections::HashMap<String, Vec<(String, String)>>,
) -> std::collections::BTreeSet<String> {
    use std::collections::BTreeSet;

    let mut all_deps: BTreeSet<String> = BTreeSet::new();

    for modules in packages.values() {
        for (_name, bytecode_base64) in modules {
            if let Ok(bytecode) = base64::engine::general_purpose::STANDARD.decode(bytecode_base64)
            {
                for dep_addr in extract_dependencies_from_bytecode(&bytecode) {
                    let addr_str = format!("0x{}", hex::encode(dep_addr.as_ref()));
                    all_deps.insert(addr_str);
                }
            }
        }
    }

    all_deps
}

/// Fetch a transaction and all its dependencies, returning a fully populated CachedTransaction.
///
/// This function automatically:
/// 1. Fetches the transaction from GraphQL
/// 2. Fetches all referenced packages
/// 3. **Recursively fetches transitive package dependencies** (up to max_depth)
/// 4. Fetches all input objects
/// 5. Optionally fetches historical object versions via gRPC
/// 6. Optionally fetches dynamic field children
///
/// # Arguments
/// * `fetcher` - DataFetcher configured for mainnet
/// * `digest` - Transaction digest to fetch
/// * `fetch_historical` - Whether to fetch historical object versions (requires gRPC)
/// * `fetch_dynamic_fields` - Whether to fetch dynamic field children
pub fn fetch_and_cache_transaction(
    fetcher: &crate::data_fetcher::DataFetcher,
    digest: &str,
    _fetch_historical: bool,
    fetch_dynamic_fields: bool,
) -> Result<sui_sandbox_types::CachedTransaction> {
    use crate::graphql::GraphQLTransactionInput;
    use std::collections::BTreeSet;

    // Maximum depth for transitive dependency resolution
    const MAX_DEPENDENCY_DEPTH: usize = 10;

    // Step 1: Fetch transaction
    eprintln!("[fetch_and_cache] Fetching transaction {}...", digest);
    let graphql_tx = fetcher.fetch_transaction(digest)?;
    let fetched_tx = graphql_to_fetched_transaction(&graphql_tx)?;
    let mut cached = sui_sandbox_types::CachedTransaction::new(fetched_tx);

    // Step 2: Extract and fetch all directly referenced packages
    let package_ids = crate::data_fetcher::DataFetcher::extract_package_ids(&graphql_tx);
    eprintln!(
        "[fetch_and_cache] Found {} directly referenced packages",
        package_ids.len()
    );

    let mut fetched_packages: BTreeSet<String> = BTreeSet::new();
    let mut packages_to_fetch: BTreeSet<String> = package_ids.into_iter().collect();

    // Step 3: Recursively fetch transitive dependencies
    for depth in 0..MAX_DEPENDENCY_DEPTH {
        if packages_to_fetch.is_empty() {
            eprintln!(
                "[fetch_and_cache] All dependencies resolved at depth {}",
                depth
            );
            break;
        }

        eprintln!(
            "[fetch_and_cache] Depth {}: fetching {} packages...",
            depth,
            packages_to_fetch.len()
        );

        let mut newly_fetched: Vec<String> = Vec::new();

        for pkg_id in &packages_to_fetch {
            if fetched_packages.contains(pkg_id) {
                continue;
            }

            match fetcher.fetch_package(pkg_id) {
                Ok(pkg) => {
                    let modules: Vec<(String, Vec<u8>)> = pkg
                        .modules
                        .into_iter()
                        .map(|m| (m.name, m.bytecode))
                        .collect();

                    eprintln!(
                        "[fetch_and_cache]   Fetched {}: {} modules",
                        &pkg_id[..20.min(pkg_id.len())],
                        modules.len()
                    );

                    cached.add_package(pkg_id.clone(), modules);
                    newly_fetched.push(pkg_id.clone());
                    fetched_packages.insert(pkg_id.clone());
                }
                Err(e) => {
                    eprintln!(
                        "[fetch_and_cache]   Warning: Failed to fetch {}: {}",
                        &pkg_id[..20.min(pkg_id.len())],
                        e
                    );
                    // Mark as "fetched" to avoid infinite retry
                    fetched_packages.insert(pkg_id.clone());
                }
            }
        }

        // Extract dependencies from newly fetched packages
        let all_deps = extract_all_dependencies(&cached.packages);

        // Find packages we haven't fetched yet
        packages_to_fetch = all_deps
            .into_iter()
            .filter(|p| !fetched_packages.contains(p))
            .collect();

        if packages_to_fetch.is_empty() {
            eprintln!("[fetch_and_cache] No more transitive dependencies to fetch");
            break;
        }

        eprintln!(
            "[fetch_and_cache] Found {} new transitive dependencies",
            packages_to_fetch.len()
        );
    }

    eprintln!(
        "[fetch_and_cache] Total packages fetched: {}",
        cached.packages.len()
    );

    // Build a map of object address -> input version from effects
    // For mutated objects: input_version = output_version - 1
    let mut input_versions: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();
    if let Some(effects) = &graphql_tx.effects {
        for change in &effects.mutated {
            if let Some(output_version) = change.version {
                // Input version is output version minus 1
                let input_version = output_version.saturating_sub(1);
                input_versions.insert(change.address.clone(), input_version);
                eprintln!(
                    "[fetch_and_cache] Object {} mutated: output_version={}, input_version={}",
                    &change.address[..20.min(change.address.len())],
                    output_version,
                    input_version
                );
            }
        }
    }

    // Step 4: Fetch input objects
    for input in &graphql_tx.inputs {
        match input {
            GraphQLTransactionInput::OwnedObject {
                address, version, ..
            } => match fetcher.fetch_object_at_version(address, *version) {
                Ok(obj) => {
                    if let Some(bcs) = obj.bcs_bytes {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&bcs);
                        cached.objects.insert(address.clone(), encoded);
                        cached.object_versions.insert(address.clone(), *version);
                        if let Some(type_str) = obj.type_string {
                            cached.object_types.insert(address.clone(), type_str);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Failed to fetch object {}: {}", address, e);
                }
            },
            GraphQLTransactionInput::SharedObject {
                address,
                initial_shared_version,
                ..
            } => {
                // For shared objects, try to use the computed input version from effects
                // This is more accurate than initial_shared_version which is when the object was first shared
                let version_to_fetch = input_versions.get(address).copied();

                let fetch_result = if let Some(version) = version_to_fetch {
                    eprintln!("[fetch_and_cache] Fetching shared object {} at input version {} (initial_shared_version={})",
                        &address[..20.min(address.len())], version, initial_shared_version);
                    fetcher.fetch_object_at_version(address, version)
                } else {
                    // No version info from effects (read-only object not in mutations)
                    eprintln!("[fetch_and_cache] Fetching shared object {} at initial_shared_version={} (no mutation)",
                        &address[..20.min(address.len())], initial_shared_version);
                    fetcher.fetch_object_at_version(address, *initial_shared_version)
                };

                match fetch_result {
                    Ok(obj) => {
                        if let Some(bcs) = obj.bcs_bytes {
                            let encoded = base64::engine::general_purpose::STANDARD.encode(&bcs);
                            cached.objects.insert(address.clone(), encoded);
                            cached.object_versions.insert(address.clone(), obj.version);
                            if let Some(type_str) = obj.type_string {
                                cached.object_types.insert(address.clone(), type_str);
                            }
                            eprintln!("[fetch_and_cache]   SUCCESS: got version {}", obj.version);
                        }
                    }
                    Err(e) => {
                        // Historical version not available - fall back to current version
                        // Note: This may cause replay differences for objects that changed since the tx
                        eprintln!(
                            "[fetch_and_cache] WARNING: Historical version unavailable for {}: {}",
                            &address[..20.min(address.len())],
                            e
                        );
                        eprintln!("[fetch_and_cache]   Falling back to CURRENT version (may cause replay differences)");

                        if let Ok(obj) = fetcher.fetch_object(address) {
                            if let Some(bcs) = obj.bcs_bytes {
                                let encoded =
                                    base64::engine::general_purpose::STANDARD.encode(&bcs);
                                cached.objects.insert(address.clone(), encoded);
                                cached.object_versions.insert(address.clone(), obj.version);
                                if let Some(type_str) = obj.type_string {
                                    cached.object_types.insert(address.clone(), type_str);
                                }
                                eprintln!("[fetch_and_cache]   Fallback SUCCESS: got version {} (wanted {})",
                                    obj.version, version_to_fetch.unwrap_or(*initial_shared_version));
                            }
                        } else {
                            eprintln!("[fetch_and_cache]   ERROR: Could not fetch object at all");
                        }
                    }
                }
            }
            // Receiving and Pure inputs don't need special fetching
            _ => {}
        }
    }

    // Step 5: Fetch dynamic field children if requested
    if fetch_dynamic_fields {
        // Fetch dynamic fields for all shared objects (they often contain important state)
        for input in &graphql_tx.inputs {
            if let GraphQLTransactionInput::SharedObject { address, .. } = input {
                match fetcher.fetch_dynamic_fields_recursive(address, 2, 100) {
                    Ok(children) => {
                        for child in children {
                            if let (Some(_name_bcs), Some(value_bcs)) =
                                (child.name_bcs, child.value_bcs)
                            {
                                let child_field = sui_sandbox_types::CachedDynamicField {
                                    parent_id: child.parent_address.clone(),
                                    type_string: child.value_type.unwrap_or_default(),
                                    bcs_base64: base64::engine::general_purpose::STANDARD
                                        .encode(&value_bcs),
                                    version: child.version.unwrap_or(0),
                                };
                                cached
                                    .dynamic_field_children
                                    .insert(child.child_address, child_field);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to fetch dynamic fields for {}: {}",
                            address, e
                        );
                    }
                }
            }
        }
    }

    Ok(cached)
}

/// Load a cached transaction from disk, or fetch and cache it if not present.
///
/// This is the main entry point for auto-caching behavior.
///
/// # Arguments
/// * `cache_dir` - Directory to store/load cached transactions
/// * `digest` - Transaction digest
/// * `fetcher` - Optional DataFetcher (created if None and fetch needed)
/// * `fetch_historical` - Whether to fetch historical versions
/// * `fetch_dynamic_fields` - Whether to fetch dynamic fields
pub fn load_or_fetch_transaction(
    cache_dir: &str,
    digest: &str,
    fetcher: Option<&crate::data_fetcher::DataFetcher>,
    fetch_historical: bool,
    fetch_dynamic_fields: bool,
) -> Result<sui_sandbox_types::CachedTransaction> {
    let cache_path = std::path::Path::new(cache_dir).join(format!("{}.json", digest));

    // Try to load from cache first
    if cache_path.exists() {
        let data = std::fs::read_to_string(&cache_path)?;
        let cached: CachedTransaction = serde_json::from_str(&data)?;
        return Ok(cached);
    }

    // Create cache directory if needed
    std::fs::create_dir_all(cache_dir)?;

    // Fetch the transaction - create a new fetcher if none provided
    let owned_fetcher;
    let fetcher_ref = match fetcher {
        Some(f) => f,
        None => {
            owned_fetcher = crate::data_fetcher::DataFetcher::mainnet();
            &owned_fetcher
        }
    };

    let cached =
        fetch_and_cache_transaction(fetcher_ref, digest, fetch_historical, fetch_dynamic_fields)?;

    // Save to cache
    let json = serde_json::to_string_pretty(&cached)?;
    std::fs::write(&cache_path, json)?;

    Ok(cached)
}
