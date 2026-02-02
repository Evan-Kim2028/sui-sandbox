//! Replay command - replay historical transactions locally

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use clap::{Parser, ValueEnum};
use move_binary_format::CompiledModule;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

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
use sui_sandbox_core::types::{format_type_tag, is_system_package_address, parse_type_tag};
use sui_sandbox_core::utilities::historical_state::HistoricalStateReconstructor;
use sui_sandbox_core::utilities::rewrite_type_tag;
use sui_sandbox_core::vm::SimulationConfig;
use sui_state_fetcher::{
    build_aliases as build_aliases_shared, fetch_child_object as fetch_child_object_shared,
    fetch_object_via_grpc as fetch_object_via_grpc_shared, HistoricalStateProvider, PackageData,
    ReplayState, VersionedCache, VersionedObject,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;
use sui_transport::grpc::GrpcOwner;
use sui_types::digests::TransactionDigest;
use sui_types::move_package::MovePackage;
use sui_types::object::{Data as SuiData, Object as SuiObject};
use sui_types::transaction::{
    Argument as SuiArgument, CallArg, Command as SuiCommand, ObjectArg, SharedObjectMutability,
    TransactionData, TransactionDataAPI, TransactionKind,
};
use sui_types::type_input::TypeInput;
use sui_sandbox_types::{
    normalize_address as normalize_address_shared, FetchedTransaction, GasSummary,
    PtbArgument, PtbCommand, TransactionDigest as SandboxTransactionDigest,
    TransactionEffectsSummary, TransactionInput, TransactionStatus, CLOCK_OBJECT_ID,
    RANDOM_OBJECT_ID,
};

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

    /// Use hybrid loader (igloo-mcp Snowflake for tx/effects/packages, gRPC for objects)
    #[arg(long)]
    pub hybrid: bool,

    /// Path to mcp_service_config.json for igloo-mcp (defaults to nearest ancestor)
    #[arg(long)]
    pub igloo_config: Option<PathBuf>,

    /// Override igloo-mcp command path (defaults to config or igloo_mcp in PATH)
    #[arg(long)]
    pub igloo_command: Option<String>,

    /// Groot database name (default: PIPELINE_V2_GROOT_DB)
    #[arg(long, default_value = "PIPELINE_V2_GROOT_DB")]
    pub groot_db: String,

    /// Groot schema name (default: PIPELINE_V2_GROOT_SCHEMA)
    #[arg(long, default_value = "PIPELINE_V2_GROOT_SCHEMA")]
    pub groot_schema: String,

    /// Analytics database name (default: ANALYTICS_DB_V2)
    #[arg(long, default_value = "ANALYTICS_DB_V2")]
    pub analytics_db: String,

    /// Analytics schema name (default: CHAINDATA_MAINNET)
    #[arg(long, default_value = "CHAINDATA_MAINNET")]
    pub analytics_schema: String,

    /// Fetch packages from Snowflake when available (fallback to gRPC if missing)
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub snowflake_packages: bool,

    /// Require packages to be loaded from Snowflake (no gRPC fallback)
    #[arg(long, default_value_t = false)]
    pub require_snowflake_packages: bool,

    /// Timeout in seconds for gRPC object fetches (default: 30)
    #[arg(long, default_value_t = 30)]
    pub grpc_timeout_secs: u64,
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
        let dotenv = load_dotenv_vars();
        let replay_progress = std::env::var("SUI_REPLAY_PROGRESS")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        if verbose {
            eprintln!("Fetching transaction {}...", self.digest);
        }

        // Fetch full replay state using gRPC + GraphQL
        let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
        let network = infer_network(&state.rpc_url, &graphql_endpoint);
        let cache = Arc::new(VersionedCache::with_storage(cache_dir(&network))?);
        let api_key = std::env::var("SUI_GRPC_API_KEY")
            .ok()
            .or_else(|| dotenv.get("SUI_GRPC_API_KEY").cloned());
        let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT")
            .or_else(|_| std::env::var("SUI_GRPC_ARCHIVE_ENDPOINT"))
            .or_else(|_| std::env::var("SUI_GRPC_HISTORICAL_ENDPOINT"))
            .or_else(|_| {
                dotenv
                    .get("SUI_GRPC_ENDPOINT")
                    .cloned()
                    .ok_or_else(|| std::env::VarError::NotPresent)
            })
            .or_else(|_| {
                dotenv
                    .get("SUI_GRPC_ARCHIVE_ENDPOINT")
                    .cloned()
                    .ok_or_else(|| std::env::VarError::NotPresent)
            })
            .or_else(|_| {
                dotenv
                    .get("SUI_GRPC_HISTORICAL_ENDPOINT")
                    .cloned()
                    .ok_or_else(|| std::env::VarError::NotPresent)
            })
            .unwrap_or_else(|_| state.rpc_url.clone());
        if verbose && grpc_endpoint != state.rpc_url {
            eprintln!("[grpc] using endpoint override {}", grpc_endpoint);
        }
        let grpc_client = GrpcClient::with_api_key(&grpc_endpoint, api_key).await?;
        let graphql_client = sui_transport::graphql::GraphQLClient::new(&graphql_endpoint);
        let provider = Arc::new(
            HistoricalStateProvider::with_clients(grpc_client, graphql_client)
                .with_walrus_from_env()
                .with_local_object_store_from_env()
                .with_cache(cache),
        );

        let enable_dynamic_fields = self.hybrid || self.fetch_strategy == FetchStrategy::Full;
        let strict_df_checkpoint =
            env_bool_opt("SUI_DF_STRICT_CHECKPOINT").unwrap_or(self.hybrid);
        if strict_df_checkpoint {
            std::env::set_var("SUI_DF_STRICT_CHECKPOINT", "1");
        }
        let replay_state = if self.hybrid {
            self.build_replay_state_hybrid(provider.as_ref(), verbose)
                .await
                .context("Failed to fetch replay state (hybrid)")?
        } else {
            provider
                .replay_state_builder()
                .prefetch_dynamic_fields(enable_dynamic_fields)
                .dynamic_field_depth(self.prefetch_depth)
                .dynamic_field_limit(self.prefetch_limit)
                .auto_system_objects(self.auto_system_objects)
                .build(&self.digest)
                .await
                .context("Failed to fetch replay state")?
        };
        if replay_progress {
            eprintln!("[replay] state built");
        }

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
        if replay_progress {
            eprintln!("[replay] aliases built");
        }

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
        if replay_progress {
            eprintln!("[replay] resolver hydrated");
        }

        let allow_graphql_deps = std::env::var("SUI_ALLOW_GRAPHQL_DEPS")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let fetched_deps = if self.hybrid && !allow_graphql_deps {
            if verbose {
                eprintln!("[deps] skipping GraphQL dependency fetch in hybrid mode");
            }
            0
        } else {
            if verbose {
                eprintln!("[deps] resolving dependency closure (GraphQL)");
            }
            let deps = fetch_dependency_closure(
                &mut resolver,
                provider.graphql(),
                replay_state.checkpoint,
                verbose,
            )
            .unwrap_or(0);
            if verbose {
                eprintln!("[deps] dependency closure complete (fetched {})", deps);
            }
            deps
        };
        if replay_progress {
            eprintln!("[replay] dependency closure done");
        }
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
        if replay_progress {
            eprintln!("[replay] executing locally");
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
            if replay_progress {
                eprintln!("[replay] version patcher start");
            }
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
            if replay_progress {
                eprintln!("[replay] version patcher done");
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
                if enable_dynamic_fields {
                    let provider_clone = Arc::clone(&provider);
                    let provider_clone_for_key = Arc::clone(&provider);
                    let checkpoint = replay_state.checkpoint;
                    let strict_checkpoint = strict_df_checkpoint && checkpoint.is_some();
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
                                strict_checkpoint,
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
                    let resolver_strict_checkpoint = strict_checkpoint;
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
                                .or_else(|err| {
                                    if resolver_strict_checkpoint {
                                        Err(err)
                                    } else {
                                        gql.find_dynamic_field_by_bcs(
                                            &parent_hex,
                                            key_bytes,
                                            None,
                                            enum_limit,
                                        )
                                    }
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
        if replay_progress {
            eprintln!("[replay] first execution attempt done");
        }
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

    fn resolve_igloo_config(&self) -> Result<IglooConfig> {
        let explicit_path = self.igloo_config.clone();
        if let Some(path) = explicit_path.as_ref() {
            if !path.exists() {
                return Err(anyhow!(
                    "Igloo config not found: {}",
                    path.display()
                ));
            }
        }
        let config_path = explicit_path
            .or_else(|| std::env::var("IGLOO_MCP_SERVICE_CONFIG").ok().map(PathBuf::from))
            .or_else(|| std::env::var("IGLOO_MCP_CONFIG").ok().map(PathBuf::from))
            .or_else(|| find_mcp_service_config(&std::env::current_dir().unwrap_or_default()));

        let mut config = if let Some(path) = config_path {
            if path.exists() {
                load_igloo_config(&path)?
            } else {
                IglooConfig::default()
            }
        } else {
            IglooConfig::default()
        };

        if let Some(cmd) = &self.igloo_command {
            config.command = cmd.clone();
        }

        config.command = resolve_igloo_command(&config.command)
            .unwrap_or_else(|| config.command.clone());
        apply_snowflake_config(&mut config);

        Ok(config)
    }

    async fn build_replay_state_hybrid(
        &self,
        provider: &HistoricalStateProvider,
        verbose: bool,
    ) -> Result<ReplayState> {
        let mut igloo = IglooMcpClient::connect(self.resolve_igloo_config()?).await?;
        let result = async {
            let digest_sql = escape_sql_literal(&self.digest);

            let meta_query = format!(
                "select CHECKPOINT, EPOCH, TIMESTAMP_MS, EFFECTS_JSON from {}.{}.TRANSACTION where TRANSACTION_DIGEST = '{}' limit 1",
                self.analytics_db, self.analytics_schema, digest_sql
            );
            let meta_row = igloo
                .query_one(&meta_query, "hybrid replay: transaction metadata")
                .await
                .context("TRANSACTION metadata query failed")?;
            let checkpoint = row_get_u64(&meta_row, "CHECKPOINT")
                .ok_or_else(|| anyhow!("Missing CHECKPOINT in TRANSACTION for {}", self.digest))?;
            let mut epoch = row_get_u64(&meta_row, "EPOCH").unwrap_or(0);
            let timestamp_ms = row_get_u64(&meta_row, "TIMESTAMP_MS")
                .ok_or_else(|| anyhow!("Missing TIMESTAMP_MS in TRANSACTION for {}", self.digest))?;
            let timestamp_ms_opt = Some(timestamp_ms);
            let effects_raw = row_get_value(&meta_row, "EFFECTS_JSON")
                .ok_or_else(|| anyhow!("Missing EFFECTS_JSON for {}", self.digest))?;
            let effects_json = parse_effects_value(effects_raw)?;
            let shared_versions = parse_effects_versions(&effects_json);
            let effects_summary = build_effects_summary(&effects_json, &shared_versions);
            if epoch == 0 {
                if let Some(executed) = extract_executed_epoch(&effects_json) {
                    epoch = executed;
                }
            }

            let tx_query = format!(
                "select BCS from {}.{}.TRANSACTION_BCS where TRANSACTION_DIGEST = '{}' and TIMESTAMP_MS = {} and CHECKPOINT = {} limit 1",
                self.analytics_db, self.analytics_schema, digest_sql, timestamp_ms, checkpoint
            );
            let tx_row = igloo
                .query_one(&tx_query, "hybrid replay: transaction_bcs")
                .await
                .context("TRANSACTION_BCS query failed")?;
            let tx_bcs = row_get_string(&tx_row, "BCS")
                .ok_or_else(|| anyhow!("Missing BCS in TRANSACTION_BCS for {}", self.digest))?;

            let tx_data = decode_transaction_bcs(&tx_bcs)?;
            let ptb = match tx_data.kind() {
                TransactionKind::ProgrammableTransaction(ptb) => ptb,
                other => {
                    return Err(anyhow!(
                        "Hybrid loader only supports programmable transactions (got {:?})",
                        other
                    ))
                }
            };

            let mut input_specs: Vec<InputSpec> = Vec::with_capacity(ptb.inputs.len());
            let mut object_requests: HashMap<AccountAddress, u64> = HashMap::new();
            let mut historical_versions: HashMap<String, u64> = HashMap::new();

            for input in &ptb.inputs {
                match input {
                    CallArg::Pure(bytes) => input_specs.push(InputSpec::Pure(bytes.clone())),
                    CallArg::FundsWithdrawal(_) => {
                        return Err(anyhow!(
                            "Hybrid loader does not support FundsWithdrawal inputs yet"
                        ))
                    }
                    CallArg::Object(obj_arg) => match obj_arg {
                        ObjectArg::ImmOrOwnedObject(obj_ref) => {
                            let addr = AccountAddress::from(obj_ref.0);
                            let version = obj_ref.1.value();
                            let digest = obj_ref.2.to_string();
                            input_specs.push(InputSpec::ImmOrOwned {
                                id: addr,
                                version,
                                digest,
                            });
                            object_requests.insert(addr, version);
                            historical_versions
                                .insert(normalize_address_shared(&addr.to_hex_literal()), version);
                        }
                        ObjectArg::Receiving(obj_ref) => {
                            let addr = AccountAddress::from(obj_ref.0);
                            let version = obj_ref.1.value();
                            let digest = obj_ref.2.to_string();
                            input_specs.push(InputSpec::Receiving {
                                id: addr,
                                version,
                                digest,
                            });
                            object_requests.insert(addr, version);
                            historical_versions
                                .insert(normalize_address_shared(&addr.to_hex_literal()), version);
                        }
                        ObjectArg::SharedObject {
                            id,
                            initial_shared_version,
                            mutability,
                        } => {
                            let addr = AccountAddress::from(*id);
                            let initial = initial_shared_version.value();
                            let normalized = normalize_address_shared(&addr.to_hex_literal());
                            let actual = shared_versions.get(&normalized).copied().unwrap_or(initial);
                            let mutable = matches!(mutability, SharedObjectMutability::Mutable);
                            input_specs.push(InputSpec::Shared {
                                id: addr,
                                initial_shared_version: initial,
                                mutable,
                            });
                            object_requests.insert(addr, actual);
                            historical_versions
                                .insert(normalize_address_shared(&addr.to_hex_literal()), actual);
                        }
                    },
                }
            }

            if verbose {
                eprintln!(
                    "[hybrid] inputs={} object_requests={}",
                    input_specs.len(),
                    object_requests.len()
                );
            }

            let mut objects: HashMap<AccountAddress, VersionedObject> = HashMap::new();
            let mut owner_map: HashMap<AccountAddress, GrpcOwner> = HashMap::new();
            for (addr, version) in object_requests {
                let id_hex = addr.to_hex_literal();
                if verbose {
                    eprintln!("[hybrid] fetch object {} @{}", id_hex, version);
                }
                let grpc_obj = match tokio::time::timeout(
                    Duration::from_secs(self.grpc_timeout_secs),
                    provider.grpc().get_object_at_version(&id_hex, Some(version)),
                )
                .await
                {
                    Ok(result) => result?
                        .ok_or_else(|| anyhow!("Object not found: {} @{}", id_hex, version))?,
                    Err(_) => {
                        return Err(anyhow!(
                            "gRPC timeout fetching object {} @{} ({}s)",
                            id_hex,
                            version,
                            self.grpc_timeout_secs
                        ))
                    }
                };
                if verbose {
                    eprintln!("[hybrid] fetched object {} @{}", id_hex, version);
                }
                let versioned = grpc_object_to_versioned(&grpc_obj, addr, version)?;
                owner_map.insert(addr, grpc_obj.owner.clone());
                objects.insert(addr, versioned);
            }

            if self.auto_system_objects {
                ensure_system_objects(
                    &mut objects,
                    &historical_versions,
                    timestamp_ms_opt,
                    Some(checkpoint),
                );
            }

            let inputs = build_transaction_inputs(&input_specs, &owner_map);
            let commands = convert_sui_commands(&ptb.commands)?;

            let sender = AccountAddress::from(tx_data.sender());
            let transaction = FetchedTransaction {
                digest: SandboxTransactionDigest(self.digest.clone()),
                sender,
                gas_budget: tx_data.gas_budget(),
                gas_price: tx_data.gas_price(),
                commands,
                inputs,
                effects: effects_summary,
                timestamp_ms: timestamp_ms_opt,
                checkpoint: Some(checkpoint),
            };

            let mut package_ids = collect_package_ids_from_commands(&ptb.commands);
            for obj in objects.values() {
                if let Some(type_tag) = &obj.type_tag {
                    collect_package_ids_from_type_str(type_tag, &mut package_ids);
                }
            }
            if verbose {
                eprintln!("[hybrid] package seeds={}", package_ids.len());
            }

            let mut packages: HashMap<AccountAddress, PackageData> = HashMap::new();
            let mut pending: VecDeque<AccountAddress> = package_ids.into_iter().collect();
            let mut seen: HashSet<AccountAddress> = HashSet::new();
            let mut have_storage: HashSet<AccountAddress> = HashSet::new();
            let mut have_runtime: HashSet<AccountAddress> = HashSet::new();

            while let Some(pkg_id) = pending.pop_front() {
                if !seen.insert(pkg_id) {
                    continue;
                }
                if have_storage.contains(&pkg_id) || have_runtime.contains(&pkg_id) {
                    continue;
                }

                if verbose {
                    eprintln!("[hybrid] fetch package {}", pkg_id.to_hex_literal());
                }
                let is_system_pkg = is_system_package_address(&pkg_id);
                if is_system_pkg && verbose {
                    eprintln!(
                        "[hybrid] system package -> gRPC {}",
                        pkg_id.to_hex_literal()
                    );
                }
                let mut pkg_opt = None;
                if self.snowflake_packages && !is_system_pkg {
                    pkg_opt = fetch_package_from_snowflake(
                        &mut igloo,
                        &self.analytics_db,
                        &self.analytics_schema,
                        &pkg_id,
                        Some(checkpoint),
                        timestamp_ms_opt,
                    )
                    .await?;
                }
                if pkg_opt.is_none() && self.require_snowflake_packages && !is_system_pkg {
                    return Err(anyhow!(
                        "Snowflake package missing for {}",
                        pkg_id.to_hex_literal()
                    ));
                }
                if pkg_opt.is_none() {
                    pkg_opt = fetch_package_via_grpc(provider, &pkg_id, None).await?;
                }

                if let Some(pkg) = pkg_opt {
                    if verbose {
                        eprintln!(
                            "[hybrid] fetched package {} (modules={})",
                            pkg.address.to_hex_literal(),
                            pkg.modules.len()
                        );
                    }
                    let deps = extract_module_dependency_ids(&pkg.modules);
                    for dep in deps {
                        if !seen.contains(&dep)
                            && !have_storage.contains(&dep)
                            && !have_runtime.contains(&dep)
                        {
                            pending.push_back(dep);
                        }
                    }
                    have_storage.insert(pkg.address);
                    have_runtime.insert(pkg.runtime_id());
                    packages.insert(pkg.address, pkg);
                } else if verbose {
                    eprintln!("[hybrid] missing package {}", pkg_id.to_hex_literal());
                }
            }

            let mut protocol_version = 0u64;
            let mut reference_gas_price = None;
            if epoch > 0 {
                match tokio::time::timeout(
                    Duration::from_secs(self.grpc_timeout_secs),
                    provider.grpc().get_epoch(Some(epoch)),
                )
                .await
                {
                    Ok(Ok(Some(ep))) => {
                        protocol_version = ep.protocol_version.unwrap_or(0);
                        reference_gas_price = ep.reference_gas_price;
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => {
                        if verbose {
                            eprintln!(
                                "[hybrid] get_epoch failed for epoch {}: {}",
                                epoch, err
                            );
                        }
                    }
                    Err(_) => {
                        if verbose {
                            eprintln!(
                                "[hybrid] get_epoch timeout for epoch {} ({}s)",
                                epoch, self.grpc_timeout_secs
                            );
                        }
                    }
                }
            }

            Ok(ReplayState {
                transaction,
                objects,
                packages,
                protocol_version,
                epoch,
                reference_gas_price,
                checkpoint: Some(checkpoint),
            })
        }
        .await;

        let _ = igloo.shutdown().await;
        result
    }
}

#[derive(Debug, Clone)]
enum InputSpec {
    Pure(Vec<u8>),
    ImmOrOwned {
        id: AccountAddress,
        version: u64,
        digest: String,
    },
    Receiving {
        id: AccountAddress,
        version: u64,
        digest: String,
    },
    Shared {
        id: AccountAddress,
        initial_shared_version: u64,
        mutable: bool,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
struct IglooConfigFile {
    igloo: Option<IglooConfigSection>,
    snowflake: Option<SnowflakeConfigSection>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct IglooConfigSection {
    command: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SnowflakeConfigSection {
    account: Option<String>,
    user: Option<String>,
    database: Option<String>,
    schema: Option<String>,
    warehouse: Option<String>,
    role: Option<String>,
    authenticator: Option<String>,
}

#[derive(Debug, Clone)]
struct IglooConfig {
    command: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
    snowflake: Option<SnowflakeConfigSection>,
}

impl Default for IglooConfig {
    fn default() -> Self {
        Self {
            command: "igloo_mcp".to_string(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            snowflake: None,
        }
    }
}

struct IglooMcpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

const IGLOO_QUERY_TIMEOUT_SECS: u64 = 120;

impl IglooMcpClient {
    async fn connect(config: IglooConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().context("Failed to spawn igloo-mcp")?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to open igloo-mcp stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to open igloo-mcp stdout"))?;
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&mut self) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        let init = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {
                    "name": "sui-sandbox",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }
        });
        self.send_message(&init).await?;
        let _ = self.read_response(id).await?;

        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        self.send_message(&initialized).await?;
        Ok(())
    }

    async fn query_one(&mut self, statement: &str, reason: &str) -> Result<Value> {
        let rows = self.query_rows(statement, reason).await?;
        rows.into_iter()
            .next()
            .ok_or_else(|| anyhow!("No rows returned for query"))
    }

    async fn query_rows(&mut self, statement: &str, reason: &str) -> Result<Vec<Value>> {
        let payload = serde_json::json!({
            "statement": statement,
            "reason": reason,
            "result_mode": "full",
            "timeout_seconds": IGLOO_QUERY_TIMEOUT_SECS,
        });
        let result = self.call_tool("execute_query", payload).await?;
        let mut structured = result
            .get("structuredContent")
            .cloned()
            .unwrap_or(result.clone());
        if let Some(inner) = structured.get("result") {
            structured = inner.clone();
        }
        let rows = structured
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(rows)
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
            }
        });
        self.send_message(&request).await?;
        let result = self.read_response(id).await?;
        if result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let message = extract_text_from_content(result.get("content"))
                .unwrap_or_else(|| "igloo-mcp tool error".to_string());
            return Err(anyhow!("igloo-mcp {} failed: {}", name, message));
        }
        Ok(result)
    }

    async fn send_message(&mut self, value: &Value) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.stdin.write_all(json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_response(&mut self, expected_id: u64) -> Result<Value> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self.stdout.read_line(&mut line).await?;
            if bytes == 0 {
                return Err(anyhow!("igloo-mcp closed stdout unexpectedly"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed: Value = match serde_json::from_str(trimmed) {
                Ok(val) => val,
                Err(_) => continue,
            };
            let Some(id_value) = parsed.get("id") else {
                continue;
            };
            if !json_id_matches(id_value, expected_id) {
                continue;
            }
            if let Some(err) = parsed.get("error") {
                return Err(anyhow!("igloo-mcp error: {}", err));
            }
            return Ok(parsed.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        let _ = self.stdin.shutdown().await;
        let wait = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
        if wait.is_err() {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
        Ok(())
    }
}

fn env_bool_opt(key: &str) -> Option<bool> {
    std::env::var(key).ok().map(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn find_mcp_service_config(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors().take(6) {
        let candidate = ancestor.join("mcp_service_config.json");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn find_dotenv(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors().take(6) {
        let candidate = ancestor.join(".env");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn load_dotenv_vars() -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let Ok(start) = std::env::current_dir() else {
        return vars;
    };
    let Some(path) = find_dotenv(&start) else {
        return vars;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return vars;
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => k.trim(),
            None => continue,
        };
        let value = parts.next().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        let unquoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value)
            .to_string();
        vars.insert(key.to_string(), unquoted);
    }
    vars
}

fn load_igloo_config(path: &Path) -> Result<IglooConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read igloo config: {}", path.display()))?;
    let parsed: IglooConfigFile =
        serde_json::from_str(&raw).context("Failed to parse igloo config JSON")?;
    let igloo = parsed.igloo.unwrap_or_default();
    Ok(IglooConfig {
        command: igloo.command.unwrap_or_else(|| "igloo_mcp".to_string()),
        args: igloo.args.unwrap_or_default(),
        cwd: igloo.cwd.map(PathBuf::from),
        env: igloo.env.unwrap_or_default(),
        snowflake: parsed.snowflake,
    })
}

fn resolve_igloo_command(command: &str) -> Option<String> {
    let path = Path::new(command);
    if command.contains('/') || path.is_absolute() {
        if path.exists() {
            return Some(command.to_string());
        }
        if command.ends_with("igloo-mcp") {
            let alt = command.replace("igloo-mcp", "igloo_mcp");
            if Path::new(&alt).exists() {
                return Some(alt);
            }
        }
        return find_in_path("igloo_mcp");
    }
    find_in_path(command).or_else(|| find_in_path("igloo_mcp"))
}

fn find_in_path(command: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(command);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn apply_snowflake_config(config: &mut IglooConfig) {
    let Some(sf) = config.snowflake.as_ref() else {
        return;
    };
    let has_profile = has_arg(&config.args, &["--profile"])
        || config.env.contains_key("SNOWFLAKE_PROFILE")
        || config.env.contains_key("SNOWCLI_DEFAULT_PROFILE");
    if has_profile {
        strip_login_env(&mut config.env);
        set_env_if_missing(&mut config.env, "SNOWFLAKE_DATABASE", sf.database.as_ref());
        set_env_if_missing(&mut config.env, "SNOWFLAKE_SCHEMA", sf.schema.as_ref());
        return;
    }

    push_arg_if_missing(
        &mut config.args,
        &["--account", "--account-identifier"],
        sf.account.as_deref(),
    );
    push_arg_if_missing(
        &mut config.args,
        &["--user", "--username"],
        sf.user.as_deref(),
    );
    push_arg_if_missing(&mut config.args, &["--warehouse"], sf.warehouse.as_deref());
    push_arg_if_missing(&mut config.args, &["--role"], sf.role.as_deref());
    push_arg_if_missing(
        &mut config.args,
        &["--authenticator"],
        sf.authenticator.as_deref(),
    );

    set_env_if_missing(&mut config.env, "SNOWFLAKE_ACCOUNT", sf.account.as_ref());
    set_env_if_missing(&mut config.env, "SNOWFLAKE_USER", sf.user.as_ref());
    set_env_if_missing(
        &mut config.env,
        "SNOWFLAKE_AUTHENTICATOR",
        sf.authenticator.as_ref(),
    );
    set_env_if_missing(&mut config.env, "SNOWFLAKE_DATABASE", sf.database.as_ref());
    set_env_if_missing(&mut config.env, "SNOWFLAKE_SCHEMA", sf.schema.as_ref());
    set_env_if_missing(&mut config.env, "SNOWFLAKE_WAREHOUSE", sf.warehouse.as_ref());
    set_env_if_missing(&mut config.env, "SNOWFLAKE_ROLE", sf.role.as_ref());
}

fn push_arg_if_missing(args: &mut Vec<String>, names: &[&str], value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    if value.trim().is_empty() {
        return;
    }
    if has_arg(args, names) {
        return;
    }
    args.push(names[0].to_string());
    args.push(value.to_string());
}

fn has_arg(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|arg| {
        names.iter().any(|name| {
            if arg == name {
                return true;
            }
            let prefix = format!("{}=", name);
            arg.starts_with(&prefix)
        })
    })
}

fn strip_login_env(env: &mut HashMap<String, String>) {
    for key in [
        "SNOWFLAKE_ACCOUNT",
        "SNOWFLAKE_USER",
        "SNOWFLAKE_PASSWORD",
        "SNOWFLAKE_PAT",
        "SNOWFLAKE_ROLE",
        "SNOWFLAKE_WAREHOUSE",
        "SNOWFLAKE_PASSCODE",
        "SNOWFLAKE_PASSCODE_IN_PASSWORD",
        "SNOWFLAKE_PRIVATE_KEY",
        "SNOWFLAKE_PRIVATE_KEY_FILE",
        "SNOWFLAKE_PRIVATE_KEY_FILE_PWD",
        "SNOWFLAKE_AUTHENTICATOR",
        "SNOWFLAKE_HOST",
    ] {
        env.remove(key);
    }
}

fn set_env_if_missing(env: &mut HashMap<String, String>, key: &str, value: Option<&String>) {
    if env.contains_key(key) {
        return;
    }
    let Some(value) = value else {
        return;
    };
    if value.trim().is_empty() {
        return;
    }
    env.insert(key.to_string(), value.clone());
}

fn json_id_matches(value: &Value, expected: u64) -> bool {
    match value {
        Value::Number(num) => num.as_u64() == Some(expected),
        Value::String(s) => s.parse::<u64>().ok() == Some(expected),
        _ => false,
    }
}

fn extract_text_from_content(value: Option<&Value>) -> Option<String> {
    let content = value?.as_array()?;
    for item in content {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }
    None
}

fn escape_sql_literal(input: &str) -> String {
    input.replace('\'', "''")
}

fn row_get_value<'a>(row: &'a Value, key: &str) -> Option<&'a Value> {
    let obj = row.as_object()?;
    if let Some(val) = obj.get(key) {
        return Some(val);
    }
    let upper = key.to_ascii_uppercase();
    if let Some(val) = obj.get(&upper) {
        return Some(val);
    }
    let lower = key.to_ascii_lowercase();
    obj.get(&lower)
}

fn row_get_string(row: &Value, key: &str) -> Option<String> {
    row_get_value(row, key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn row_get_u64(row: &Value, key: &str) -> Option<u64> {
    row_get_value(row, key).and_then(value_as_u64)
}

fn value_as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(num) => num.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_effects_value(value: &Value) -> Result<Value> {
    match value {
        Value::String(text) => {
            serde_json::from_str(text).context("Failed to parse EFFECTS_JSON")
        }
        Value::Object(_) | Value::Array(_) => Ok(value.clone()),
        _ => Err(anyhow!("Unexpected EFFECTS_JSON format")),
    }
}

fn parse_effects_versions(effects: &Value) -> HashMap<String, u64> {
    let mut versions = HashMap::new();
    let root = effects.get("V2").unwrap_or(effects);

    if let Some(changed) = root.get("changed_objects").and_then(|v| v.as_array()) {
        for entry in changed {
            let id = entry.get(0).and_then(|v| v.as_str());
            let Some(id_str) = id else { continue };
            let input_state = entry.get(1).and_then(|v| v.get("input_state"));
            if let Some(version) = input_state.and_then(extract_version_from_input_state) {
                versions.insert(normalize_address_shared(id_str), version);
            }
        }
    }

    if let Some(unchanged) = root
        .get("unchanged_consensus_objects")
        .and_then(|v| v.as_array())
    {
        for entry in unchanged {
            let id = entry.get(0).and_then(|v| v.as_str());
            let Some(id_str) = id else { continue };
            let info = entry.get(1);
            let Some(info) = info else { continue };
            let version = info
                .get("ReadOnlyRoot")
                .or_else(|| info.get("ReadOnly"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.get(0))
                .and_then(value_as_u64);
            if let Some(ver) = version {
                versions.insert(normalize_address_shared(id_str), ver);
            }
        }
    }

    versions
}

fn extract_version_from_input_state(input_state: &Value) -> Option<u64> {
    let exist = input_state.get("Exist")?;
    let arr = exist.as_array()?;
    let version_entry = arr.get(0)?.as_array()?;
    value_as_u64(version_entry.get(0)?)
}

fn extract_executed_epoch(effects: &Value) -> Option<u64> {
    let root = effects.get("V2").unwrap_or(effects);
    root.get("executed_epoch").and_then(value_as_u64)
}

fn build_effects_summary(
    effects: &Value,
    shared_versions: &HashMap<String, u64>,
) -> Option<TransactionEffectsSummary> {
    let root = effects.get("V2").unwrap_or(effects);
    let status_value = root.get("status")?;
    let status = match status_value {
        Value::String(s) => {
            if s.eq_ignore_ascii_case("success") {
                TransactionStatus::Success
            } else {
                TransactionStatus::Failure { error: s.clone() }
            }
        }
        Value::Object(map) => {
            if map.contains_key("Success") {
                TransactionStatus::Success
            } else if let Some(err) = map
                .get("Failure")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            {
                TransactionStatus::Failure { error: err }
            } else {
                TransactionStatus::Failure {
                    error: "Unknown failure".to_string(),
                }
            }
        }
        _ => TransactionStatus::Failure {
            error: "Unknown failure".to_string(),
        },
    };

    let mut created = Vec::new();
    let mut mutated = Vec::new();
    let mut deleted = Vec::new();

    if let Some(changed) = root.get("changed_objects").and_then(|v| v.as_array()) {
        for entry in changed {
            let id = entry.get(0).and_then(|v| v.as_str()).map(|s| s.to_string());
            let Some(id) = id else { continue };
            let op = entry
                .get(1)
                .and_then(|v| v.get("id_operation"))
                .and_then(|v| v.as_str())
                .unwrap_or("None");
            match op {
                "Created" => created.push(id),
                "Deleted" => deleted.push(id),
                _ => mutated.push(id),
            }
        }
    }

    Some(TransactionEffectsSummary {
        status,
        created,
        mutated,
        deleted,
        wrapped: Vec::new(),
        unwrapped: Vec::new(),
        gas_used: GasSummary::default(),
        events_count: 0,
        shared_object_versions: shared_versions.clone(),
    })
}

fn decode_transaction_bcs(bcs_str: &str) -> Result<TransactionData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bcs_str)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(bcs_str))
        .context("Failed to decode transaction BCS")?;
    let tx: TransactionData = bcs::from_bytes(&bytes).context("Failed to parse transaction BCS")?;
    Ok(tx)
}

fn convert_sui_commands(commands: &[SuiCommand]) -> Result<Vec<PtbCommand>> {
    commands.iter().map(convert_sui_command).collect()
}

fn convert_sui_command(command: &SuiCommand) -> Result<PtbCommand> {
    Ok(match command {
        SuiCommand::MoveCall(call) => PtbCommand::MoveCall {
            package: format!("{}", call.package),
            module: call.module.clone(),
            function: call.function.clone(),
            type_arguments: call
                .type_arguments
                .iter()
                .map(|t| t.to_canonical_string(true))
                .collect(),
            arguments: call.arguments.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::TransferObjects(objs, addr) => PtbCommand::TransferObjects {
            objects: objs.iter().map(convert_sui_argument).collect(),
            address: convert_sui_argument(addr),
        },
        SuiCommand::SplitCoins(coin, amounts) => PtbCommand::SplitCoins {
            coin: convert_sui_argument(coin),
            amounts: amounts.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::MergeCoins(dest, sources) => PtbCommand::MergeCoins {
            destination: convert_sui_argument(dest),
            sources: sources.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::MakeMoveVec(type_arg, elems) => PtbCommand::MakeMoveVec {
            type_arg: type_arg.as_ref().map(|t| t.to_canonical_string(true)),
            elements: elems.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::Publish(modules, deps) => PtbCommand::Publish {
            modules: modules
                .iter()
                .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                .collect(),
            dependencies: deps.iter().map(|d| format!("{}", d)).collect(),
        },
        SuiCommand::Upgrade(modules, _deps, package, ticket) => PtbCommand::Upgrade {
            modules: modules
                .iter()
                .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                .collect(),
            package: format!("{}", package),
            ticket: convert_sui_argument(ticket),
        },
    })
}

fn convert_sui_argument(arg: &SuiArgument) -> PtbArgument {
    match arg {
        SuiArgument::GasCoin => PtbArgument::GasCoin,
        SuiArgument::Input(index) => PtbArgument::Input { index: *index },
        SuiArgument::Result(index) => PtbArgument::Result { index: *index },
        SuiArgument::NestedResult(index, result_index) => PtbArgument::NestedResult {
            index: *index,
            result_index: *result_index,
        },
    }
}

fn build_transaction_inputs(
    specs: &[InputSpec],
    owner_map: &HashMap<AccountAddress, GrpcOwner>,
) -> Vec<TransactionInput> {
    specs
        .iter()
        .map(|spec| match spec {
            InputSpec::Pure(bytes) => TransactionInput::Pure { bytes: bytes.clone() },
            InputSpec::ImmOrOwned { id, version, digest } => {
                let is_immutable = matches!(owner_map.get(id), Some(GrpcOwner::Immutable));
                if is_immutable {
                    TransactionInput::ImmutableObject {
                        object_id: id.to_hex_literal(),
                        version: *version,
                        digest: digest.clone(),
                    }
                } else {
                    TransactionInput::Object {
                        object_id: id.to_hex_literal(),
                        version: *version,
                        digest: digest.clone(),
                    }
                }
            }
            InputSpec::Receiving { id, version, digest } => TransactionInput::Receiving {
                object_id: id.to_hex_literal(),
                version: *version,
                digest: digest.clone(),
            },
            InputSpec::Shared {
                id,
                initial_shared_version,
                mutable,
            } => TransactionInput::SharedObject {
                object_id: id.to_hex_literal(),
                initial_shared_version: *initial_shared_version,
                mutable: *mutable,
            },
        })
        .collect()
}

fn collect_package_ids_from_commands(commands: &[SuiCommand]) -> HashSet<AccountAddress> {
    let mut packages = HashSet::new();
    for cmd in commands {
        match cmd {
            SuiCommand::MoveCall(call) => {
                packages.insert(AccountAddress::from(call.package));
                for ty in &call.type_arguments {
                    collect_packages_from_type_input(ty, &mut packages);
                }
            }
            SuiCommand::Publish(_, deps) => {
                for dep in deps {
                    packages.insert(AccountAddress::from(*dep));
                }
            }
            SuiCommand::Upgrade(_, deps, package, _) => {
                packages.insert(AccountAddress::from(*package));
                for dep in deps {
                    packages.insert(AccountAddress::from(*dep));
                }
            }
            SuiCommand::MakeMoveVec(type_arg, _) => {
                if let Some(tag) = type_arg {
                    collect_packages_from_type_input(tag, &mut packages);
                }
            }
            _ => {}
        }
    }
    packages
}

fn collect_packages_from_type_input(input: &TypeInput, out: &mut HashSet<AccountAddress>) {
    match input {
        TypeInput::Struct(s) => {
            out.insert(s.address);
            for ty in &s.type_params {
                collect_packages_from_type_input(ty, out);
            }
        }
        TypeInput::Vector(inner) => collect_packages_from_type_input(inner, out),
        _ => {}
    }
}

fn collect_package_ids_from_type_str(type_str: &str, out: &mut HashSet<AccountAddress>) {
    if let Ok(tag) = parse_type_tag(type_str) {
        collect_package_ids_from_type_tag(&tag, out);
    }
}

fn collect_package_ids_from_type_tag(tag: &TypeTag, out: &mut HashSet<AccountAddress>) {
    match tag {
        TypeTag::Struct(s) => {
            out.insert(s.address);
            for ty in &s.type_params {
                collect_package_ids_from_type_tag(ty, out);
            }
        }
        TypeTag::Vector(inner) => collect_package_ids_from_type_tag(inner, out),
        _ => {}
    }
}

async fn fetch_package_from_snowflake(
    igloo: &mut IglooMcpClient,
    database: &str,
    schema: &str,
    package_id: &AccountAddress,
    checkpoint: Option<u64>,
    timestamp_ms: Option<u64>,
) -> Result<Option<PackageData>> {
    let pkg_hex = package_id.to_hex_literal();
    let mut statement = format!(
        "select BCS, CHECKPOINT from {}.{}.MOVE_PACKAGE_BCS where PACKAGE_ID = '{}'",
        database,
        schema,
        escape_sql_literal(&pkg_hex)
    );
    if let Some(ts) = timestamp_ms {
        statement.push_str(&format!(" and TIMESTAMP_MS <= {}", ts));
    }
    if let Some(cp) = checkpoint {
        statement.push_str(&format!(" and CHECKPOINT <= {}", cp));
    }
    if timestamp_ms.is_some() {
        statement.push_str(" order by TIMESTAMP_MS desc limit 1");
    } else {
        statement.push_str(" order by CHECKPOINT desc limit 1");
    }
    let rows = igloo
        .query_rows(&statement, "hybrid replay: package_bcs")
        .await?;
    let row = match rows.first() {
        Some(row) => row,
        None => return Ok(None),
    };
    let bcs = match row_get_string(row, "BCS") {
        Some(v) => v,
        None => return Ok(None),
    };
    let pkg = decode_move_package_bcs(&bcs)?;
    Ok(Some(pkg))
}

async fn fetch_package_via_grpc(
    provider: &HistoricalStateProvider,
    package_id: &AccountAddress,
    version: Option<u64>,
) -> Result<Option<PackageData>> {
    let pkg_hex = package_id.to_hex_literal();
    let obj = provider
        .grpc()
        .get_object_at_version(&pkg_hex, version)
        .await?;
    let Some(obj) = obj else { return Ok(None) };
    if obj.package_modules.is_none() {
        return Ok(None);
    }
    let pkg = grpc_object_to_package_data(&obj, *package_id)?;
    Ok(Some(pkg))
}

fn decode_move_package_bcs(bcs_str: &str) -> Result<PackageData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bcs_str)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(bcs_str))
        .context("Failed to decode package BCS")?;
    if let Ok(obj) = bcs::from_bytes::<SuiObject>(&bytes) {
        if let SuiData::Package(pkg) = &obj.data {
            return Ok(package_data_from_move_package(pkg));
        }
    }
    let pkg: MovePackage =
        bcs::from_bytes(&bytes).context("Failed to parse package BCS")?;
    Ok(package_data_from_move_package(&pkg))
}

fn grpc_object_to_versioned(
    grpc_obj: &sui_transport::grpc::GrpcObject,
    id: AccountAddress,
    version: u64,
) -> Result<VersionedObject> {
    let (is_shared, is_immutable) = match &grpc_obj.owner {
        GrpcOwner::Shared { .. } => (true, false),
        GrpcOwner::Immutable => (false, true),
        _ => (false, false),
    };
    Ok(VersionedObject {
        id,
        version,
        digest: Some(grpc_obj.digest.clone()),
        type_tag: grpc_obj.type_string.clone(),
        bcs_bytes: grpc_obj.bcs.clone().unwrap_or_default(),
        is_shared,
        is_immutable,
    })
}

fn grpc_object_to_package_data(
    grpc_obj: &sui_transport::grpc::GrpcObject,
    address: AccountAddress,
) -> Result<PackageData> {
    let modules = grpc_obj.package_modules.clone().unwrap_or_default();
    let mut linkage = HashMap::new();
    if let Some(entries) = &grpc_obj.package_linkage {
        for entry in entries {
            if let (Ok(orig), Ok(upg)) = (
                AccountAddress::from_hex_literal(&entry.original_id),
                AccountAddress::from_hex_literal(&entry.upgraded_id),
            ) {
                linkage.insert(orig, upg);
            }
        }
    }
    let original_id = grpc_obj
        .package_original_id
        .as_ref()
        .and_then(|s| AccountAddress::from_hex_literal(s).ok());
    Ok(PackageData {
        address,
        version: grpc_obj.version,
        modules,
        linkage,
        original_id,
    })
}

fn extract_module_dependency_ids(modules: &[(String, Vec<u8>)]) -> Vec<AccountAddress> {
    let mut deps: HashSet<AccountAddress> = HashSet::new();
    for (_, bytes) in modules {
        if let Ok(module) = CompiledModule::deserialize_with_defaults(bytes) {
            for dep in module.immediate_dependencies() {
                deps.insert(*dep.address());
            }
        }
    }
    deps.into_iter().collect()
}

fn package_data_from_move_package(pkg: &MovePackage) -> PackageData {
    let modules = pkg
        .serialized_module_map()
        .iter()
        .map(|(name, bytes)| (name.clone(), bytes.clone()))
        .collect::<Vec<_>>();

    let linkage = pkg
        .linkage_table()
        .iter()
        .map(|(orig_id, info)| {
            (
                AccountAddress::from(*orig_id),
                AccountAddress::from(info.upgraded_id),
            )
        })
        .collect::<HashMap<_, _>>();

    let original_id = Some(AccountAddress::from(pkg.original_package_id()));

    PackageData {
        address: AccountAddress::from(pkg.id()),
        version: pkg.version().value(),
        modules,
        linkage,
        original_id,
    }
}

const CLOCK_TYPE: &str = "0x2::clock::Clock";
const RANDOM_TYPE: &str = "0x2::random::Random";
const DEFAULT_CLOCK_BASE_MS: u64 = 1_704_067_200_000;

fn synthesize_clock_bytes(clock_id: &AccountAddress, timestamp_ms: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(40);
    bytes.extend_from_slice(clock_id.as_ref());
    bytes.extend_from_slice(&timestamp_ms.to_le_bytes());
    bytes
}

fn synthesize_random_bytes(random_id: &AccountAddress, version: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(72);
    bytes.extend_from_slice(random_id.as_ref());
    bytes.extend_from_slice(random_id.as_ref());
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes
}

fn ensure_system_objects(
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
            type_tag: Some(CLOCK_TYPE.to_string()),
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
            type_tag: Some(RANDOM_TYPE.to_string()),
            bcs_bytes: synthesize_random_bytes(&random_id, random_version),
            is_shared: true,
            is_immutable: false,
        }
    });
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
    strict_checkpoint: bool,
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
    let strict_checkpoint = options.strict_checkpoint && checkpoint.is_some();
    let allow_latest = !strict_checkpoint;
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
                        if allow_latest {
                            gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_variant)
                        } else {
                            Ok(None)
                        }
                    }
                    Err(err) => {
                        if allow_latest {
                            gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_variant)
                        } else {
                            Err(err)
                        }
                    }
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
                    if allow_latest {
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
            Some(cp) => {
                if allow_latest {
                    gql.fetch_dynamic_fields_at_checkpoint(&parent_hex, enum_limit, cp)
                        .or_else(|_| gql.fetch_dynamic_fields(&parent_hex, enum_limit))
                } else {
                    gql.fetch_dynamic_fields_at_checkpoint(&parent_hex, enum_limit, cp)
                }
            }
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
        let fields = if allow_latest && fields.is_empty() && checkpoint.is_some() {
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
