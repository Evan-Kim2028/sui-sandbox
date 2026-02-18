use super::*;

pub(crate) fn merge_context_packages(
    replay_state: &mut ReplayState,
    context_packages: &HashMap<AccountAddress, PackageData>,
) -> usize {
    let mut inserted = 0usize;
    for (address, package) in context_packages {
        if replay_state.packages.contains_key(address) {
            continue;
        }
        replay_state.packages.insert(*address, package.clone());
        inserted += 1;
    }
    inserted
}

pub(crate) fn write_temp_context_file(payload: &serde_json::Value) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "sui_sandbox_flow_context_{}_{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::write(&path, serde_json::to_string(payload)?)
        .with_context(|| format!("Failed to write temp context file {}", path.display()))?;
    Ok(path)
}

/// Fetch a package's modules via GraphQL, returning (module_name, bytecode_bytes) pairs.
pub(crate) fn fetch_package_modules(
    graphql: &GraphQLClient,
    package_id: &str,
) -> Result<Vec<(String, Vec<u8>)>> {
    let pkg = graphql
        .fetch_package(package_id)
        .with_context(|| format!("fetch package {}", package_id))?;
    sui_transport::decode_graphql_modules(package_id, &pkg.modules)
}

/// Build a LocalModuleResolver with the Sui framework loaded, then fetch a target
/// package and its transitive dependencies via GraphQL.
pub(crate) fn build_resolver_with_deps(
    package_id: &str,
    extra_type_refs: &[String],
) -> Result<(
    sui_sandbox_core::resolver::LocalModuleResolver,
    HashSet<AccountAddress>,
)> {
    let mut resolver = sui_sandbox_core::resolver::LocalModuleResolver::with_sui_framework()?;
    let mut loaded_packages = HashSet::new();
    for fw in ["0x1", "0x2", "0x3"] {
        loaded_packages.insert(AccountAddress::from_hex_literal(fw).unwrap());
    }

    let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");
    let graphql = GraphQLClient::new(&graphql_endpoint);

    let mut to_fetch: VecDeque<AccountAddress> = VecDeque::new();
    let target_addr = AccountAddress::from_hex_literal(package_id)
        .with_context(|| format!("invalid target package: {}", package_id))?;
    if !loaded_packages.contains(&target_addr) {
        to_fetch.push_back(target_addr);
    }

    for type_str in extra_type_refs {
        for pkg_id in sui_sandbox_core::utilities::extract_package_ids_from_type(type_str) {
            if let Ok(addr) = AccountAddress::from_hex_literal(&pkg_id) {
                if !loaded_packages.contains(&addr) && !is_framework_address(&addr) {
                    to_fetch.push_back(addr);
                }
            }
        }
    }

    const MAX_DEP_ROUNDS: usize = 8;
    let mut visited = loaded_packages.clone();
    let mut rounds = 0;
    while let Some(addr) = to_fetch.pop_front() {
        if visited.contains(&addr) || is_framework_address(&addr) {
            continue;
        }
        rounds += 1;
        if rounds > MAX_DEP_ROUNDS {
            eprintln!(
                "Warning: dependency resolution hit max depth ({} packages fetched), \
                 stopping. Some transitive deps may be missing.",
                MAX_DEP_ROUNDS
            );
            break;
        }
        visited.insert(addr);

        let hex = addr.to_hex_literal();
        match fetch_package_modules(&graphql, &hex) {
            Ok(modules) => {
                let dep_addrs = extract_dependency_addrs(&modules);
                resolver.load_package_at(modules, addr)?;
                loaded_packages.insert(addr);

                for dep_addr in dep_addrs {
                    if !visited.contains(&dep_addr) && !is_framework_address(&dep_addr) {
                        to_fetch.push_back(dep_addr);
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to fetch package {}: {:#}", hex, e);
            }
        }
    }

    Ok((resolver, loaded_packages))
}

pub(crate) fn synthesize_missing_inputs(
    missing: &[sui_sandbox_core::tx_replay::MissingInputObject],
    cached_objects: &mut HashMap<String, String>,
    version_map: &mut HashMap<String, u64>,
    resolver: &sui_sandbox_core::resolver::LocalModuleResolver,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    graphql: &GraphQLClient,
    verbose: bool,
) -> Result<Vec<String>> {
    if missing.is_empty() {
        return Ok(Vec::new());
    }

    let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
    if modules.is_empty() {
        return Err(anyhow!("no modules loaded for synthesis"));
    }
    let type_model = sui_sandbox_core::mm2::TypeModel::from_modules(modules)
        .map_err(|e| anyhow!("failed to build type model: {}", e))?;
    let mut synthesizer = sui_sandbox_core::mm2::TypeSynthesizer::new(&type_model);

    let mut logs = Vec::new();
    for entry in missing {
        let object_id = entry.object_id.as_str();
        let version = entry.version;
        let mut type_string = graphql
            .fetch_object_at_version(object_id, version)
            .ok()
            .and_then(|obj| obj.type_string)
            .or_else(|| {
                graphql
                    .fetch_object(object_id)
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
        if let Ok(tag) = sui_sandbox_core::types::parse_type_tag(&type_str) {
            let rewritten = sui_sandbox_core::utilities::rewrite_type_tag(tag, aliases);
            synth_type = sui_sandbox_core::types::format_type_tag(&rewritten);
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

pub(crate) fn build_mm2_summary_from_modules(
    modules: Vec<CompiledModule>,
    verbose: bool,
) -> (Option<bool>, Option<String>) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sui_sandbox_core::mm2::TypeModel::from_modules(modules)
    }));
    match result {
        Ok(Ok(_)) => (Some(true), None),
        Ok(Err(err)) => {
            if verbose {
                eprintln!("[mm2] type model build failed: {}", err);
            }
            (Some(false), Some(err.to_string()))
        }
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            if verbose {
                eprintln!("[mm2] type model panicked: {}", msg);
            }
            (Some(false), Some(format!("mm2 panic: {}", msg)))
        }
    }
}

pub(crate) fn attach_mm2_summary_fields(
    output: &mut serde_json::Value,
    modules: Vec<CompiledModule>,
    verbose: bool,
) {
    let (mm2_ok, mm2_error) = build_mm2_summary_from_modules(modules, verbose);
    if let Some(analysis) = output
        .get_mut("analysis")
        .and_then(serde_json::Value::as_object_mut)
    {
        analysis.insert("mm2_model_ok".to_string(), serde_json::json!(mm2_ok));
        analysis.insert("mm2_error".to_string(), serde_json::json!(mm2_error));
    }
    if let Some(object) = output.as_object_mut() {
        object.insert("mm2_model_ok".to_string(), serde_json::json!(mm2_ok));
        object.insert("mm2_error".to_string(), serde_json::json!(mm2_error));
    }
}

pub(crate) fn enable_self_heal_fetchers(
    harness: &mut sui_sandbox_core::vm::VMHarness,
    graphql: &GraphQLClient,
    checkpoint: Option<u64>,
    max_version: u64,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    modules: &[CompiledModule],
) {
    let graphql_for_versioned = graphql.clone();
    harness.set_versioned_child_fetcher(Box::new(move |_parent, child_id| {
        let child_hex = child_id.to_hex_literal();
        let object = checkpoint
            .and_then(|cp| {
                graphql_for_versioned
                    .fetch_object_at_checkpoint(&child_hex, cp)
                    .ok()
            })
            .or_else(|| graphql_for_versioned.fetch_object(&child_hex).ok())?;

        if object.version > max_version {
            return None;
        }
        let (type_str, bcs_b64) = (object.type_string?, object.bcs_base64?);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bcs_b64.as_bytes())
            .ok()?;
        let tag = sui_sandbox_core::types::parse_type_tag(&type_str).ok()?;
        Some((tag, bytes, object.version))
    }));

    let graphql_for_key = graphql.clone();
    let aliases_for_key = aliases.clone();
    let modules_for_synth = Arc::new(modules.to_vec());
    harness.set_key_based_child_fetcher(Box::new(
        move |parent, _child_id, _key_type, key_bytes| {
            let parent_hex = parent.to_hex_literal();
            let field = graphql_for_key
                .find_dynamic_field_by_bcs(&parent_hex, key_bytes, checkpoint, 1000)
                .ok()
                .flatten()?;

            let value_type = field.value_type?;
            let parsed = sui_sandbox_core::types::parse_type_tag(&value_type).ok()?;
            let rewritten = sui_sandbox_core::utilities::rewrite_type_tag(parsed, &aliases_for_key);

            if let Some(bcs_b64) = field.value_bcs.as_deref() {
                if let Ok(bytes) =
                    base64::engine::general_purpose::STANDARD.decode(bcs_b64.as_bytes())
                {
                    return Some((rewritten, bytes));
                }
            }

            let synth_type = sui_sandbox_core::types::format_type_tag(&rewritten);
            let type_model =
                sui_sandbox_core::mm2::TypeModel::from_modules(modules_for_synth.as_ref().clone())
                    .ok()?;
            let mut synthesizer = sui_sandbox_core::mm2::TypeSynthesizer::new(&type_model);
            let mut result = synthesizer.synthesize_with_fallback(&synth_type);
            if let Some(obj_id) = field
                .object_id
                .as_deref()
                .and_then(|id| AccountAddress::from_hex_literal(id).ok())
            {
                if result.bytes.len() >= 32 {
                    result.bytes[..32].copy_from_slice(obj_id.as_ref());
                }
            }
            Some((rewritten, result.bytes))
        },
    ));
}

// ---------------------------------------------------------------------------
// extract_interface (native)
// ---------------------------------------------------------------------------

pub(crate) fn extract_interface_inner(
    package_id: Option<&str>,
    bytecode_dir: Option<&str>,
    rpc_url: &str,
) -> Result<serde_json::Value> {
    if package_id.is_none() && bytecode_dir.is_none() {
        return Err(anyhow!(
            "Either package_id or bytecode_dir must be provided"
        ));
    }
    if package_id.is_some() && bytecode_dir.is_some() {
        return Err(anyhow!(
            "Provide either package_id or bytecode_dir, not both"
        ));
    }

    if let Some(dir) = bytecode_dir {
        let dir_path = PathBuf::from(dir);
        let compiled = read_local_compiled_modules(&dir_path)?;
        let pkg_id = resolve_local_package_id(&dir_path)?;
        let (_, interface_value) =
            build_bytecode_interface_value_from_compiled_modules(&pkg_id, &compiled)?;
        return Ok(interface_value);
    }

    let pkg_id_str = package_id.unwrap();
    let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
    let graphql = GraphQLClient::new(&graphql_endpoint);
    let pkg = graphql
        .fetch_package(pkg_id_str)
        .with_context(|| format!("fetch package {}", pkg_id_str))?;

    let raw_modules = sui_transport::decode_graphql_modules(pkg_id_str, &pkg.modules)?;
    let compiled_modules: Vec<CompiledModule> = raw_modules
        .into_iter()
        .map(|(name, bytes)| {
            CompiledModule::deserialize_with_defaults(&bytes)
                .map_err(|e| anyhow!("deserialize {}::{}: {:?}", pkg_id_str, name, e))
        })
        .collect::<Result<_>>()?;

    let (_, interface_value) =
        build_bytecode_interface_value_from_compiled_modules(pkg_id_str, &compiled_modules)?;
    Ok(interface_value)
}

// ---------------------------------------------------------------------------
// replay (native — unified analyze + execute)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_inner(
    digest: &str,
    rpc_url: &str,
    source: &str,
    checkpoint: Option<u64>,
    context_packages: Option<&HashMap<AccountAddress, PackageData>>,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    vm_only: bool,
    compare: bool,
    analyze_only: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::replay_support;
    use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy};

    // 1. Fetch ReplayState
    let mut replay_state: ReplayState;
    let graphql_client: GraphQLClient;
    let effective_source: String;

    if let Some(cp) = checkpoint {
        if verbose {
            eprintln!("[walrus] fetching checkpoint {} for digest {}", cp, digest);
        }
        let checkpoint_data = WalrusClient::mainnet()
            .get_checkpoint(cp)
            .context("Failed to fetch checkpoint from Walrus")?;
        replay_state = checkpoint_to_replay_state(&checkpoint_data, digest)
            .context("Failed to convert checkpoint to replay state")?;
        let gql_endpoint = resolve_graphql_endpoint(rpc_url);
        graphql_client = GraphQLClient::new(&gql_endpoint);
        effective_source = "walrus".to_string();
    } else {
        let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
        let gql_endpoint = resolve_graphql_endpoint(rpc_url);
        graphql_client = GraphQLClient::new(&gql_endpoint);
        let (grpc_endpoint, api_key) =
            sui_transport::grpc::historical_endpoint_and_api_key_from_env();
        let provider = rt.block_on(async {
            let grpc = sui_transport::grpc::GrpcClient::with_api_key(&grpc_endpoint, api_key)
                .await
                .context("Failed to create gRPC client")?;
            let mut provider = HistoricalStateProvider::with_clients(grpc, graphql_client.clone());
            if source == "walrus" || source == "hybrid" {
                provider = provider
                    .with_walrus_from_env()
                    .with_local_object_store_from_env();
            }
            Ok::<HistoricalStateProvider, anyhow::Error>(provider)
        })?;

        let prefetch_dynamic_fields = !no_prefetch;
        replay_state = rt.block_on(async {
            provider
                .replay_state_builder()
                .with_config(sui_state_fetcher::ReplayStateConfig {
                    prefetch_dynamic_fields,
                    df_depth: prefetch_depth,
                    df_limit: prefetch_limit,
                    auto_system_objects,
                })
                .build(digest)
                .await
                .context("Failed to fetch replay state")
        })?;
        effective_source = source.to_string();
    }

    if let Some(context_packages) = context_packages {
        let merged = merge_context_packages(&mut replay_state, context_packages);
        if verbose && merged > 0 {
            eprintln!(
                "[context] merged {} package(s) from prepared context before replay",
                merged
            );
        }
    }

    if verbose {
        eprintln!(
            "  Sender: {}",
            replay_state.transaction.sender.to_hex_literal()
        );
        eprintln!("  Commands: {}", replay_state.transaction.commands.len());
        eprintln!("  Inputs: {}", replay_state.transaction.inputs.len());
        eprintln!(
            "  Objects: {}, Packages: {}",
            replay_state.objects.len(),
            replay_state.packages.len()
        );
    }

    // 2. Analyze-only: return state summary without VM execution
    if analyze_only {
        let mut output = build_analyze_replay_output(
            &replay_state,
            source,
            &effective_source,
            vm_only,
            allow_fallback,
            auto_system_objects,
            !no_prefetch,
            prefetch_depth,
            prefetch_limit,
            verbose,
        )?;
        if analyze_mm2 {
            let pkg_aliases = build_aliases(&replay_state.packages, None, replay_state.checkpoint);
            let mut resolver = replay_support::hydrate_resolver_from_replay_state(
                &replay_state,
                &pkg_aliases.linkage_upgrades,
                &pkg_aliases.aliases,
            )?;
            let _ = replay_support::fetch_dependency_closure(
                &mut resolver,
                &graphql_client,
                replay_state.checkpoint,
                verbose,
            );
            let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
            attach_mm2_summary_fields(&mut output, modules, verbose);
        }
        return Ok(output);
    }

    // 3. Full replay: build resolver, fetch deps, execute VM
    let pkg_aliases = build_aliases(&replay_state.packages, None, replay_state.checkpoint);
    let mut resolver = replay_support::hydrate_resolver_from_replay_state(
        &replay_state,
        &pkg_aliases.linkage_upgrades,
        &pkg_aliases.aliases,
    )?;

    let fetched_deps = replay_support::fetch_dependency_closure(
        &mut resolver,
        &graphql_client,
        replay_state.checkpoint,
        verbose,
    )
    .unwrap_or(0);
    if verbose && fetched_deps > 0 {
        eprintln!("[deps] fetched {} dependency packages", fetched_deps);
    }

    let mut maps = replay_support::build_replay_object_maps(&replay_state, &pkg_aliases.versions);
    replay_support::maybe_patch_replay_objects(
        &resolver,
        &replay_state,
        &pkg_aliases.versions,
        &pkg_aliases.aliases,
        &mut maps,
        verbose,
    );

    let config = replay_support::build_simulation_config(&replay_state);
    let mut harness = sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
    harness
        .set_address_aliases_with_versions(pkg_aliases.aliases.clone(), maps.versions_str.clone());
    if self_heal_dynamic_fields {
        let max_version = maps.version_map.values().copied().max().unwrap_or(0);
        let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
        if !modules.is_empty() {
            let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
            let graphql = GraphQLClient::new(&graphql_endpoint);
            enable_self_heal_fetchers(
                &mut harness,
                &graphql,
                replay_state.checkpoint,
                max_version,
                &pkg_aliases.aliases,
                &modules,
            );
        }
    }

    let reconcile_policy = EffectsReconcilePolicy::Strict;
    let mut replay_result = tx_replay::replay_with_version_tracking_with_policy_with_effects(
        &replay_state.transaction,
        &mut harness,
        &maps.cached_objects,
        &pkg_aliases.aliases,
        Some(&maps.versions_str),
        reconcile_policy,
    );
    let mut synthetic_inputs = 0usize;
    if synthesize_missing
        && replay_result
            .as_ref()
            .map(|result| !result.result.local_success)
            .unwrap_or(true)
    {
        let missing =
            tx_replay::find_missing_input_objects(&replay_state.transaction, &maps.cached_objects);
        if !missing.is_empty() {
            match synthesize_missing_inputs(
                &missing,
                &mut maps.cached_objects,
                &mut maps.version_map,
                &resolver,
                &pkg_aliases.aliases,
                &graphql_client,
                verbose,
            ) {
                Ok(logs) => {
                    synthetic_inputs = logs.len();
                    if verbose && synthetic_inputs > 0 {
                        eprintln!(
                            "[replay_fallback] synthesized {} missing input object(s)",
                            synthetic_inputs
                        );
                    }
                    if synthetic_inputs > 0 {
                        replay_result =
                            tx_replay::replay_with_version_tracking_with_policy_with_effects(
                                &replay_state.transaction,
                                &mut harness,
                                &maps.cached_objects,
                                &pkg_aliases.aliases,
                                Some(&maps.versions_str),
                                reconcile_policy,
                            );
                    }
                }
                Err(err) => {
                    if verbose {
                        eprintln!("[replay_fallback] synthesis failed: {}", err);
                    }
                }
            }
        }
    }

    // 4. Build output JSON
    build_replay_output(
        &replay_state,
        replay_result,
        source,
        &effective_source,
        vm_only,
        allow_fallback,
        auto_system_objects,
        !no_prefetch,
        prefetch_depth,
        prefetch_limit,
        "graphql_dependency_closure",
        fetched_deps,
        synthetic_inputs,
        compare,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_loaded_state_inner(
    mut replay_state: ReplayState,
    requested_source: &str,
    effective_source: &str,
    context_packages: Option<&HashMap<AccountAddress, PackageData>>,
    allow_fallback: bool,
    auto_system_objects: bool,
    self_heal_dynamic_fields: bool,
    vm_only: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    analyze_mm2: bool,
    rpc_url: &str,
    verbose: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::replay_support;
    use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy};

    if let Some(context_packages) = context_packages {
        let merged = merge_context_packages(&mut replay_state, context_packages);
        if verbose && merged > 0 {
            eprintln!(
                "[context] merged {} package(s) from prepared context before replay",
                merged
            );
        }
    }

    if analyze_only {
        let mut output = build_analyze_replay_output(
            &replay_state,
            requested_source,
            effective_source,
            vm_only,
            allow_fallback,
            auto_system_objects,
            false,
            0,
            0,
            verbose,
        )?;
        if analyze_mm2 {
            let pkg_aliases = build_aliases(&replay_state.packages, None, replay_state.checkpoint);
            let resolver = replay_support::hydrate_resolver_from_replay_state(
                &replay_state,
                &pkg_aliases.linkage_upgrades,
                &pkg_aliases.aliases,
            )?;
            let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
            attach_mm2_summary_fields(&mut output, modules, verbose);
        }
        return Ok(output);
    }

    let pkg_aliases = build_aliases(&replay_state.packages, None, replay_state.checkpoint);
    let resolver = replay_support::hydrate_resolver_from_replay_state(
        &replay_state,
        &pkg_aliases.linkage_upgrades,
        &pkg_aliases.aliases,
    )?;

    let mut maps = replay_support::build_replay_object_maps(&replay_state, &pkg_aliases.versions);
    replay_support::maybe_patch_replay_objects(
        &resolver,
        &replay_state,
        &pkg_aliases.versions,
        &pkg_aliases.aliases,
        &mut maps,
        verbose,
    );

    let config = replay_support::build_simulation_config(&replay_state);
    let mut harness = sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
    harness
        .set_address_aliases_with_versions(pkg_aliases.aliases.clone(), maps.versions_str.clone());
    if self_heal_dynamic_fields {
        let max_version = maps.version_map.values().copied().max().unwrap_or(0);
        let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
        if !modules.is_empty() {
            let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
            let graphql = GraphQLClient::new(&graphql_endpoint);
            enable_self_heal_fetchers(
                &mut harness,
                &graphql,
                replay_state.checkpoint,
                max_version,
                &pkg_aliases.aliases,
                &modules,
            );
        }
    }

    let mut replay_result = tx_replay::replay_with_version_tracking_with_policy_with_effects(
        &replay_state.transaction,
        &mut harness,
        &maps.cached_objects,
        &pkg_aliases.aliases,
        Some(&maps.versions_str),
        EffectsReconcilePolicy::Strict,
    );
    let mut synthetic_inputs = 0usize;
    if synthesize_missing
        && replay_result
            .as_ref()
            .map(|result| !result.result.local_success)
            .unwrap_or(true)
    {
        let missing =
            tx_replay::find_missing_input_objects(&replay_state.transaction, &maps.cached_objects);
        if !missing.is_empty() {
            let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
            let graphql = GraphQLClient::new(&graphql_endpoint);
            match synthesize_missing_inputs(
                &missing,
                &mut maps.cached_objects,
                &mut maps.version_map,
                &resolver,
                &pkg_aliases.aliases,
                &graphql,
                verbose,
            ) {
                Ok(logs) => {
                    synthetic_inputs = logs.len();
                    if verbose && synthetic_inputs > 0 {
                        eprintln!(
                            "[replay_fallback] synthesized {} missing input object(s)",
                            synthetic_inputs
                        );
                    }
                    if synthetic_inputs > 0 {
                        replay_result =
                            tx_replay::replay_with_version_tracking_with_policy_with_effects(
                                &replay_state.transaction,
                                &mut harness,
                                &maps.cached_objects,
                                &pkg_aliases.aliases,
                                Some(&maps.versions_str),
                                EffectsReconcilePolicy::Strict,
                            );
                    }
                }
                Err(err) => {
                    if verbose {
                        eprintln!("[replay_fallback] synthesis failed: {}", err);
                    }
                }
            }
        }
    }

    build_replay_output(
        &replay_state,
        replay_result,
        requested_source,
        effective_source,
        vm_only,
        allow_fallback,
        auto_system_objects,
        false,
        0,
        0,
        effective_source,
        0,
        synthetic_inputs,
        compare,
    )
}
