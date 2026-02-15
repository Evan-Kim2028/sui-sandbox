use std::collections::HashMap;

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;
use sui_sandbox_core::replay_support::{self, ReplayObjectMaps};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::vm::SimulationConfig;
use sui_sandbox_types::{
    normalize_address as normalize_address_shared, synthesize_clock_bytes, synthesize_random_bytes,
    CLOCK_OBJECT_ID, CLOCK_TYPE_STR, DEFAULT_CLOCK_BASE_MS, RANDOM_OBJECT_ID, RANDOM_TYPE_STR,
};
use sui_state_fetcher::{PackageData, ReplayState, VersionedObject};

use super::super::SandboxState;

pub(super) fn hydrate_resolver_from_replay_state(
    state: &SandboxState,
    replay_state: &ReplayState,
    linkage_upgrades: &HashMap<AccountAddress, AccountAddress>,
    aliases: &HashMap<AccountAddress, AccountAddress>,
) -> LocalModuleResolver {
    let mut resolver = state.resolver.clone();
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
    resolver
}

pub(super) fn build_replay_object_maps(
    replay_state: &ReplayState,
    versions: &HashMap<AccountAddress, u64>,
) -> ReplayObjectMaps {
    replay_support::build_replay_object_maps(replay_state, versions)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn maybe_patch_replay_objects(
    resolver: &LocalModuleResolver,
    replay_state: &ReplayState,
    versions: &HashMap<AccountAddress, u64>,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    maps: &mut ReplayObjectMaps,
    progress_logging: bool,
    patch_stats_logging: bool,
) {
    if progress_logging {
        eprintln!("[replay] version patcher start");
    }
    replay_support::maybe_patch_replay_objects(
        resolver,
        replay_state,
        versions,
        aliases,
        maps,
        patch_stats_logging,
    );
    if progress_logging {
        eprintln!("[replay] version patcher done");
    }
}

pub(super) fn build_simulation_config(replay_state: &ReplayState) -> SimulationConfig {
    replay_support::build_simulation_config(replay_state)
}

pub(super) fn ensure_system_objects(
    objects: &mut HashMap<AccountAddress, VersionedObject>,
    historical_versions: &HashMap<String, u64>,
    tx_timestamp_ms: Option<u64>,
    checkpoint: Option<u64>,
) {
    let clock_id = CLOCK_OBJECT_ID;
    objects.entry(clock_id).or_insert_with(|| {
        let clock_version = historical_versions
            .get(&normalize_address_shared(&clock_id.to_hex_literal()))
            .copied()
            .or(checkpoint)
            .unwrap_or(1);
        let clock_ts = tx_timestamp_ms.unwrap_or(DEFAULT_CLOCK_BASE_MS);
        VersionedObject {
            id: clock_id,
            version: clock_version,
            digest: None,
            type_tag: Some(CLOCK_TYPE_STR.to_string()),
            bcs_bytes: synthesize_clock_bytes(&clock_id, clock_ts),
            is_shared: true,
            is_immutable: false,
        }
    });

    let random_id = RANDOM_OBJECT_ID;
    objects.entry(random_id).or_insert_with(|| {
        let random_version = historical_versions
            .get(&normalize_address_shared(&random_id.to_hex_literal()))
            .copied()
            .or(checkpoint)
            .unwrap_or(1);
        VersionedObject {
            id: random_id,
            version: random_version,
            digest: None,
            type_tag: Some(RANDOM_TYPE_STR.to_string()),
            bcs_bytes: synthesize_random_bytes(&random_id, random_version),
            is_shared: true,
            is_immutable: false,
        }
    });
}

pub(super) fn emit_linkage_debug_info(
    resolver: &LocalModuleResolver,
    aliases: &HashMap<AccountAddress, AccountAddress>,
) {
    if let Ok(addrs) = std::env::var("SUI_DUMP_PACKAGE_MODULES") {
        for addr_str in addrs.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            if let Ok(addr) = AccountAddress::from_hex_literal(addr_str) {
                let mut modules = resolver.get_package_modules(&addr);
                let alias = resolver
                    .get_alias(&addr)
                    .or_else(|| aliases.get(&addr).copied());
                if modules.is_empty() {
                    if let Some(alias_addr) = alias {
                        modules = resolver.get_package_modules(&alias_addr);
                    }
                }
                match alias {
                    Some(alias_addr) if alias_addr != addr => eprintln!(
                        "[linkage] package_modules addr={} alias={} count={} [{}]",
                        addr.to_hex_literal(),
                        alias_addr.to_hex_literal(),
                        modules.len(),
                        modules.join(", ")
                    ),
                    _ => eprintln!(
                        "[linkage] package_modules addr={} count={} [{}]",
                        addr.to_hex_literal(),
                        modules.len(),
                        modules.join(", ")
                    ),
                }
            }
        }
    }

    if let Ok(addr_str) = std::env::var("SUI_CHECK_ALIAS") {
        if let Ok(addr) = AccountAddress::from_hex_literal(addr_str.trim()) {
            match resolver
                .get_alias(&addr)
                .or_else(|| aliases.get(&addr).copied())
            {
                Some(alias) => eprintln!(
                    "[linkage] alias_check {} -> {}",
                    addr.to_hex_literal(),
                    alias.to_hex_literal()
                ),
                None => eprintln!("[linkage] alias_check {} not found", addr.to_hex_literal()),
            }
        }
    }

    if let Ok(spec) = std::env::var("SUI_DUMP_MODULE_FUNCTIONS") {
        if let Some((addr_str, module_name)) = spec.split_once("::") {
            if let (Ok(addr), Ok(ident)) = (
                AccountAddress::from_hex_literal(addr_str),
                Identifier::new(module_name.to_string()),
            ) {
                let id = ModuleId::new(addr, ident);
                if let Some(module) = resolver.get_module_struct(&id) {
                    let mut names = Vec::new();
                    for def in &module.function_defs {
                        let handle = &module.function_handles[def.function.0 as usize];
                        let name = module.identifier_at(handle.name).to_string();
                        names.push(name);
                    }
                    names.sort();
                    eprintln!(
                        "[linkage] module_functions {}::{} count={} [{}]",
                        addr.to_hex_literal(),
                        module_name,
                        names.len(),
                        names.join(", ")
                    );
                } else {
                    eprintln!(
                        "[linkage] module_functions {}::{} not found",
                        addr.to_hex_literal(),
                        module_name
                    );
                }
            }
        }
    }

    if std::env::var("SUI_DEBUG_LINKAGE")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        let missing = resolver.get_missing_dependencies();
        if !missing.is_empty() {
            let list = missing
                .iter()
                .map(|addr| addr.to_hex_literal())
                .collect::<Vec<_>>();
            eprintln!(
                "[linkage] resolver_missing_dependencies={} [{}]",
                list.len(),
                list.join(", ")
            );
        } else {
            eprintln!("[linkage] resolver_missing_dependencies=0");
        }
    }
}
