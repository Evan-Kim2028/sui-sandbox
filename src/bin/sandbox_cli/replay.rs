//! Replay command - replay historical transactions locally

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use clap::{Parser, ValueEnum};
use move_binary_format::CompiledModule;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use super::network::{cache_dir, infer_network, resolve_graphql_endpoint};
use super::output::format_error;
use super::SandboxState;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{ModuleId, TypeTag};
use sui_prefetch::compute_dynamic_field_id;
use sui_sandbox_core::mm2::{TypeModel, TypeSynthesizer};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy, MissingInputObject};
use sui_sandbox_core::types::{format_type_tag, parse_type_tag};
use sui_sandbox_core::utilities::historical_state::HistoricalStateReconstructor;
use sui_sandbox_core::utilities::rewrite_type_tag;
use sui_sandbox_core::vm::SimulationConfig;
use sui_state_fetcher::{
    build_aliases as build_aliases_shared, fetch_child_object as fetch_child_object_shared,
    fetch_object_via_grpc as fetch_object_via_grpc_shared, HistoricalStateProvider, PackageData,
    VersionedCache,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;
use sui_types::digests::TransactionDigest;

#[derive(Parser, Debug)]
pub struct ReplayCmd {
    /// Transaction digest
    pub digest: String,

    /// Compare local execution with on-chain effects
    #[arg(long)]
    pub compare: bool,

    /// Show detailed execution trace
    #[arg(long, short)]
    pub verbose: bool,

    /// Prefetch depth for dynamic fields (default: 3)
    #[arg(long, default_value_t = 3)]
    pub prefetch_depth: usize,

    /// Prefetch limit for dynamic fields (default: 200)
    #[arg(long, default_value_t = 200)]
    pub prefetch_limit: usize,

    /// Fetch strategy for dynamic field children during replay
    #[arg(long, value_enum, default_value = "full")]
    pub fetch_strategy: FetchStrategy,

    /// Auto-inject system objects (Clock/Random) when missing
    #[arg(long, default_value_t = true)]
    pub auto_system_objects: bool,

    /// Reconcile dynamic-field effects when on-chain lists omit them
    #[arg(long, default_value_t = true)]
    pub reconcile_dynamic_fields: bool,

    /// If replay fails due to missing input objects, synthesize placeholders and retry
    #[arg(long, default_value_t = false)]
    pub synthesize_missing: bool,

    /// Allow dynamic-field reads to synthesize placeholder values when data is missing
    #[arg(long, default_value_t = false)]
    pub self_heal_dynamic_fields: bool,
}

#[derive(Debug, Serialize)]
pub struct ReplayOutput {
    pub digest: String,
    pub local_success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<ComparisonResult>,
    pub commands_executed: usize,
}

#[derive(Debug, Serialize)]
pub struct ComparisonResult {
    pub status_match: bool,
    pub created_match: bool,
    pub mutated_match: bool,
    pub deleted_match: bool,
    pub on_chain_status: String,
    pub local_status: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum FetchStrategy {
    Eager,
    Full,
}

impl ReplayCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let result = self.execute_inner(state, verbose || self.verbose).await;

        match result {
            Ok(output) => {
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    print_replay_result(&output, self.compare);
                }

                if output.local_success {
                    Ok(())
                } else {
                    Err(anyhow!(output
                        .local_error
                        .unwrap_or_else(|| "Replay failed".to_string())))
                }
            }
            Err(e) => {
                eprintln!("{}", format_error(&e, json_output));
                Err(e)
            }
        }
    }

    async fn execute_inner(&self, state: &SandboxState, verbose: bool) -> Result<ReplayOutput> {
        if verbose {
            eprintln!("Fetching transaction {}...", self.digest);
        }

        // Fetch full replay state using gRPC + GraphQL
        let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
        let network = infer_network(&state.rpc_url, &graphql_endpoint);
        let cache = Arc::new(VersionedCache::with_storage(cache_dir(&network))?);
        let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
        let grpc_client = GrpcClient::with_api_key(&state.rpc_url, api_key).await?;
        let graphql_client = sui_transport::graphql::GraphQLClient::new(&graphql_endpoint);
        let provider = Arc::new(
            HistoricalStateProvider::with_clients(grpc_client, graphql_client)
                .with_walrus_from_env()
                .with_local_object_store_from_env()
                .with_cache(cache),
        );

        let replay_state = provider
            .replay_state_builder()
            .prefetch_dynamic_fields(self.fetch_strategy == FetchStrategy::Full)
            .dynamic_field_depth(self.prefetch_depth)
            .dynamic_field_limit(self.prefetch_limit)
            .auto_system_objects(self.auto_system_objects)
            .build(&self.digest)
            .await
            .context("Failed to fetch replay state")?;

        if verbose {
            eprintln!(
                "  Sender: {}",
                replay_state.transaction.sender.to_hex_literal()
            );
            eprintln!("  Commands: {}", replay_state.transaction.commands.len());
            eprintln!("  Inputs: {}", replay_state.transaction.inputs.len());
        }

        let pkg_aliases = build_aliases_shared(
            &replay_state.packages,
            Some(provider.as_ref()),
            replay_state.checkpoint,
        );

        // Create a resolver with packages from replay state
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
        }
        for (original, upgraded) in &pkg_aliases.linkage_upgrades {
            resolver.add_linkage_upgrade(*original, *upgraded);
        }
        for (storage, runtime) in &pkg_aliases.aliases {
            resolver.add_address_alias(*storage, *runtime);
        }

        let fetched_deps = fetch_dependency_closure(
            &mut resolver,
            provider.graphql(),
            replay_state.checkpoint,
            verbose,
        )
        .unwrap_or(0);
        if verbose && fetched_deps > 0 {
            eprintln!(
                "[deps] fetched {} missing dependency packages",
                fetched_deps
            );
        }
        if let Ok(addrs) = std::env::var("SUI_DUMP_PACKAGE_MODULES") {
            for addr_str in addrs.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                if let Ok(addr) = AccountAddress::from_hex_literal(addr_str) {
                    let modules = resolver.get_package_modules(&addr);
                    eprintln!(
                        "[linkage] package_modules addr={} count={} [{}]",
                        addr.to_hex_literal(),
                        modules.len(),
                        modules.join(", ")
                    );
                }
            }
        }
        if let Ok(addr_str) = std::env::var("SUI_CHECK_ALIAS") {
            if let Ok(addr) = AccountAddress::from_hex_literal(addr_str.trim()) {
                match pkg_aliases.aliases.get(&addr) {
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

        if verbose {
            eprintln!("Executing locally...");
        }

        let versions_str: HashMap<String, u64> = pkg_aliases
            .versions
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

        let disable_version_patch = std::env::var("SUI_DISABLE_VERSION_PATCH")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        if !disable_version_patch {
            let mut reconstructor = HistoricalStateReconstructor::new();
            reconstructor.configure_from_modules(resolver.iter_modules());
            if let Some(ts) = replay_state.transaction.timestamp_ms {
                reconstructor.set_timestamp(ts);
            }
            // Register versions for both storage and runtime addresses.
            for (storage, ver) in &pkg_aliases.versions {
                let storage_hex = storage.to_hex_literal();
                reconstructor.register_version(&storage_hex, *ver);
            }
            for (storage, runtime) in &pkg_aliases.aliases {
                if let Some(ver) = pkg_aliases.versions.get(storage) {
                    let runtime_hex = runtime.to_hex_literal();
                    reconstructor.register_version(&runtime_hex, *ver);
                }
            }
            let reconstructed = reconstructor.reconstruct(&object_bytes, &object_types);
            for (id, bytes) in reconstructed.objects {
                cached_objects.insert(id, base64::engine::general_purpose::STANDARD.encode(&bytes));
            }
            if verbose
                || std::env::var("SUI_DEBUG_PATCHER")
                    .ok()
                    .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                    .unwrap_or(false)
            {
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

        let synth_modules = if self.self_heal_dynamic_fields {
            let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
            if modules.is_empty() {
                if verbose {
                    eprintln!("[self_heal] no modules loaded; dynamic-field synthesis disabled");
                }
                None
            } else {
                Some(Arc::new(modules))
            }
        } else {
            None
        };

        let reconcile_policy = if self.reconcile_dynamic_fields {
            EffectsReconcilePolicy::DynamicFields
        } else {
            EffectsReconcilePolicy::Strict
        };
        let make_harness =
            |version_map: &HashMap<String, u64>| -> Result<sui_sandbox_core::vm::VMHarness> {
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
                if let Ok(digest) = TransactionDigest::from_str(&replay_state.transaction.digest.0)
                {
                    config = config.with_tx_hash(digest.into_inner());
                }
                let mut harness =
                    sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
                harness.set_address_aliases_with_versions(
                    pkg_aliases.aliases.clone(),
                    versions_str.clone(),
                );

                let max_version = version_map.values().copied().max().unwrap_or(0);
                if self.fetch_strategy == FetchStrategy::Full {
                    let provider_clone = Arc::clone(&provider);
                    let provider_clone_for_key = Arc::clone(&provider);
                    let checkpoint = replay_state.checkpoint;
                    let synth_modules_for_fetcher = synth_modules.clone();
                    let self_heal_dynamic_fields = self.self_heal_dynamic_fields;
                    let fetcher = move |_parent: AccountAddress, child_id: AccountAddress| {
                        fetch_child_object_shared(
                            &provider_clone,
                            child_id,
                            checkpoint,
                            max_version,
                        )
                    };
                    harness.set_versioned_child_fetcher(Box::new(fetcher));

                    let alias_map = pkg_aliases.aliases.clone();
                    let alias_map_for_fetcher = alias_map.clone();
                    let child_id_aliases: Arc<
                        parking_lot::Mutex<HashMap<AccountAddress, AccountAddress>>,
                    > = Arc::new(parking_lot::Mutex::new(HashMap::new()));
                    let child_id_aliases_for_fetcher = child_id_aliases.clone();
                    let debug_df = matches!(
                        std::env::var("SUI_DEBUG_DF_FETCH")
                            .ok()
                            .as_deref()
                            .map(|v| v.to_ascii_lowercase())
                            .as_deref(),
                        Some("1") | Some("true") | Some("yes") | Some("on")
                    );
                    let debug_df_full = matches!(
                        std::env::var("SUI_DEBUG_DF_FETCH_FULL")
                            .ok()
                            .as_deref()
                            .map(|v| v.to_ascii_lowercase())
                            .as_deref(),
                        Some("1") | Some("true") | Some("yes") | Some("on")
                    );
                    let miss_cache: Arc<parking_lot::Mutex<HashMap<String, MissEntry>>> =
                        Arc::new(parking_lot::Mutex::new(HashMap::new()));
                    let log_self_heal = matches!(
                        std::env::var("SUI_SELF_HEAL_LOG")
                            .ok()
                            .as_deref()
                            .map(|v| v.to_ascii_lowercase())
                            .as_deref(),
                        Some("1") | Some("true") | Some("yes") | Some("on")
                    ) || verbose;
                    let key_fetcher =
                        move |parent: AccountAddress,
                              child_id: AccountAddress,
                              key_type: &TypeTag,
                              key_bytes: &[u8]| {
                            let options = ChildFetchOptions {
                                provider: &provider_clone_for_key,
                                checkpoint,
                                max_version,
                                aliases: &alias_map_for_fetcher,
                                child_id_aliases: &child_id_aliases_for_fetcher,
                                miss_cache: Some(&miss_cache),
                                debug_df,
                                debug_df_full,
                                self_heal_dynamic_fields,
                                synth_modules: synth_modules_for_fetcher.clone(),
                                log_self_heal,
                            };
                            fetch_child_object_by_key(
                                &options, parent, child_id, key_type, key_bytes,
                            )
                        };
                    harness.set_key_based_child_fetcher(Box::new(key_fetcher));
                    harness.set_child_id_aliases(child_id_aliases.clone());

                    let resolver_cache: Arc<Mutex<HashMap<String, TypeTag>>> =
                        Arc::new(Mutex::new(HashMap::new()));
                    let provider_clone_for_resolver = Arc::clone(&provider);
                    let child_id_aliases_for_resolver = child_id_aliases.clone();
                    let alias_map_for_resolver = alias_map;
                    let resolver_checkpoint = replay_state.checkpoint;
                    let key_type_resolver = move |parent: AccountAddress,
                                                  key_bytes: &[u8]|
                          -> Option<TypeTag> {
                        let parent_hex = parent.to_hex_literal();
                        let key_b64 = base64::engine::general_purpose::STANDARD.encode(key_bytes);
                        let cache_key = format!("{}:{}", parent_hex, key_b64);
                        if let Ok(cache) = resolver_cache.lock() {
                            if let Some(tag) = cache.get(&cache_key) {
                                return Some(tag.clone());
                            }
                        }
                        let gql = provider_clone_for_resolver.graphql();
                        let enum_limit = std::env::var("SUI_DF_ENUM_LIMIT")
                            .ok()
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(1000);
                        let field = match resolver_checkpoint {
                            Some(cp) => gql
                                .find_dynamic_field_by_bcs(
                                    &parent_hex,
                                    key_bytes,
                                    Some(cp),
                                    enum_limit,
                                )
                                .or_else(|_| {
                                    gql.find_dynamic_field_by_bcs(
                                        &parent_hex,
                                        key_bytes,
                                        None,
                                        enum_limit,
                                    )
                                }),
                            None => gql.find_dynamic_field_by_bcs(
                                &parent_hex,
                                key_bytes,
                                None,
                                enum_limit,
                            ),
                        };
                        if let Ok(Some(df)) = field {
                            if let Ok(tag) = parse_type_tag(&df.name_type) {
                                if let Some(object_id) = df.object_id.as_deref() {
                                    let mut candidate_tags = vec![tag.clone()];
                                    let rewritten =
                                        rewrite_type_tag(tag.clone(), &alias_map_for_resolver);
                                    if rewritten != tag {
                                        candidate_tags.push(rewritten);
                                    }
                                    for candidate in candidate_tags {
                                        if let Ok(type_bcs) = bcs::to_bytes(&candidate) {
                                            if let Some(computed_hex) = compute_dynamic_field_id(
                                                &parent_hex,
                                                key_bytes,
                                                &type_bcs,
                                            ) {
                                                if let (Ok(computed_id), Ok(actual_id)) = (
                                                    AccountAddress::from_hex_literal(&computed_hex),
                                                    AccountAddress::from_hex_literal(object_id),
                                                ) {
                                                    if computed_id != actual_id {
                                                        let mut map =
                                                            child_id_aliases_for_resolver.lock();
                                                        map.insert(computed_id, actual_id);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if let Ok(mut cache) = resolver_cache.lock() {
                                    cache.insert(cache_key.clone(), tag.clone());
                                }
                                return Some(tag);
                            }
                        }
                        None
                    };
                    harness.set_key_type_resolver(Box::new(key_type_resolver));
                }

                Ok(harness)
            };

        let replay_once = |cached: &HashMap<String, String>,
                           versions: &HashMap<String, u64>|
         -> Result<sui_sandbox_core::tx_replay::ReplayResult> {
            let mut harness = make_harness(versions)?;
            sui_sandbox_core::tx_replay::replay_with_version_tracking_with_policy(
                &replay_state.transaction,
                &mut harness,
                cached,
                &pkg_aliases.aliases,
                Some(&versions_str),
                reconcile_policy,
            )
        };

        let mut replay_result = replay_once(&cached_objects, &version_map);
        let mut synthetic_logs: Vec<String> = Vec::new();

        if self.synthesize_missing
            && replay_result
                .as_ref()
                .map(|r| !r.local_success)
                .unwrap_or(true)
        {
            let missing =
                tx_replay::find_missing_input_objects(&replay_state.transaction, &cached_objects);
            if !missing.is_empty() {
                eprintln!(
                    "[replay_fallback] missing_input_objects={} (attempting synthesis)",
                    missing.len()
                );
                match synthesize_missing_inputs(
                    &missing,
                    &mut cached_objects,
                    &mut version_map,
                    &resolver,
                    &pkg_aliases.aliases,
                    &provider,
                    verbose,
                ) {
                    Ok(logs) => {
                        synthetic_logs = logs;
                        if !synthetic_logs.is_empty() {
                            eprintln!(
                                "[replay_fallback] synthesized_inputs={}",
                                synthetic_logs.len()
                            );
                            replay_result = replay_once(&cached_objects, &version_map);
                        }
                    }
                    Err(e) => {
                        if verbose {
                            eprintln!("[replay_fallback] synthesis_error={}", e);
                        }
                    }
                }
            }
        }

        match replay_result {
            Ok(result) => {
                let comparison = if self.compare {
                    result.comparison.map(|c| {
                        let mut notes = c.notes.clone();
                        if !synthetic_logs.is_empty() {
                            notes.push(format!("synthetic_inputs={}", synthetic_logs.len()));
                        }
                        ComparisonResult {
                            status_match: c.status_match,
                            created_match: c.created_count_match,
                            mutated_match: c.mutated_count_match,
                            deleted_match: c.deleted_count_match,
                            on_chain_status: if c.status_match && result.local_success {
                                "success".to_string()
                            } else if c.status_match && !result.local_success {
                                "failed".to_string()
                            } else {
                                "unknown".to_string()
                            },
                            local_status: if result.local_success {
                                "success".to_string()
                            } else {
                                "failed".to_string()
                            },
                            notes,
                        }
                    })
                } else {
                    None
                };

                if !synthetic_logs.is_empty() && verbose {
                    for line in &synthetic_logs {
                        eprintln!("[replay_fallback] {}", line);
                    }
                }

                Ok(ReplayOutput {
                    digest: self.digest.clone(),
                    local_success: result.local_success,
                    local_error: result.local_error,
                    comparison,
                    commands_executed: result.commands_executed,
                })
            }
            Err(e) => Ok(ReplayOutput {
                digest: self.digest.clone(),
                local_success: false,
                local_error: Some(e.to_string()),
                comparison: None,
                commands_executed: 0,
            }),
        }
    }
}

fn b64_matches_bytes(encoded: &str, expected: &[u8]) -> bool {
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) {
        return decoded == expected;
    }
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded) {
        return decoded == expected;
    }
    false
}

fn is_type_name_tag(tag: &TypeTag) -> bool {
    let TypeTag::Struct(s) = tag else {
        return false;
    };
    let Ok(std_addr) = AccountAddress::from_hex_literal("0x1") else {
        return false;
    };
    s.address == std_addr && s.module.as_str() == "type_name" && s.name.as_str() == "TypeName"
}

#[derive(Debug, Clone)]
struct MissEntry {
    count: u32,
    last: std::time::Instant,
}

struct ChildFetchOptions<'a> {
    provider: &'a HistoricalStateProvider,
    checkpoint: Option<u64>,
    max_version: u64,
    aliases: &'a HashMap<AccountAddress, AccountAddress>,
    child_id_aliases: &'a Arc<parking_lot::Mutex<HashMap<AccountAddress, AccountAddress>>>,
    miss_cache: Option<&'a Arc<parking_lot::Mutex<HashMap<String, MissEntry>>>>,
    debug_df: bool,
    debug_df_full: bool,
    self_heal_dynamic_fields: bool,
    synth_modules: Option<Arc<Vec<CompiledModule>>>,
    log_self_heal: bool,
}

fn fetch_child_object_by_key(
    options: &ChildFetchOptions<'_>,
    parent_id: AccountAddress,
    child_id: AccountAddress,
    key_type: &TypeTag,
    key_bytes: &[u8],
) -> Option<(TypeTag, Vec<u8>)> {
    let provider = options.provider;
    let checkpoint = options.checkpoint;
    let max_version = options.max_version;
    let aliases = options.aliases;
    let child_id_aliases = options.child_id_aliases;
    let miss_cache = options.miss_cache;
    let debug_df = options.debug_df;
    let debug_df_full = options.debug_df_full;
    let self_heal_dynamic_fields = options.self_heal_dynamic_fields;
    let synth_modules = options.synth_modules.as_ref();
    let log_self_heal = options.log_self_heal;

    let try_synthesize =
        |value_type: &str, object_id: Option<&str>, source: &str| -> Option<(TypeTag, Vec<u8>)> {
            if !self_heal_dynamic_fields {
                return None;
            }
            let modules = synth_modules?;
            let parsed = parse_type_tag(value_type).ok()?;
            let rewritten = rewrite_type_tag(parsed, aliases);
            let synth_type = format_type_tag(&rewritten);
            let type_model = match TypeModel::from_modules(modules.as_ref().clone()) {
                Ok(model) => model,
                Err(err) => {
                    if log_self_heal {
                        eprintln!("[df_self_heal] type model build failed: {}", err);
                    }
                    return None;
                }
            };
            let mut synthesizer = TypeSynthesizer::new(&type_model);
            let mut result = synthesizer.synthesize_with_fallback(&synth_type);
            let mut synth_id = child_id;
            if let Some(obj_id) = object_id.and_then(|s| AccountAddress::from_hex_literal(s).ok()) {
                if obj_id != child_id {
                    let mut map = child_id_aliases.lock();
                    map.insert(child_id, obj_id);
                }
                synth_id = obj_id;
                if result.bytes.len() >= 32 {
                    result.bytes[..32].copy_from_slice(synth_id.as_ref());
                }
            }
            if log_self_heal {
                eprintln!(
                    "[df_self_heal] synthesized source={} child={} type={} stub={} ({})",
                    source,
                    synth_id.to_hex_literal(),
                    synth_type,
                    result.is_stub,
                    result.description
                );
            }
            Some((rewritten, result.bytes))
        };

    if let Some(obj) = provider.cache().get_object_latest(&child_id) {
        if obj.version <= max_version {
            if let Some(type_str) = obj.type_tag {
                if let Ok(tag) = parse_type_tag(&type_str) {
                    if debug_df {
                        eprintln!(
                            "[df_fetch] cache hit child={} type={}",
                            child_id.to_hex_literal(),
                            type_str
                        );
                    }
                    return Some((tag, obj.bcs_bytes));
                }
            }
        }
    }

    let gql = provider.graphql();
    let child_hex = child_id.to_hex_literal();
    let record_alias = |child_id: AccountAddress, object_id: &str| {
        if let Ok(actual) = AccountAddress::from_hex_literal(object_id) {
            if actual != child_id {
                let mut map = child_id_aliases.lock();
                map.insert(child_id, actual);
            }
        }
    };

    if let Some(cp) = checkpoint {
        if let Ok(obj) = gql.fetch_object_at_checkpoint(&child_hex, cp) {
            if obj.version <= max_version {
                if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&bcs_b64) {
                        if let Ok(tag) = parse_type_tag(&type_str) {
                            if debug_df {
                                eprintln!(
                                    "[df_fetch] checkpoint object child={} type={}",
                                    child_hex, type_str
                                );
                            }
                            return Some((tag, bytes));
                        }
                    }
                }
            }
        }
    }

    let parent_hex = parent_id.to_hex_literal();
    let miss_key = miss_cache.map(|_| {
        let key_type_str = format_type_tag(key_type);
        let key_b64 = base64::engine::general_purpose::STANDARD.encode(key_bytes);
        format!("{}:{}:{}:{}", parent_hex, child_hex, key_type_str, key_b64)
    });
    if let (Some(cache), Some(key)) = (miss_cache, miss_key.as_ref()) {
        if let Some(entry) = cache.lock().get(key).cloned() {
            let backoff_ms = std::env::var("SUI_DF_MISS_BACKOFF_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(250);
            let exp = entry.count.saturating_sub(1).min(3);
            let delay = backoff_ms.saturating_mul(1u64 << exp);
            if entry.last.elapsed().as_millis() < delay as u128 {
                if debug_df {
                    eprintln!(
                        "[df_fetch] cached miss/backoff parent={} child={} key_len={} delay_ms={}",
                        parent_hex,
                        child_hex,
                        key_bytes.len(),
                        delay
                    );
                }
                return None;
            }
        }
    }

    let mut reverse_aliases: HashMap<AccountAddress, AccountAddress> = HashMap::new();
    let mut reverse_aliases_all: HashMap<AccountAddress, Vec<AccountAddress>> = HashMap::new();
    if !aliases.is_empty() {
        for (storage, runtime) in aliases {
            reverse_aliases.insert(*runtime, *storage);
            reverse_aliases_all
                .entry(*runtime)
                .or_default()
                .push(*storage);
        }
    }
    let mut name_types = Vec::with_capacity(2);
    name_types.push(format_type_tag(key_type));
    if !aliases.is_empty() {
        let rewritten = rewrite_type_tag(key_type.clone(), aliases);
        let alt = format_type_tag(&rewritten);
        if alt != name_types[0] {
            name_types.push(alt);
        }
        let reverse = rewrite_type_tag(key_type.clone(), &reverse_aliases);
        let reverse_str = format_type_tag(&reverse);
        if !name_types.contains(&reverse_str) {
            name_types.push(reverse_str);
        }
        if let TypeTag::Struct(s) = key_type {
            if let Some(storages) = reverse_aliases_all.get(&s.address) {
                for storage in storages {
                    if *storage == s.address {
                        continue;
                    }
                    let mut reverse_map = HashMap::new();
                    reverse_map.insert(s.address, *storage);
                    let alt_tag = rewrite_type_tag(key_type.clone(), &reverse_map);
                    let alt_str = format_type_tag(&alt_tag);
                    if !name_types.contains(&alt_str) {
                        name_types.push(alt_str);
                    }
                }
            }
        }
    }
    let has_vector_u8 = name_types.iter().any(|t| t == "vector<u8>");
    let has_string = name_types.iter().any(|t| {
        t == "0x1::string::String"
            || t == "0x0000000000000000000000000000000000000000000000000000000000000001::string::String"
    });
    if has_vector_u8 && !has_string {
        name_types.push("0x1::string::String".to_string());
        name_types.push(
            "0x0000000000000000000000000000000000000000000000000000000000000001::string::String"
                .to_string(),
        );
    } else if has_string && !has_vector_u8 {
        name_types.push("vector<u8>".to_string());
    }

    let mut key_variants: Vec<Vec<u8>> = Vec::new();
    let mut key_variants_seen: HashSet<Vec<u8>> = HashSet::new();
    let mut push_key_variant = |bytes: Vec<u8>| {
        if key_variants_seen.insert(bytes.clone()) {
            key_variants.push(bytes);
        }
    };
    push_key_variant(key_bytes.to_vec());

    let mut type_name_variants: Vec<String> = Vec::new();
    let mut type_name_seen: HashSet<String> = HashSet::new();
    if is_type_name_tag(key_type) {
        if let Ok(raw_bytes) = bcs::from_bytes::<Vec<u8>>(key_bytes) {
            if let Ok(name_str) = String::from_utf8(raw_bytes) {
                if type_name_seen.insert(name_str.clone()) {
                    type_name_variants.push(name_str.clone());
                }
                if let Ok(parsed) = parse_type_tag(&name_str) {
                    let mut tag_variants = Vec::new();
                    tag_variants.push(parsed.clone());
                    let rewritten = rewrite_type_tag(parsed.clone(), aliases);
                    if rewritten != parsed {
                        tag_variants.push(rewritten);
                    }
                    if !reverse_aliases.is_empty() {
                        let reversed = rewrite_type_tag(parsed.clone(), &reverse_aliases);
                        if reversed != parsed {
                            tag_variants.push(reversed.clone());
                        }
                        if let TypeTag::Struct(s) = &parsed {
                            if let Some(storages) = reverse_aliases_all.get(&s.address) {
                                for storage in storages {
                                    if *storage == s.address {
                                        continue;
                                    }
                                    let mut reverse_map = HashMap::new();
                                    reverse_map.insert(s.address, *storage);
                                    let alt = rewrite_type_tag(parsed.clone(), &reverse_map);
                                    tag_variants.push(alt);
                                }
                            }
                        }
                    }
                    for tag in tag_variants {
                        let rendered = format_type_tag(&tag);
                        if type_name_seen.insert(rendered.clone()) {
                            type_name_variants.push(rendered);
                        }
                    }
                }
                for rendered in &type_name_variants {
                    if let Ok(bcs_bytes) = bcs::to_bytes(&rendered.as_bytes().to_vec()) {
                        push_key_variant(bcs_bytes);
                    }
                }
            }
        }
    }

    // If we can derive an alternate child ID from known name types, prefer cached hits.
    {
        let mut seen = std::collections::HashSet::new();
        for name_type in &name_types {
            let Ok(tag) = parse_type_tag(name_type) else {
                continue;
            };
            let Ok(type_bcs) = bcs::to_bytes(&tag) else {
                continue;
            };
            for key_variant in &key_variants {
                let Some(computed_hex) =
                    compute_dynamic_field_id(&parent_hex, key_variant, &type_bcs)
                else {
                    continue;
                };
                let Ok(computed_id) = AccountAddress::from_hex_literal(&computed_hex) else {
                    continue;
                };
                if !seen.insert(computed_id) {
                    continue;
                }
                if let Some(obj) = provider.cache().get_object_latest(&computed_id) {
                    if obj.version <= max_version {
                        if let Some(type_str) = obj.type_tag {
                            if let Ok(tag) = parse_type_tag(&type_str) {
                                if computed_id != child_id {
                                    let mut map = child_id_aliases.lock();
                                    map.insert(child_id, computed_id);
                                }
                                if debug_df {
                                    eprintln!(
                                        "[df_fetch] cache alias hit child={} alias={} type={}",
                                        child_hex,
                                        computed_id.to_hex_literal(),
                                        type_str
                                    );
                                }
                                return Some((tag, obj.bcs_bytes));
                            }
                        }
                    }
                }
                if self_heal_dynamic_fields {
                    if let Some((tag, bytes, _)) =
                        fetch_child_object_shared(provider, computed_id, checkpoint, max_version)
                    {
                        if computed_id != child_id {
                            let mut map = child_id_aliases.lock();
                            map.insert(child_id, computed_id);
                        }
                        if debug_df {
                            eprintln!(
                                "[df_fetch] fetched alias child={} alias={} type={}",
                                child_hex,
                                computed_id.to_hex_literal(),
                                format_type_tag(&tag)
                            );
                        }
                        return Some((tag, bytes));
                    }
                }
            }
        }
    }

    if debug_df && !type_name_variants.is_empty() {
        let preview = if debug_df_full {
            type_name_variants.join(" | ")
        } else {
            type_name_variants
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        };
        eprintln!(
            "[df_fetch] type_name variants parent={} child={} count={} [{}]",
            parent_hex,
            child_hex,
            type_name_variants.len(),
            preview
        );
    }

    for (variant_idx, key_variant) in key_variants.iter().enumerate() {
        for name_type in &name_types {
            let df = if let Some(cp) = checkpoint {
                match gql.fetch_dynamic_field_by_name_at_checkpoint(
                    &parent_hex,
                    name_type,
                    key_variant,
                    cp,
                ) {
                    Ok(Some(df)) => Ok(Some(df)),
                    Ok(None) => {
                        gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_variant)
                    }
                    Err(_) => gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_variant),
                }
            } else {
                gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_variant)
            };
            if let Ok(Some(df)) = df {
                if let Some(version) = df.version {
                    if version > max_version {
                        continue;
                    }
                }
                if let Some(object_id) = df.object_id.as_deref() {
                    record_alias(child_id, object_id);
                    if let Some(version) = df.version {
                        if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        if debug_df {
                                            eprintln!(
                                                "[df_fetch] by_name object versioned child={} version={}",
                                                object_id, version
                                            );
                                        }
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                        if let Some((tag, bytes, _)) =
                            fetch_object_via_grpc_shared(provider, object_id, Some(version))
                        {
                            return Some((tag, bytes));
                        }
                    }
                    if let Some(cp) = checkpoint {
                        if let Ok(obj) = gql.fetch_object_at_checkpoint(object_id, cp) {
                            if obj.version <= max_version {
                                if let (Some(type_str), Some(bcs_b64)) =
                                    (obj.type_string, obj.bcs_base64)
                                {
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                    {
                                        if let Ok(tag) = parse_type_tag(&type_str) {
                                            if debug_df {
                                                eprintln!(
                                                    "[df_fetch] by_name object checkpoint child={} type={}",
                                                    object_id, type_str
                                                );
                                            }
                                            return Some((tag, bytes));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Ok(obj) = gql.fetch_object(object_id) {
                        if obj.version <= max_version {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        if debug_df {
                                            eprintln!(
                                                "[df_fetch] by_name object child={} type={}",
                                                object_id, type_str
                                            );
                                        }
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                    }
                    if let Some((tag, bytes, version)) =
                        fetch_object_via_grpc_shared(provider, object_id, None)
                    {
                        if version <= max_version {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let (Some(value_type), Some(value_bcs)) = (&df.value_type, &df.value_bcs) {
                    if let Ok(bytes) =
                        base64::engine::general_purpose::STANDARD.decode(value_bcs.as_bytes())
                    {
                        if let Ok(tag) = parse_type_tag(value_type.as_str()) {
                            if debug_df {
                                if key_variants.len() > 1 {
                                    eprintln!(
                                        "[df_fetch] by_name hit parent={} name_type={} child={} value_type={} key_variant={}",
                                        parent_hex, name_type, child_hex, value_type, variant_idx
                                    );
                                } else {
                                    eprintln!(
                                        "[df_fetch] by_name hit parent={} name_type={} child={} value_type={}",
                                        parent_hex, name_type, child_hex, value_type
                                    );
                                }
                            }
                            return Some((tag, bytes));
                        }
                    }
                }
                if let Some(value_type) = df.value_type.as_deref() {
                    if let Some(synth) =
                        try_synthesize(value_type, df.object_id.as_deref(), "by_name")
                    {
                        return Some(synth);
                    }
                }
            } else if debug_df {
                eprintln!(
                    "[df_fetch] by_name miss parent={} name_type={} child={}",
                    parent_hex, name_type, child_hex
                );
            }
        }
    }

    let enum_limit = std::env::var("SUI_DF_ENUM_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1000);
    let key_b64s: Vec<String> = key_variants
        .iter()
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .collect();
    for name_type in &name_types {
        let fields = match checkpoint {
            Some(cp) => gql
                .fetch_dynamic_fields_at_checkpoint(&parent_hex, enum_limit, cp)
                .or_else(|_| gql.fetch_dynamic_fields(&parent_hex, enum_limit)),
            None => gql.fetch_dynamic_fields(&parent_hex, enum_limit),
        };
        let Ok(fields) = fields else {
            if debug_df {
                eprintln!(
                    "[df_fetch] enumerate failed parent={} name_type={}",
                    parent_hex, name_type
                );
            }
            continue;
        };
        let fields = if fields.is_empty() && checkpoint.is_some() {
            match gql.fetch_dynamic_fields(&parent_hex, enum_limit) {
                Ok(latest) if !latest.is_empty() => {
                    if debug_df {
                        eprintln!(
                            "[df_fetch] enumerate fallback latest parent={} name_type={} fields={}",
                            parent_hex,
                            name_type,
                            latest.len()
                        );
                    }
                    latest
                }
                _ => fields,
            }
        } else {
            fields
        };
        if debug_df {
            eprintln!(
                "[df_fetch] enumerate parent={} name_type={} fields={}",
                parent_hex,
                name_type,
                fields.len()
            );
            let key_preview = if debug_df_full {
                key_b64s.join("|")
            } else {
                key_b64s
                    .first()
                    .and_then(|b| b.get(0..16))
                    .unwrap_or("<none>")
                    .to_string()
            };
            eprintln!(
                "[df_fetch] key_b64 parent={} name_type={} key_b64={}",
                parent_hex, name_type, key_preview
            );
            for (idx, df) in fields.iter().take(5).enumerate() {
                let bcs_preview = df
                    .name_bcs
                    .as_deref()
                    .and_then(|s| s.get(0..16))
                    .unwrap_or("<none>");
                eprintln!(
                    "[df_fetch] enumerate field parent={} idx={} name_type={} name_bcs_prefix={}",
                    parent_hex, idx, df.name_type, bcs_preview
                );
                if debug_df_full {
                    let full = df.name_bcs.as_deref().unwrap_or("<none>");
                    eprintln!(
                        "[df_fetch] enumerate field full parent={} idx={} name_bcs_full={}",
                        parent_hex, idx, full
                    );
                }
            }
        }
        let mut fallback: Option<sui_transport::graphql::DynamicFieldInfo> = None;
        let mut fallback_count = 0usize;
        let mut fallback_missing_bcs: Option<sui_transport::graphql::DynamicFieldInfo> = None;
        let mut fallback_missing_bcs_count = 0usize;
        for df in &fields {
            let name_bcs = match df.name_bcs.as_deref() {
                Some(bcs) => bcs,
                None => {
                    if self_heal_dynamic_fields {
                        fallback_missing_bcs_count += 1;
                        if fallback_missing_bcs.is_none() {
                            fallback_missing_bcs = Some(df.clone());
                        }
                    }
                    continue;
                }
            };
            let mut matched = false;
            for (idx, key_b64) in key_b64s.iter().enumerate() {
                if name_bcs == key_b64.as_str() || b64_matches_bytes(name_bcs, &key_variants[idx]) {
                    matched = true;
                    break;
                }
            }
            if !matched {
                continue;
            }
            if df.name_type != *name_type {
                fallback_count += 1;
                if fallback.is_none() {
                    fallback = Some(df.clone());
                }
                continue;
            }
            if let Some(version) = df.version {
                if version > max_version {
                    continue;
                }
            }
            if let Some(object_id) = &df.object_id {
                record_alias(child_id, object_id);
                if let Some(version) = df.version {
                    if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                        if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                            if let Ok(bytes) =
                                base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                            {
                                if let Ok(tag) = parse_type_tag(&type_str) {
                                    if debug_df {
                                        eprintln!(
                                            "[df_fetch] enum object versioned child={} version={}",
                                            object_id, version
                                        );
                                    }
                                    return Some((tag, bytes));
                                }
                            }
                        }
                    }
                }
                if let Some(cp) = checkpoint {
                    if let Ok(obj) = gql.fetch_object_at_checkpoint(object_id, cp) {
                        if obj.version <= max_version {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        if debug_df {
                                            eprintln!(
                                                "[df_fetch] enum object checkpoint child={} type={}",
                                                object_id, type_str
                                            );
                                        }
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Ok(obj) = gql.fetch_object(object_id) {
                    if obj.version <= max_version {
                        if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                            if let Ok(bytes) =
                                base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                            {
                                if let Ok(tag) = parse_type_tag(&type_str) {
                                    if debug_df {
                                        eprintln!(
                                            "[df_fetch] enum object child={} type={}",
                                            object_id, type_str
                                        );
                                    }
                                    return Some((tag, bytes));
                                }
                            }
                        }
                    }
                }
                if let Some((tag, bytes, version)) =
                    fetch_object_via_grpc_shared(provider, object_id, None)
                {
                    if version <= max_version {
                        return Some((tag, bytes));
                    }
                }
            }
            if let (Some(value_type), Some(value_bcs)) = (&df.value_type, &df.value_bcs) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(value_bcs) {
                    if let Ok(tag) = parse_type_tag(value_type) {
                        if debug_df {
                            eprintln!(
                                "[df_fetch] enum hit parent={} name_type={} child={} value_type={}",
                                parent_hex, name_type, child_hex, value_type
                            );
                        }
                        return Some((tag, bytes));
                    }
                }
            }
            if let Some(value_type) = df.value_type.as_deref() {
                if let Some(synth) =
                    try_synthesize(value_type, df.object_id.as_deref(), "enumerate")
                {
                    return Some(synth);
                }
            }
        }
        if self_heal_dynamic_fields && fallback_count == 1 {
            if let Some(df) = fallback {
                if debug_df {
                    eprintln!(
                        "[df_fetch] enum fallback parent={} requested={} found={} child={}",
                        parent_hex, name_type, df.name_type, child_hex
                    );
                }
                if let Some(version) = df.version {
                    if version > max_version {
                        continue;
                    }
                }
                if let Some(object_id) = df.object_id.as_deref() {
                    record_alias(child_id, object_id);
                    if let Some(version) = df.version {
                        if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                        if let Some((tag, bytes, _)) =
                            fetch_object_via_grpc_shared(provider, object_id, Some(version))
                        {
                            return Some((tag, bytes));
                        }
                    }
                    if let Some(cp) = checkpoint {
                        if let Ok(obj) = gql.fetch_object_at_checkpoint(object_id, cp) {
                            if obj.version <= max_version {
                                if let (Some(type_str), Some(bcs_b64)) =
                                    (obj.type_string, obj.bcs_base64)
                                {
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                    {
                                        if let Ok(tag) = parse_type_tag(&type_str) {
                                            return Some((tag, bytes));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Ok(obj) = gql.fetch_object(object_id) {
                        if obj.version <= max_version {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                    }
                    if let Some((tag, bytes, version)) =
                        fetch_object_via_grpc_shared(provider, object_id, None)
                    {
                        if version <= max_version {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let (Some(value_type), Some(value_bcs)) = (&df.value_type, &df.value_bcs) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(value_bcs) {
                        if let Ok(tag) = parse_type_tag(value_type) {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let Some(value_type) = df.value_type.as_deref() {
                    if let Some(synth) =
                        try_synthesize(value_type, df.object_id.as_deref(), "fallback")
                    {
                        return Some(synth);
                    }
                }
            }
        }
        if self_heal_dynamic_fields && fallback_count == 0 && fallback_missing_bcs_count == 1 {
            if let Some(df) = fallback_missing_bcs {
                if debug_df {
                    eprintln!(
                        "[df_fetch] enum fallback missing name_bcs parent={} name_type={} child={}",
                        parent_hex, name_type, child_hex
                    );
                }
                if let Some(version) = df.version {
                    if version > max_version {
                        continue;
                    }
                }
                if let Some(object_id) = df.object_id.as_deref() {
                    record_alias(child_id, object_id);
                    if let Some(version) = df.version {
                        if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                        if let Some((tag, bytes, _)) =
                            fetch_object_via_grpc_shared(provider, object_id, Some(version))
                        {
                            return Some((tag, bytes));
                        }
                    }
                    if let Some(cp) = checkpoint {
                        if let Ok(obj) = gql.fetch_object_at_checkpoint(object_id, cp) {
                            if obj.version <= max_version {
                                if let (Some(type_str), Some(bcs_b64)) =
                                    (obj.type_string, obj.bcs_base64)
                                {
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                    {
                                        if let Ok(tag) = parse_type_tag(&type_str) {
                                            return Some((tag, bytes));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Ok(obj) = gql.fetch_object(object_id) {
                        if obj.version <= max_version {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                    }
                    if let Some((tag, bytes, version)) =
                        fetch_object_via_grpc_shared(provider, object_id, None)
                    {
                        if version <= max_version {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let (Some(value_type), Some(value_bcs)) = (&df.value_type, &df.value_bcs) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(value_bcs) {
                        if let Ok(tag) = parse_type_tag(value_type) {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let Some(value_type) = df.value_type.as_deref() {
                    if let Some(synth) =
                        try_synthesize(value_type, df.object_id.as_deref(), "fallback_missing_bcs")
                    {
                        return Some(synth);
                    }
                }
            }
        }
    }

    if let Ok(obj) = gql.fetch_object(&child_hex) {
        if obj.version <= max_version {
            if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&bcs_b64) {
                    if let Ok(tag) = parse_type_tag(&type_str) {
                        if debug_df {
                            eprintln!(
                                "[df_fetch] fallback object child={} type={}",
                                child_hex, type_str
                            );
                        }
                        return Some((tag, bytes));
                    }
                }
            }
        }
    }

    if let Some((tag, bytes, version)) = fetch_object_via_grpc_shared(provider, &child_hex, None) {
        if version <= max_version {
            if debug_df {
                eprintln!(
                    "[df_fetch] fallback grpc child={} version={}",
                    child_hex, version
                );
            }
            return Some((tag, bytes));
        }
    }

    if debug_df {
        eprintln!(
            "[df_fetch] miss parent={} child={} key_len={}",
            parent_hex,
            child_hex,
            key_bytes.len()
        );
    }
    if let (Some(cache), Some(key)) = (miss_cache, miss_key) {
        let mut map = cache.lock();
        let entry = map.entry(key).or_insert_with(|| MissEntry {
            count: 0,
            last: std::time::Instant::now(),
        });
        entry.count = entry.count.saturating_add(1);
        entry.last = std::time::Instant::now();
    }
    None
}

fn print_replay_result(result: &ReplayOutput, show_comparison: bool) {
    println!("\x1b[1mTransaction Replay: {}\x1b[0m\n", result.digest);

    if result.local_success {
        println!("\x1b[32m✓ Local execution succeeded\x1b[0m");
    } else {
        println!("\x1b[31m✗ Local execution failed\x1b[0m");
        if let Some(err) = &result.local_error {
            println!("  Error: {}", err);
        }
    }

    println!("  Commands executed: {}", result.commands_executed);

    if show_comparison {
        if let Some(cmp) = &result.comparison {
            println!("\n\x1b[1mComparison with on-chain:\x1b[0m");
            println!(
                "  Status: {} (local: {}, on-chain: {})",
                if cmp.status_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[31m✗ mismatch\x1b[0m"
                },
                cmp.local_status,
                cmp.on_chain_status
            );
            println!(
                "  Created objects: {}",
                if cmp.created_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
            println!(
                "  Mutated objects: {}",
                if cmp.mutated_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
            println!(
                "  Deleted objects: {}",
                if cmp.deleted_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
        } else {
            println!("\n\x1b[33mNote: No on-chain effects available for comparison\x1b[0m");
        }
    }
}

fn fetch_dependency_closure(
    resolver: &mut LocalModuleResolver,
    graphql: &GraphQLClient,
    checkpoint: Option<u64>,
    verbose: bool,
) -> Result<usize> {
    use std::collections::BTreeSet;

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
                let mut modules = Vec::new();
                for module in pkg.modules {
                    if let Some(bytes_b64) = module.bytecode_base64 {
                        if let Ok(bytes) =
                            base64::engine::general_purpose::STANDARD.decode(bytes_b64)
                        {
                            modules.push((module.name, bytes));
                        }
                    }
                }
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

fn synthesize_missing_inputs(
    missing: &[MissingInputObject],
    cached_objects: &mut HashMap<String, String>,
    version_map: &mut HashMap<String, u64>,
    resolver: &LocalModuleResolver,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    provider: &HistoricalStateProvider,
    verbose: bool,
) -> Result<Vec<String>> {
    if missing.is_empty() {
        return Ok(Vec::new());
    }

    let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
    if modules.is_empty() {
        return Err(anyhow!("no modules loaded for synthesis"));
    }
    let type_model = TypeModel::from_modules(modules)
        .map_err(|e| anyhow!("failed to build type model: {}", e))?;
    let mut synthesizer = TypeSynthesizer::new(&type_model);

    let gql = provider.graphql();
    let mut logs = Vec::new();

    for entry in missing {
        let object_id = entry.object_id.as_str();
        let version = entry.version;
        let mut type_string = gql
            .fetch_object_at_version(object_id, version)
            .ok()
            .and_then(|obj| obj.type_string)
            .or_else(|| {
                gql.fetch_object(object_id)
                    .ok()
                    .and_then(|obj| obj.type_string)
            });

        let Some(type_str) = type_string.take() else {
            if verbose {
                logs.push(format!(
                    "missing_type object={} version={} (skipped)",
                    object_id, version
                ));
            }
            continue;
        };

        let mut synth_type = type_str.clone();
        if let Ok(tag) = parse_type_tag(&type_str) {
            let rewritten = rewrite_type_tag(tag, aliases);
            synth_type = format_type_tag(&rewritten);
        }

        let mut result = synthesizer.synthesize_with_fallback(&synth_type);
        if let Ok(id) = AccountAddress::from_hex_literal(object_id) {
            if result.bytes.len() >= 32 {
                result.bytes[..32].copy_from_slice(id.as_ref());
            }
        }

        let encoded = base64::engine::general_purpose::STANDARD.encode(&result.bytes);
        let normalized = sui_sandbox_core::utilities::normalize_address(object_id);
        cached_objects.insert(normalized.clone(), encoded.clone());
        cached_objects.insert(object_id.to_string(), encoded.clone());
        if let Some(short) = sui_sandbox_core::types::normalize_address_short(object_id) {
            cached_objects.insert(short, encoded.clone());
        }
        version_map.insert(normalized.clone(), version);

        logs.push(format!(
            "synthesized object={} version={} type={} stub={} ({})",
            normalized, version, synth_type, result.is_stub, result.description
        ));
    }

    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_output_serialization() {
        let output = ReplayOutput {
            digest: "test123".to_string(),
            local_success: true,
            local_error: None,
            comparison: Some(ComparisonResult {
                status_match: true,
                created_match: true,
                mutated_match: true,
                deleted_match: true,
                on_chain_status: "success".to_string(),
                local_status: "success".to_string(),
                notes: Vec::new(),
            }),
            commands_executed: 3,
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"local_success\":true"));
        assert!(json.contains("\"status_match\":true"));
    }
}
