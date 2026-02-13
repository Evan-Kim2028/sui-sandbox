//! Replay support functions shared between the CLI and Python bindings.
//!
//! These functions handle the orchestration layer between raw `ReplayState`
//! (fetched by `sui-state-fetcher`) and the VM execution engine. They cover:
//!
//! - Resolver hydration from replay state
//! - Dependency closure fetching via GraphQL
//! - Object map construction for the VM harness
//! - Object version patching for historical replay
//! - Simulation config construction from replay state

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

use anyhow::Result;
use base64::Engine;
use move_core_types::account_address::AccountAddress;

use sui_state_fetcher::{PackageData, ReplayState};
use sui_transport::decode_graphql_modules;
use sui_transport::graphql::GraphQLClient;

use crate::resolver::LocalModuleResolver;
use crate::utilities::historical_state::HistoricalStateReconstructor;
use crate::vm::SimulationConfig;

// ---------------------------------------------------------------------------
// Resolver hydration
// ---------------------------------------------------------------------------

/// Build a `LocalModuleResolver` from a `ReplayState`, pre-loaded with the
/// Sui framework and all packages from the replay state.
///
/// Unlike the CLI version which clones from `SandboxState`, this starts fresh
/// with `LocalModuleResolver::with_sui_framework()`.
pub fn hydrate_resolver_from_replay_state(
    replay_state: &ReplayState,
    linkage_upgrades: &HashMap<AccountAddress, AccountAddress>,
    aliases: &HashMap<AccountAddress, AccountAddress>,
) -> Result<LocalModuleResolver> {
    let mut resolver = LocalModuleResolver::with_sui_framework()?;

    // Sort packages by (runtime_id, version) for deterministic loading
    let mut packages: Vec<&PackageData> = replay_state.packages.values().collect();
    packages.sort_by(|a, b| {
        let ra = a.runtime_id();
        let rb = b.runtime_id();
        if ra == rb {
            a.version.cmp(&b.version)
        } else {
            ra.as_ref().cmp(rb.as_ref())
        }
    });

    for pkg in packages {
        let _ = resolver.add_package_modules_at(pkg.modules.clone(), Some(pkg.address));
        resolver.add_package_linkage(pkg.address, pkg.runtime_id(), &pkg.linkage);
    }
    for (original, upgraded) in linkage_upgrades {
        resolver.add_linkage_upgrade(*original, *upgraded);
    }
    for (storage, runtime) in aliases {
        resolver.add_address_alias(*storage, *runtime);
    }
    Ok(resolver)
}

// ---------------------------------------------------------------------------
// Dependency closure
// ---------------------------------------------------------------------------

/// Fetch transitive package dependencies via GraphQL until the resolver has
/// no more missing dependencies (up to `MAX_ROUNDS` iterations).
///
/// Returns the number of packages fetched.
pub fn fetch_dependency_closure(
    resolver: &mut LocalModuleResolver,
    graphql: &GraphQLClient,
    checkpoint: Option<u64>,
    verbose: bool,
) -> Result<usize> {
    const MAX_ROUNDS: usize = 8;
    let mut fetched = 0usize;
    let mut seen: BTreeSet<AccountAddress> = BTreeSet::new();

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
            let mut candidates = Vec::new();
            candidates.push(addr);
            if let Some(upgraded) = resolver.get_linkage_upgrade(&addr) {
                candidates.push(upgraded);
            }
            if let Some(alias) = resolver.get_alias(&addr) {
                candidates.push(alias);
            }
            for (target, source) in resolver.get_all_aliases() {
                if source == addr {
                    candidates.push(target);
                }
            }
            candidates.sort();
            candidates.dedup();

            let mut fetched_this = false;
            for candidate in candidates {
                if seen.contains(&candidate) {
                    continue;
                }
                seen.insert(candidate);
                let addr_hex = candidate.to_hex_literal();
                if verbose {
                    eprintln!("[deps] fetching {}", addr_hex);
                }
                let pkg = match checkpoint {
                    Some(cp) => match graphql.fetch_package_at_checkpoint(&addr_hex, cp) {
                        Ok(p) => p,
                        Err(err) => {
                            if verbose {
                                eprintln!(
                                    "[deps] failed to fetch {} at checkpoint {}: {}",
                                    addr_hex, cp, err
                                );
                                eprintln!("[deps] falling back to latest package for {}", addr_hex);
                            }
                            graphql.fetch_package(&addr_hex)?
                        }
                    },
                    None => graphql.fetch_package(&addr_hex)?,
                };
                let modules = decode_graphql_modules(&addr_hex, &pkg.modules)?;
                if modules.is_empty() {
                    if verbose {
                        eprintln!("[deps] no modules for {}", addr_hex);
                    }
                    continue;
                }
                let _ = resolver.add_package_modules_at(modules, Some(candidate));
                fetched += 1;
                fetched_this = true;
                break;
            }
            if !fetched_this && verbose {
                eprintln!(
                    "[deps] failed to fetch any candidate for {}",
                    addr.to_hex_literal()
                );
            }
        }
    }

    Ok(fetched)
}

// ---------------------------------------------------------------------------
// Object maps
// ---------------------------------------------------------------------------

/// Object maps prepared from `ReplayState` for VM execution.
pub struct ReplayObjectMaps {
    /// Package version map: hex address → version
    pub versions_str: HashMap<String, u64>,
    /// Object BCS bytes as base64 strings, keyed by hex ID
    pub cached_objects: HashMap<String, String>,
    /// Object version map: hex ID → version
    pub version_map: HashMap<String, u64>,
    /// Raw object BCS bytes, keyed by hex ID
    pub object_bytes: HashMap<String, Vec<u8>>,
    /// Object type tags, keyed by hex ID
    pub object_types: HashMap<String, String>,
}

/// Convert `ReplayState` objects into the maps needed by the VM harness.
pub fn build_replay_object_maps(
    replay_state: &ReplayState,
    versions: &HashMap<AccountAddress, u64>,
) -> ReplayObjectMaps {
    let versions_str: HashMap<String, u64> = versions
        .iter()
        .map(|(addr, ver)| (addr.to_hex_literal(), *ver))
        .collect();
    let mut cached_objects: HashMap<String, String> = HashMap::new();
    let mut version_map: HashMap<String, u64> = HashMap::new();
    let mut object_bytes: HashMap<String, Vec<u8>> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    for (id, obj) in &replay_state.objects {
        let id_hex = id.to_hex_literal();
        cached_objects.insert(
            id_hex.clone(),
            base64::engine::general_purpose::STANDARD.encode(&obj.bcs_bytes),
        );
        version_map.insert(id_hex.clone(), obj.version);
        object_bytes.insert(id_hex.clone(), obj.bcs_bytes.clone());
        if let Some(type_tag) = &obj.type_tag {
            object_types.insert(id_hex, type_tag.clone());
        }
    }
    ReplayObjectMaps {
        versions_str,
        cached_objects,
        version_map,
        object_bytes,
        object_types,
    }
}

// ---------------------------------------------------------------------------
// Object patching
// ---------------------------------------------------------------------------

/// Patch object bytes for historical version compatibility.
///
/// Uses `HistoricalStateReconstructor` to fix struct layouts that changed
/// between protocol versions. Respects the `SUI_DISABLE_VERSION_PATCH` env var.
pub fn maybe_patch_replay_objects(
    resolver: &LocalModuleResolver,
    replay_state: &ReplayState,
    versions: &HashMap<AccountAddress, u64>,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    maps: &mut ReplayObjectMaps,
    verbose: bool,
) {
    let disable_version_patch = std::env::var("SUI_DISABLE_VERSION_PATCH")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if disable_version_patch {
        return;
    }
    let mut reconstructor = HistoricalStateReconstructor::new();
    reconstructor.configure_from_modules(resolver.iter_modules());
    if let Some(ts) = replay_state.transaction.timestamp_ms {
        reconstructor.set_timestamp(ts);
    }
    for (storage, ver) in versions {
        reconstructor.register_version(&storage.to_hex_literal(), *ver);
    }
    for (storage, runtime) in aliases {
        if let Some(ver) = versions.get(storage) {
            reconstructor.register_version(&runtime.to_hex_literal(), *ver);
        }
    }
    let reconstructed = reconstructor.reconstruct(&maps.object_bytes, &maps.object_types);
    for (id, bytes) in reconstructed.objects {
        maps.cached_objects
            .insert(id, base64::engine::general_purpose::STANDARD.encode(&bytes));
    }
    if verbose {
        let stats = reconstructed.stats;
        if stats.total_patched() > 0 {
            eprintln!(
                "[patch] patched_objects={} overrides={} raw={} struct={} skips={}",
                stats.total_patched(),
                stats.override_patched,
                stats.raw_patched,
                stats.struct_patched,
                stats.skipped
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Simulation config
// ---------------------------------------------------------------------------

/// Build a `SimulationConfig` from a `ReplayState`, setting all replay-relevant
/// fields (sender, gas, epoch, protocol version, timestamp, transaction hash).
pub fn build_simulation_config(replay_state: &ReplayState) -> SimulationConfig {
    use sui_types::digests::TransactionDigest;

    let mut config = SimulationConfig::default()
        .with_sender_address(replay_state.transaction.sender)
        .with_gas_budget(Some(replay_state.transaction.gas_budget))
        .with_gas_price(replay_state.transaction.gas_price)
        .with_epoch(replay_state.epoch);
    if let Some(rgp) = replay_state.reference_gas_price {
        config = config.with_reference_gas_price(rgp);
    }
    if replay_state.protocol_version > 0 {
        config = config.with_protocol_version(replay_state.protocol_version);
    }
    if let Some(ts) = replay_state.transaction.timestamp_ms {
        config = config.with_tx_timestamp(ts);
    }
    if let Ok(digest) = TransactionDigest::from_str(&replay_state.transaction.digest.0) {
        config = config.with_tx_hash(digest.into_inner());
    }
    config
}
