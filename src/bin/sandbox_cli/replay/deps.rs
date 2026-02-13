use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_state_fetcher::{package_data_from_move_package, PackageData, ReplayState};
use sui_transport::decode_graphql_modules;
use sui_transport::graphql::GraphQLClient;

type CachedObjectMap = Arc<parking_lot::Mutex<HashMap<String, (String, Vec<u8>, u64)>>>;
type CachedPackageMap = Arc<parking_lot::Mutex<HashMap<AccountAddress, PackageData>>>;

/// Apply a transaction's output_objects to shared caches for intra-checkpoint
/// state progression. This ensures subsequent transactions in the same checkpoint
/// see post-execution object state from earlier transactions.
pub(super) fn apply_output_objects_to_cache(
    obj_cache: &CachedObjectMap,
    pkg_cache: &CachedPackageMap,
    tx: &sui_types::full_checkpoint_content::CheckpointTransaction,
) {
    for obj in &tx.output_objects {
        match &obj.data {
            sui_types::object::Data::Package(pkg) => {
                let pkg_data = package_data_from_move_package(pkg);
                pkg_cache.lock().insert(pkg_data.address, pkg_data);
            }
            _ => {
                let oid = format!("0x{}", hex::encode(obj.id().into_bytes()));
                if let Some((ts, bcs, ver, _shared)) =
                    sui_transport::walrus::extract_object_bcs(obj)
                {
                    obj_cache.lock().insert(oid, (ts, bcs, ver));
                }
            }
        }
    }
}

// Re-export from library crate — shared between CLI and Python bindings.
pub(super) use sui_sandbox_core::replay_support::fetch_dependency_closure;

/// Fetch a package via Walrus, resolving to the correct version for a target checkpoint.
///
/// For packages with upgrades, walks the upgrade chain to find the latest version
/// whose publish transaction is at or before the target checkpoint. This ensures
/// we load the exact package version that was active during execution.
pub(super) fn fetch_package_via_walrus(
    gql: &GraphQLClient,
    pkg_cache: &parking_lot::Mutex<HashMap<AccountAddress, PackageData>>,
    address: &str,
    verbose: bool,
) -> Option<PackageData> {
    if let Some(pkg) = fetch_single_package_from_walrus(gql, pkg_cache, address, verbose) {
        return Some(pkg);
    }
    None
}

/// Walrus-backed dependency closure: resolves transitive package dependencies
/// using `fetch_package_via_walrus` (previousTransaction → checkpoint) for
/// correct historical versions, falling back to GraphQL for system packages.
pub(super) fn fetch_dependency_closure_walrus(
    resolver: &mut LocalModuleResolver,
    graphql: &GraphQLClient,
    pkg_cache: &parking_lot::Mutex<HashMap<AccountAddress, PackageData>>,
    replay_state: &mut ReplayState,
    verbose: bool,
) -> Result<usize> {
    const MAX_ROUNDS: usize = 8;
    let mut fetched = 0usize;
    let mut seen: BTreeSet<AccountAddress> = BTreeSet::new();
    // Track which storage addresses have already been fetched to avoid duplicates
    let mut fetched_storage: BTreeSet<AccountAddress> = BTreeSet::new();
    let target_checkpoint = replay_state.checkpoint.unwrap_or(u64::MAX);

    // Build a multi-map of all linkage targets: original -> {storage_addr, ...}
    // Different packages may reference different versions of the same dependency.
    // We must load ALL versions because different calling packages may need
    // different versions during VM execution.
    let mut all_linkage_targets: HashMap<AccountAddress, BTreeSet<AccountAddress>> = HashMap::new();
    for pkg in replay_state.packages.values() {
        for (original, storage) in &pkg.linkage {
            if original != storage {
                all_linkage_targets
                    .entry(*original)
                    .or_default()
                    .insert(*storage);
            }
        }
    }

    // Proactively fetch upgraded package versions from linkage tables.
    // When package A's linkage says dep 0x91bf -> 0xa5a0, we need the V2 bytecode
    // at 0xa5a0 even though V1 at 0x91bf may already be loaded. The V2 package has
    // new structs/functions that V1 doesn't, causing LOOKUP_FAILED in the verifier.
    {
        let loaded = resolver.loaded_packages();
        let targets_to_fetch: Vec<AccountAddress> = all_linkage_targets
            .values()
            .flat_map(|targets| targets.iter())
            .copied()
            .filter(|addr| !loaded.contains(addr) && !fetched_storage.contains(addr))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for storage_addr in targets_to_fetch {
            let storage_hex = storage_addr.to_hex_literal();
            if verbose {
                eprintln!("[deps] proactive upgrade fetch: {}", storage_hex);
            }
            let pkg_data =
                fetch_single_package_from_walrus(graphql, pkg_cache, &storage_hex, verbose);
            if let Some(pkg_data) = pkg_data {
                register_dep_package(
                    &pkg_data,
                    resolver,
                    replay_state,
                    &mut all_linkage_targets,
                    &mut seen,
                    &mut fetched_storage,
                    verbose,
                );
                fetched += 1;
            } else if verbose {
                eprintln!("[deps] failed to proactively fetch {}", storage_hex);
            }
        }
    }

    for _ in 0..MAX_ROUNDS {
        let missing = resolver.get_missing_dependencies();
        let pending: Vec<AccountAddress> = missing
            .into_iter()
            .filter(|addr| !seen.contains(addr))
            .collect();
        if pending.is_empty() {
            break;
        }
        for addr in pending {
            seen.insert(addr);
            let addr_hex = addr.to_hex_literal();

            // Collect ALL storage addresses for this dependency from all packages'
            // linkage tables. Different packages may be compiled against different
            // versions, and we need to load all of them for the VM to work correctly.
            let linkage_targets: Vec<AccountAddress> = all_linkage_targets
                .get(&addr)
                .map(|set| set.iter().copied().collect())
                .unwrap_or_default();

            if linkage_targets.is_empty() {
                // No linkage entry: resolve to the latest version at the target checkpoint
                if verbose {
                    eprintln!(
                        "[deps] resolving {} at checkpoint {} (no linkage)",
                        addr_hex, target_checkpoint
                    );
                }
                let pkg_data = resolve_package_at_checkpoint(
                    graphql,
                    pkg_cache,
                    &addr_hex,
                    target_checkpoint,
                    verbose,
                );
                if let Some(pkg_data) = pkg_data {
                    register_dep_package(
                        &pkg_data,
                        resolver,
                        replay_state,
                        &mut all_linkage_targets,
                        &mut seen,
                        &mut fetched_storage,
                        verbose,
                    );
                    fetched += 1;
                } else {
                    fallback_graphql_dep_fetch(
                        graphql,
                        resolver,
                        &addr,
                        &addr_hex,
                        &mut fetched,
                        verbose,
                    )?;
                }
            } else {
                // Fetch ALL versions specified by different packages' linkage tables
                for storage_addr in &linkage_targets {
                    if fetched_storage.contains(storage_addr) {
                        continue;
                    }
                    let storage_hex = storage_addr.to_hex_literal();
                    if verbose {
                        eprintln!(
                            "[deps] resolving {} via linkage -> {} (exact version)",
                            addr_hex, storage_hex
                        );
                    }
                    let pkg_data =
                        fetch_single_package_from_walrus(graphql, pkg_cache, &storage_hex, verbose);
                    if let Some(pkg_data) = pkg_data {
                        register_dep_package(
                            &pkg_data,
                            resolver,
                            replay_state,
                            &mut all_linkage_targets,
                            &mut seen,
                            &mut fetched_storage,
                            verbose,
                        );
                        fetched += 1;
                    } else if verbose {
                        eprintln!("[deps] failed to fetch linkage target {}", storage_hex);
                    }
                }
                // If no linkage target was successfully fetched, try fallback
                if linkage_targets.iter().all(|s| !fetched_storage.contains(s)) {
                    fallback_graphql_dep_fetch(
                        graphql,
                        resolver,
                        &addr,
                        &addr_hex,
                        &mut fetched,
                        verbose,
                    )?;
                }
            }
        }
    }

    Ok(fetched)
}

/// Fetch a single package version from Walrus by address.
/// Uses previousTransaction -> checkpoint -> Walrus pipeline.
fn fetch_single_package_from_walrus(
    gql: &GraphQLClient,
    pkg_cache: &parking_lot::Mutex<HashMap<AccountAddress, PackageData>>,
    address: &str,
    verbose: bool,
) -> Option<PackageData> {
    let addr = AccountAddress::from_hex_literal(address).ok()?;

    if let Some(cached) = pkg_cache.lock().get(&addr) {
        return Some(cached.clone());
    }

    let prev_tx = match gql.fetch_object(address) {
        Ok(obj) => obj.previous_transaction,
        Err(e) => {
            if verbose {
                eprintln!("[walrus-pkg] failed to fetch object {}: {}", address, e);
            }
            return None;
        }
    };

    let prev_tx_digest = prev_tx.as_deref()?;

    let tx_meta = match gql.fetch_transaction_meta(prev_tx_digest) {
        Ok(m) => m,
        Err(e) => {
            if verbose {
                eprintln!(
                    "[walrus-pkg] failed to get tx meta for {}: {}",
                    prev_tx_digest, e
                );
            }
            return None;
        }
    };
    let cp = tx_meta.checkpoint?;

    if verbose {
        eprintln!(
            "[walrus-pkg] {} -> prevTx {} -> checkpoint {}",
            address, prev_tx_digest, cp
        );
    }

    let walrus = sui_transport::walrus::WalrusClient::mainnet();
    let cp_data = match walrus.get_checkpoint(cp) {
        Ok(d) => d,
        Err(e) => {
            if verbose {
                eprintln!(
                    "[walrus-pkg] failed to fetch checkpoint {} from Walrus: {}",
                    cp, e
                );
            }
            return None;
        }
    };

    let mut found = None;
    {
        let mut cache = pkg_cache.lock();
        for tx in &cp_data.transactions {
            for obj in tx.input_objects.iter().chain(tx.output_objects.iter()) {
                if let sui_types::object::Data::Package(pkg) = &obj.data {
                    let pkg_data = package_data_from_move_package(pkg);
                    let pkg_addr = pkg_data.address;
                    if pkg_addr == addr {
                        found = Some(pkg_data.clone());
                    }
                    cache.entry(pkg_addr).or_insert(pkg_data);
                }
            }
        }
    }

    if found.is_none() && verbose {
        eprintln!(
            "[walrus-pkg] package {} not found in checkpoint {} (may have been published earlier)",
            address, cp
        );
    }

    found
}

/// Resolve the correct package version for a specific target checkpoint.
///
/// Given an original (runtime) package address, finds the latest upgrade version
/// whose previousTransaction checkpoint is <= target_checkpoint. This handles cases
/// where linkage tables are stale (compiled against older versions).
fn resolve_package_at_checkpoint(
    gql: &GraphQLClient,
    pkg_cache: &parking_lot::Mutex<HashMap<AccountAddress, PackageData>>,
    original_address: &str,
    target_checkpoint: u64,
    verbose: bool,
) -> Option<PackageData> {
    if let Ok(addr) = AccountAddress::from_hex_literal(original_address) {
        if let Some(cached) = pkg_cache.lock().get(&addr) {
            return Some(cached.clone());
        }
    }

    let upgrades = match gql.get_package_upgrades(original_address) {
        Ok(u) => u,
        Err(e) => {
            if verbose {
                eprintln!(
                    "[walrus-pkg] failed to get upgrade chain for {}: {}",
                    original_address, e
                );
            }
            return fetch_single_package_from_walrus(gql, pkg_cache, original_address, verbose);
        }
    };

    if upgrades.len() <= 1 {
        return fetch_single_package_from_walrus(gql, pkg_cache, original_address, verbose);
    }

    if verbose {
        eprintln!(
            "[walrus-pkg] {} has {} versions, resolving for checkpoint {}",
            original_address,
            upgrades.len(),
            target_checkpoint
        );
    }

    // Each entry is (address, version), ordered oldest -> newest.
    for (addr, ver) in upgrades.iter().rev() {
        if let Ok(pkg_addr) = AccountAddress::from_hex_literal(addr) {
            if let Some(cached) = pkg_cache.lock().get(&pkg_addr) {
                if verbose {
                    eprintln!("[walrus-pkg] using cached v{} at {}", cached.version, addr);
                }
                return Some(cached.clone());
            }
        }

        let prev_tx = match gql.fetch_object(addr) {
            Ok(obj) => obj.previous_transaction,
            Err(_) => continue,
        };
        if let Some(prev_digest) = prev_tx.as_deref() {
            if let Ok(meta) = gql.fetch_transaction_meta(prev_digest) {
                if let Some(publish_cp) = meta.checkpoint {
                    if publish_cp <= target_checkpoint {
                        if verbose {
                            eprintln!(
                                "[walrus-pkg] resolved {} v{} at {} (published at checkpoint {})",
                                original_address, ver, addr, publish_cp
                            );
                        }
                        return fetch_single_package_from_walrus(gql, pkg_cache, addr, verbose);
                    }
                }
            }
        }
    }

    if verbose {
        eprintln!(
            "[walrus-pkg] no version of {} found before checkpoint {}",
            original_address, target_checkpoint
        );
    }
    fetch_single_package_from_walrus(gql, pkg_cache, original_address, verbose)
}

/// Register a fetched dependency package: add to resolver, update linkage multi-map,
/// track aliases and seen addresses.
fn register_dep_package(
    pkg_data: &PackageData,
    resolver: &mut LocalModuleResolver,
    replay_state: &mut ReplayState,
    all_linkage_targets: &mut HashMap<AccountAddress, BTreeSet<AccountAddress>>,
    seen: &mut BTreeSet<AccountAddress>,
    fetched_storage: &mut BTreeSet<AccountAddress>,
    verbose: bool,
) {
    if verbose {
        eprintln!(
            "[deps] got {} v{} original={:?}",
            pkg_data.address.to_hex_literal(),
            pkg_data.version,
            pkg_data.original_id.map(|a| a.to_hex_literal()),
        );
    }
    let _ = resolver.add_package_modules_at(pkg_data.modules.clone(), Some(pkg_data.address));
    resolver.add_package_linkage(pkg_data.address, pkg_data.runtime_id(), &pkg_data.linkage);
    for (original, upgraded) in &pkg_data.linkage {
        resolver.add_linkage_upgrade(*original, *upgraded);
        // Also update the multi-map so newly discovered linkage targets
        // are available for subsequent rounds.
        if original != upgraded {
            all_linkage_targets
                .entry(*original)
                .or_default()
                .insert(*upgraded);
        }
    }
    if let Some(orig_id) = pkg_data.original_id {
        if orig_id != pkg_data.address {
            resolver.add_address_alias(pkg_data.address, orig_id);
            seen.insert(orig_id);
        }
    }
    fetched_storage.insert(pkg_data.address);
    replay_state
        .packages
        .insert(pkg_data.address, pkg_data.clone());
}

/// Fallback: fetch a dependency package via GraphQL when Walrus resolution fails.
fn fallback_graphql_dep_fetch(
    graphql: &GraphQLClient,
    resolver: &mut LocalModuleResolver,
    addr: &AccountAddress,
    addr_hex: &str,
    fetched: &mut usize,
    verbose: bool,
) -> Result<()> {
    if verbose {
        eprintln!(
            "[deps] Walrus resolution failed for {}, trying GraphQL",
            addr_hex
        );
    }
    match graphql.fetch_package(addr_hex) {
        Ok(pkg) => {
            let modules = decode_graphql_modules(addr_hex, &pkg.modules)?;
            if modules.is_empty() {
                if verbose {
                    eprintln!("[deps] no modules for {}", addr_hex);
                }
                return Ok(());
            }
            let _ = resolver.add_package_modules_at(modules, Some(*addr));
            *fetched += 1;
            Ok(())
        }
        Err(e) => {
            if verbose {
                eprintln!("[deps] failed to fetch {}: {}", addr_hex, e);
            }
            Ok(())
        }
    }
}
