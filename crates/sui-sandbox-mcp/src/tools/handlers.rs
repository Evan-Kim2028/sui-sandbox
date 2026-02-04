//! MCP tool handler implementations.

use crate::state::{ProviderConfig, ToolDispatcher, ToolResponse};
use anyhow::{anyhow, Result};
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::Mutex;
use sui_sandbox_types::{env_bool, env_var_or};

use sui_prefetch::compute_dynamic_field_id;
use sui_resolver::normalize_address;
use sui_sandbox_core::mm2::{TypeModel, TypeSynthesizer};
use sui_sandbox_core::ptb::{Argument, Command as PtbCommand, InputValue, ObjectInput};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::shared::{
    decode_b64, decode_b64_no_pad_opt, decode_b64_opt, encode_b64, extract_input, parse_address,
};
use sui_sandbox_core::simulation::SimulationEnvironment;
use sui_sandbox_core::types::{format_type_tag, parse_type_tag};
use sui_sandbox_core::utilities::extract_package_ids_from_type_tag;
use sui_sandbox_core::utilities::rewrite_type_tag;
use sui_sandbox_core::{ptb, tx_replay};
use sui_state_fetcher::types::{PackageData, VersionedObject};
use sui_state_fetcher::{
    build_aliases as build_aliases_shared, fetch_child_object as fetch_child_object_shared,
    fetch_object_via_grpc as fetch_object_via_grpc_shared, HistoricalStateProvider,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcObject, GrpcOwner};
use sui_transport::walrus::WalrusClient;
use sui_types::digests::TransactionDigest;

// Import all input types from the inputs module
use super::inputs::{
    CachePolicy, CallFunctionInput, ConfigureInput, CreateAssetInput, CreateProjectInput,
    EditFileInput, ExecutePtbInput, FetchStrategy, GetInterfaceInput, GetStateInput,
    ListPackagesInput, ListProjectsInput, LoadFromMainnetInput, LoadPackageBytesInput,
    ProjectIdInput, PtbOptions, ReadFileInput, ReadObjectInput, ReplayInput, SearchInput,
    SetActivePackageInput, TestProjectInput, UpgradeProjectInput, WalrusFetchInput,
};

impl ToolDispatcher {
    pub async fn call_function(&self, input: Value) -> ToolResponse {
        let parsed: CallFunctionInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        let mut inputs: Vec<InputValue> = Vec::new();
        let mut args: Vec<Argument> = Vec::new();

        for arg in &parsed.args {
            match parse_arg_reference(arg) {
                Ok(Some(reference)) => args.push(reference),
                Ok(None) => {
                    let input_spec = parse_input_spec(arg);
                    match input_spec {
                        Ok(InputSpec::Pure { bytes }) => {
                            let idx = inputs.len() as u16;
                            inputs.push(InputValue::Pure(bytes));
                            args.push(Argument::Input(idx));
                        }
                        Ok(InputSpec::Object(spec)) => {
                            let idx = inputs.len() as u16;
                            inputs.push(InputValue::Object(ObjectInput::Owned {
                                id: AccountAddress::ZERO,
                                bytes: Vec::new(),
                                type_tag: None,
                                version: None,
                            }));
                            args.push(Argument::Input(idx));
                            // We'll replace the placeholder once objects are resolved
                            let auto_fetch = parsed
                                .options
                                .as_ref()
                                .and_then(|o| o.fetch_missing_objects)
                                .unwrap_or(false);
                            inputs[idx as usize] = match self
                                .resolve_object_input(spec, parsed.options.clone(), auto_fetch)
                                .await
                            {
                                Ok(obj) => InputValue::Object(obj),
                                Err(e) => return ToolResponse::error(e.to_string()),
                            };
                        }
                        Err(e) => return ToolResponse::error(e),
                    }
                }
                Err(e) => return ToolResponse::error(e.to_string()),
            }
        }

        let command = match build_move_call_command(
            &parsed.package,
            &parsed.module,
            &parsed.function,
            &parsed.type_args,
            args,
        ) {
            Ok(cmd) => cmd,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let on_demand = parsed
            .options
            .as_ref()
            .and_then(|o| o.enable_on_demand_fetch)
            .unwrap_or(false);
        let provider_for_fetch: Option<std::sync::Arc<sui_state_fetcher::HistoricalStateProvider>> =
            if on_demand {
                match self.provider().await {
                    Ok(p) => Some(p),
                    Err(e) => return ToolResponse::error(e.to_string()),
                }
            } else {
                None
            };

        let mut env_guard = self.env.lock();
        if let Some(provider) = provider_for_fetch {
            let provider_clone = provider.clone();
            let fetcher = move |_parent: AccountAddress, child_id: AccountAddress| {
                fetch_child_object_shared(&provider_clone, child_id, None, u64::MAX)
            };
            env_guard.set_versioned_child_fetcher(Box::new(fetcher));
        }
        let (old_sender, old_config) = capture_env_state(&mut env_guard);
        apply_ptb_options(&mut env_guard, parsed.options.as_ref());

        let exec = env_guard.execute_ptb_with_gas_budget(
            inputs,
            vec![command],
            parsed.options.as_ref().and_then(|o| o.gas_budget),
        );

        restore_env_state(&mut env_guard, old_sender, old_config);
        drop(env_guard);

        ToolResponse::ok(exec_to_json(self, &exec))
    }

    pub async fn execute_ptb(&self, input: Value) -> ToolResponse {
        let parsed: ExecutePtbInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        if parsed.commands.is_empty() {
            return ToolResponse::error("execute_ptb requires commands".to_string());
        }

        let options = parsed.options.clone().unwrap_or_default();
        let resolution_mode = options
            .resolution_mode
            .clone()
            .unwrap_or_else(|| "strict".to_string());
        let fetch_missing =
            options.fetch_missing_objects.unwrap_or(false) || resolution_mode == "auto";

        let mut parsed_inputs: Vec<ParsedInput> = Vec::new();
        for value in &parsed.inputs {
            match parse_input_spec(value) {
                Ok(InputSpec::Pure { bytes }) => parsed_inputs.push(ParsedInput::Pure(bytes)),
                Ok(InputSpec::Object(spec)) => parsed_inputs.push(ParsedInput::Object(spec)),
                Err(e) => return ToolResponse::error(e.to_string()),
            }
        }

        let mut commands = Vec::new();
        for command_value in &parsed.commands {
            match parse_command(command_value) {
                Ok(cmd) => commands.push(cmd),
                Err(e) => return ToolResponse::error(e.to_string()),
            }
        }

        if options.fetch_missing_packages.unwrap_or(false) || resolution_mode == "auto" {
            if let Err(e) = self
                .ensure_packages_for_commands(&commands, options.cache_policy)
                .await
            {
                return ToolResponse::error(e.to_string());
            }
        }

        if let Err(e) = self
            .prefetch_missing_objects(&parsed_inputs, fetch_missing, options.cache_policy)
            .await
        {
            return ToolResponse::error(e.to_string());
        }

        let mut inputs = Vec::new();
        for input in parsed_inputs {
            match input {
                ParsedInput::Pure(bytes) => inputs.push(InputValue::Pure(bytes)),
                ParsedInput::Object(spec) => match self
                    .resolve_object_input(spec, Some(options.clone()), fetch_missing)
                    .await
                {
                    Ok(obj) => inputs.push(InputValue::Object(obj)),
                    Err(e) => return ToolResponse::error(e.to_string()),
                },
            }
        }

        let mut env_guard = self.env.lock();
        let (old_sender, old_config) = capture_env_state(&mut env_guard);
        apply_ptb_options(&mut env_guard, Some(&options));

        let exec = env_guard.execute_ptb_with_gas_budget(inputs, commands, options.gas_budget);

        restore_env_state(&mut env_guard, old_sender, old_config);
        drop(env_guard);

        ToolResponse::ok(exec_to_json(self, &exec))
    }

    pub async fn replay_transaction(&self, input: Value) -> ToolResponse {
        let parsed: ReplayInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        let options = parsed.options.unwrap_or_default();
        let compare = options.compare_effects.unwrap_or(true);
        let prefetch_depth = options.prefetch_depth.unwrap_or(3);
        let prefetch_limit = options.prefetch_limit.unwrap_or(200);
        let fetch_strategy = options.fetch_strategy.unwrap_or_default();
        let auto_system_objects = options.auto_system_objects.unwrap_or(true);
        let reconcile_dynamic_fields = options.reconcile_dynamic_fields.unwrap_or(true);
        let synthesize_missing = options.synthesize_missing.unwrap_or(false);
        let self_heal_dynamic_fields = options.self_heal_dynamic_fields.unwrap_or(false);

        let provider = match self.provider().await {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let replay_state = match provider
            .replay_state_builder()
            .prefetch_dynamic_fields(fetch_strategy == FetchStrategy::Full)
            .dynamic_field_depth(prefetch_depth)
            .dynamic_field_limit(prefetch_limit)
            .auto_system_objects(auto_system_objects)
            .build(&parsed.digest)
            .await
        {
            Ok(state) => state,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let mut env_guard = self.env.lock();

        if options.sync_env.unwrap_or(true) {
            let mut config = env_guard.config().clone();
            if let Some(ts) = replay_state.transaction.timestamp_ms {
                config = config.with_tx_timestamp(ts);
            }
            config = config.with_epoch(replay_state.epoch);
            config = config.with_gas_budget(Some(replay_state.transaction.gas_budget));
            config = config.with_gas_price(replay_state.transaction.gas_price);
            env_guard.set_config(config);
            if let Some(ts) = replay_state.transaction.timestamp_ms {
                env_guard.set_timestamp_ms(ts);
            }
        }

        let pkg_aliases = build_aliases_shared(
            &replay_state.packages,
            Some(provider.as_ref()),
            replay_state.checkpoint,
        );
        env_guard.set_address_aliases_with_versions(
            pkg_aliases.aliases.clone(),
            pkg_aliases.versions.clone(),
        );

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
        for package in packages {
            let _ = load_package_into_env(&mut env_guard, package);
        }

        let debug_linkage = env_bool("SUI_DEBUG_LINKAGE");
        let _ = fetch_dependency_closure_mcp(
            env_guard.resolver_mut(),
            provider.graphql(),
            replay_state.checkpoint,
            debug_linkage,
        );

        let mut cached_objects: HashMap<String, String> = HashMap::new();
        let mut version_map: HashMap<String, u64> = HashMap::new();
        for (id, obj) in &replay_state.objects {
            let id_hex = id.to_hex_literal();
            cached_objects.insert(id_hex.clone(), encode_b64(&obj.bcs_bytes));
            version_map.insert(id_hex, obj.version);
        }

        let versions_str: HashMap<String, u64> = pkg_aliases
            .versions
            .iter()
            .map(|(addr, ver)| (addr.to_hex_literal(), *ver))
            .collect();

        let reconcile_policy = if reconcile_dynamic_fields {
            tx_replay::EffectsReconcilePolicy::DynamicFields
        } else {
            tx_replay::EffectsReconcilePolicy::Strict
        };

        let mut replay_result = replay_with_harness(
            &mut env_guard,
            &replay_state,
            provider.clone(),
            fetch_strategy,
            &pkg_aliases.aliases,
            &versions_str,
            &cached_objects,
            &version_map,
            reconcile_policy,
            self_heal_dynamic_fields,
        );
        let mut synthetic_logs: Vec<String> = Vec::new();

        if synthesize_missing
            && replay_result
                .as_ref()
                .map(|r| !r.local_success)
                .unwrap_or(true)
        {
            let missing =
                tx_replay::find_missing_input_objects(&replay_state.transaction, &cached_objects);
            if !missing.is_empty() {
                if let Ok(logs) = synthesize_missing_inputs_for_replay(
                    &missing,
                    &mut cached_objects,
                    &mut version_map,
                    env_guard.resolver_mut(),
                    &pkg_aliases.aliases,
                    &provider,
                ) {
                    synthetic_logs = logs;
                    if !synthetic_logs.is_empty() {
                        replay_result = replay_with_harness(
                            &mut env_guard,
                            &replay_state,
                            provider.clone(),
                            fetch_strategy,
                            &pkg_aliases.aliases,
                            &versions_str,
                            &cached_objects,
                            &version_map,
                            reconcile_policy,
                            self_heal_dynamic_fields,
                        );
                    }
                }
            }
        }

        let replay_result = match replay_result {
            Ok(result) => result,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let effects_match = replay_result
            .comparison
            .as_ref()
            .map(|cmp| {
                cmp.status_match
                    && cmp.created_count_match
                    && cmp.mutated_count_match
                    && cmp.deleted_count_match
                    && cmp.created_ids_match
                    && cmp.mutated_ids_match
                    && cmp.deleted_ids_match
            })
            .unwrap_or(false);

        let result_json = json!({
            "success": replay_result.local_success,
            "effects_match": if compare { Some(effects_match) } else { None::<bool> },
            "transaction_info": {
                "sender": replay_state.transaction.sender.to_hex_literal(),
                "timestamp_ms": replay_state.transaction.timestamp_ms,
                "checkpoint": replay_state.transaction.checkpoint,
                "gas_budget": replay_state.transaction.gas_budget,
            },
            "synthetic_inputs": if synthetic_logs.is_empty() { None::<Vec<String>> } else { Some(synthetic_logs) },
            "replay": replay_result,
        });

        ToolResponse::ok(result_json)
    }

    pub async fn create_move_project(&self, input: Value) -> ToolResponse {
        let parsed: CreateProjectInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let persist = parsed.persist.unwrap_or(false);
        let (info, files) = match self.projects.create_project(
            &parsed.name,
            parsed.initial_module.as_deref(),
            parsed.dependencies.clone(),
            persist,
        ) {
            Ok(v) => v,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        ToolResponse::ok(json!({
            "project_id": info.id,
            "path": info.path,
            "files": files,
            "persisted": info.persisted,
        }))
    }

    pub async fn read_move_file(&self, input: Value) -> ToolResponse {
        let parsed: ReadFileInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        let project_path = match self.projects.project_path(&parsed.project_id) {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };
        let file_path = match resolve_project_file(&project_path, &parsed.file) {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };
        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        ToolResponse::ok(json!({
            "file_path": file_path.to_string_lossy(),
            "content": content,
        }))
    }

    pub async fn edit_move_file(&self, input: Value) -> ToolResponse {
        let parsed: EditFileInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        let project_path = match self.projects.project_path(&parsed.project_id) {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };
        let file_path = match resolve_project_file(&project_path, &parsed.file) {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        if let Some(content) = parsed.content {
            if let Err(e) = std::fs::write(&file_path, content) {
                return ToolResponse::error(e.to_string());
            }
            if let Err(e) = self.projects.touch(&parsed.project_id) {
                return ToolResponse::error(e.to_string());
            }
            return ToolResponse::ok(json!({
                "success": true,
                "file_path": file_path.to_string_lossy(),
                "edits_applied": 0,
            }));
        }

        let edits = parsed.edits.unwrap_or_default();
        let mut content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => return ToolResponse::error(e.to_string()),
        };
        let mut applied = 0usize;
        for edit in edits {
            if content.contains(&edit.find) {
                content = content.replace(&edit.find, &edit.replace);
                applied += 1;
            }
        }
        if let Err(e) = std::fs::write(&file_path, content) {
            return ToolResponse::error(e.to_string());
        }
        if let Err(e) = self.projects.touch(&parsed.project_id) {
            return ToolResponse::error(e.to_string());
        }

        ToolResponse::ok(json!({
            "success": true,
            "file_path": file_path.to_string_lossy(),
            "edits_applied": applied,
        }))
    }

    pub async fn build_project(&self, input: Value) -> ToolResponse {
        let parsed: ProjectIdInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let project_path = match self.projects.project_path(&parsed.project_id) {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let env_guard = self.env.lock();
        match env_guard.compile_source(&project_path) {
            Ok(result) => {
                let modules: Vec<String> = result
                    .modules
                    .iter()
                    .filter_map(|p| {
                        p.file_stem()
                            .and_then(|s| s.to_str())
                            .map(|s| s.to_string())
                    })
                    .collect();
                ToolResponse::ok(json!({
                    "success": true,
                    "modules": modules,
                    "warnings": result.warnings,
                }))
            }
            Err(err) => {
                let errors: Vec<Value> = err
                    .errors
                    .into_iter()
                    .map(|e| {
                        json!({
                            "file": e.file,
                            "line": e.line,
                            "column": e.column,
                            "message": e.message,
                        })
                    })
                    .collect();
                ToolResponse::ok(json!({
                    "success": false,
                    "errors": errors,
                    "raw_output": err.raw_output,
                }))
            }
        }
    }

    pub async fn test_project(&self, input: Value) -> ToolResponse {
        let parsed: TestProjectInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let project_path = match self.projects.project_path(&parsed.project_id) {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let mut cmd = Command::new("sui");
        cmd.args(["move", "test", "--path"]);
        cmd.arg(&project_path);
        if let Some(filter) = parsed.filter.as_ref() {
            cmd.arg(filter);
        }
        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => return ToolResponse::error(format!("Failed to run sui move test: {}", e)),
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();
        ToolResponse::ok(json!({
            "success": success,
            "stdout": stdout,
            "stderr": stderr,
        }))
    }

    pub async fn deploy_project(&self, input: Value) -> ToolResponse {
        let parsed: ProjectIdInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let project_path = match self.projects.project_path(&parsed.project_id) {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let mut env_guard = self.env.lock();
        let result = match env_guard.compile_and_deploy(&project_path) {
            Ok(v) => v,
            Err(e) => return ToolResponse::error(e.to_string()),
        };
        let package_id = result.0.to_hex_literal();
        let modules = result.1;
        drop(env_guard);

        let info = match self
            .projects
            .register_package(&parsed.project_id, &package_id)
        {
            Ok(i) => i,
            Err(e) => return ToolResponse::error(e.to_string()),
        };
        let object_ref = self.register_object_ref(&package_id);
        ToolResponse::ok(json!({
            "package_id": package_id,
            "package_ref": object_ref,
            "modules": modules,
            "project": info,
        }))
    }

    pub async fn list_projects(&self, input: Value) -> ToolResponse {
        let parsed: ListProjectsInput =
            serde_json::from_value(input).unwrap_or(ListProjectsInput {
                include_paths: Some(true),
            });
        let projects = self.projects.list_projects();
        let include_paths = parsed.include_paths.unwrap_or(true);
        let list: Vec<Value> = projects
            .into_iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "path": if include_paths { Some(p.path) } else { None },
                    "persisted": p.persisted,
                    "active_package": p.active_package,
                    "dependencies": p.dependencies,
                })
            })
            .collect();
        ToolResponse::ok(json!({ "projects": list }))
    }

    pub async fn list_packages(&self, input: Value) -> ToolResponse {
        let parsed: ListPackagesInput =
            serde_json::from_value(input).unwrap_or(ListPackagesInput {
                limit: Some(200),
                cursor: Some(0),
            });
        let limit = parsed.limit.unwrap_or(200);
        let cursor = parsed.cursor.unwrap_or(0);

        let env_guard = self.env.lock();
        let packages = env_guard.list_available_packages();
        let total = packages.len();
        let slice = packages
            .into_iter()
            .skip(cursor)
            .take(limit)
            .map(|(addr, modules)| {
                json!({
                    "package_id": addr.to_hex_literal(),
                    "modules": modules,
                })
            })
            .collect::<Vec<_>>();
        ToolResponse::ok(json!({
            "packages": slice,
            "cursor": cursor,
            "next_cursor": if cursor + limit < total { Some(cursor + limit) } else { None },
            "total": total,
        }))
    }

    pub async fn set_active_package(&self, input: Value) -> ToolResponse {
        let parsed: SetActivePackageInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let info = match self
            .projects
            .set_active_package(&parsed.project_id, &parsed.package_id)
        {
            Ok(i) => i,
            Err(e) => return ToolResponse::error(e.to_string()),
        };
        ToolResponse::ok(json!({ "project": info }))
    }

    pub async fn upgrade_project(&self, input: Value) -> ToolResponse {
        let parsed: UpgradeProjectInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let project_path = match self.projects.project_path(&parsed.project_id) {
            Ok(p) => p,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let mut env_guard = self.env.lock();
        let result = match env_guard.compile_and_deploy(&project_path) {
            Ok(v) => v,
            Err(e) => return ToolResponse::error(e.to_string()),
        };
        let package_id = result.0.to_hex_literal();
        let modules = result.1;
        drop(env_guard);

        let info = match self
            .projects
            .register_package(&parsed.project_id, &package_id)
        {
            Ok(i) => i,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let mut response = json!({
            "package_id": package_id,
            "modules": modules,
            "project": info,
        });
        if let Some(upgrade_cap) = parsed.upgrade_cap {
            response["upgrade_cap"] = json!(upgrade_cap);
        }
        ToolResponse::ok(response)
            .with_warning("Upgrade executed as local redeploy; UpgradeCap not enforced.")
    }

    pub async fn read_object(&self, input: Value) -> ToolResponse {
        let parsed: ReadObjectInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        let addr = match parse_address(&parsed.object_id) {
            Ok(a) => a,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let mut obj_opt = {
            let env_guard = self.env.lock();
            if let Some(version) = parsed.version {
                env_guard.get_object_at_version(&addr, version).cloned()
            } else {
                env_guard.get_object(&addr).cloned()
            }
        };

        if obj_opt.is_none() && parsed.fetch.unwrap_or(false) {
            let _ = self
                .fetch_object_to_env(&parsed.object_id, parsed.version, None)
                .await;
            obj_opt = {
                let env_guard = self.env.lock();
                if let Some(version) = parsed.version {
                    env_guard.get_object_at_version(&addr, version).cloned()
                } else {
                    env_guard.get_object(&addr).cloned()
                }
            };
        }

        let obj = match obj_opt {
            Some(o) => o,
            None => return ToolResponse::error("Object not found".to_string()),
        };

        let object_id = obj.id.to_hex_literal();
        let object_ref = self.register_object_ref(&object_id);
        ToolResponse::ok(json!({
            "object_id": object_id,
            "object_ref": object_ref,
            "type": format_type_tag(&obj.type_tag),
            "version": obj.version,
            "is_shared": obj.is_shared,
            "is_immutable": obj.is_immutable,
            "bcs_bytes_b64": encode_b64(&obj.bcs_bytes),
        }))
    }

    pub async fn create_asset(&self, input: Value) -> ToolResponse {
        let parsed: CreateAssetInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        let mut env_guard = self.env.lock();
        let asset_type = parsed.asset_type.as_str();
        let object_id = match asset_type {
            "sui_coin" | "sui" | "coin" => {
                let amount = parsed.amount.unwrap_or(0);
                match env_guard.create_sui_coin(amount) {
                    Ok(id) => id,
                    Err(e) => return ToolResponse::error(e.to_string()),
                }
            }
            "custom_coin" => {
                let amount = parsed.amount.unwrap_or(0);
                let Some(type_tag) = parsed.type_tag.as_ref() else {
                    return ToolResponse::error("custom_coin requires type_tag".to_string());
                };
                match env_guard.create_coin(type_tag, amount) {
                    Ok(id) => id,
                    Err(e) => return ToolResponse::error(e.to_string()),
                }
            }
            "object" => {
                if let Some(bcs_b64) = parsed.bcs_bytes_b64.as_ref() {
                    let bytes = match decode_b64(bcs_b64) {
                        Ok(b) => b,
                        Err(e) => return ToolResponse::error(e.to_string()),
                    };
                    let type_tag = parsed
                        .type_tag
                        .clone()
                        .unwrap_or_else(|| "0x2::object::Object".to_string());
                    let object_id = parsed
                        .object_id
                        .clone()
                        .unwrap_or_else(|| env_guard.fresh_id().to_hex_literal());
                    let addr = match parse_address(&object_id) {
                        Ok(a) => a,
                        Err(e) => return ToolResponse::error(e.to_string()),
                    };
                    let type_tag_parsed = match parse_type_tag(&type_tag) {
                        Ok(t) => t,
                        Err(e) => return ToolResponse::error(e.to_string()),
                    };
                    env_guard.add_object_with_version_and_status(
                        addr,
                        bytes,
                        type_tag_parsed,
                        1,
                        parsed.shared.unwrap_or(false),
                        parsed.immutable.unwrap_or(false),
                    );
                    addr
                } else {
                    let Some(fields) = parsed.fields.as_ref() else {
                        return ToolResponse::error(
                            "object requires fields or bcs_bytes_b64".to_string(),
                        );
                    };
                    let type_tag = parsed
                        .type_tag
                        .clone()
                        .unwrap_or_else(|| "0x2::object::Object".to_string());
                    let specific_id = parsed
                        .object_id
                        .as_ref()
                        .and_then(|id| parse_address(id).ok())
                        .map(|addr| addr.into_bytes());
                    let id = match env_guard.create_object_from_json(&type_tag, fields, specific_id)
                    {
                        Ok(id) => id,
                        Err(e) => return ToolResponse::error(e.to_string()),
                    };
                    if parsed.shared.unwrap_or(false) || parsed.immutable.unwrap_or(false) {
                        if let Some(obj) = env_guard.get_object(&id).cloned() {
                            env_guard.add_object_with_version_and_status(
                                id,
                                obj.bcs_bytes.clone(),
                                obj.type_tag.clone(),
                                obj.version,
                                parsed.shared.unwrap_or(false),
                                parsed.immutable.unwrap_or(false),
                            );
                        }
                    }
                    id
                }
            }
            _ => return ToolResponse::error("Unknown asset type".to_string()),
        };

        let object_id = object_id.to_hex_literal();
        let object_ref = self.register_object_ref(&object_id);
        ToolResponse::ok(json!({
            "object_id": object_id,
            "object_ref": object_ref,
        }))
    }

    pub async fn load_from_mainnet(&self, input: Value) -> ToolResponse {
        let parsed: LoadFromMainnetInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        if let Some(network) = parsed.network {
            self.set_provider_config(ProviderConfig {
                network,
                grpc_endpoint: None,
                graphql_endpoint: None,
            })
            .await;
        }

        let kind = parsed.kind.to_lowercase();
        match kind.as_str() {
            "object" => {
                let result = self
                    .fetch_object_to_env(&parsed.id, parsed.version, parsed.cache_policy)
                    .await;
                match result {
                    Ok(obj) => ToolResponse::ok(obj),
                    Err(e) => ToolResponse::error(e.to_string()),
                }
            }
            "package" => {
                let result = self
                    .fetch_package_to_env(&parsed.id, parsed.version, parsed.cache_policy)
                    .await;
                match result {
                    Ok(obj) => ToolResponse::ok(obj),
                    Err(e) => ToolResponse::error(e.to_string()),
                }
            }
            _ => ToolResponse::error("kind must be 'object' or 'package'".to_string()),
        }
    }

    pub async fn load_package_bytes(&self, input: Value) -> ToolResponse {
        let parsed: LoadPackageBytesInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let addr = match parse_address(&parsed.package_id) {
            Ok(a) => a,
            Err(e) => return ToolResponse::error(format!("Invalid package_id: {}", e)),
        };
        let mut modules: Vec<(String, Vec<u8>)> = Vec::new();
        for module in parsed.modules {
            let bytes = match decode_b64(&module.bytes_b64) {
                Ok(b) => b,
                Err(e) => return ToolResponse::error(format!("Invalid module bytes: {}", e)),
            };
            modules.push((module.name, bytes));
        }
        if modules.is_empty() {
            return ToolResponse::error("modules cannot be empty".to_string());
        }
        let package = PackageData {
            address: addr,
            version: parsed.version.unwrap_or(1),
            modules,
            linkage: HashMap::new(),
            original_id: None,
        };
        let mut env_guard = self.env.lock();
        if let Err(e) = load_package_into_env(&mut env_guard, &package) {
            return ToolResponse::error(e.to_string());
        }
        drop(env_guard);
        let package_id = addr.to_hex_literal();
        let package_ref = self.register_object_ref(&package_id);
        ToolResponse::ok(json!({
            "package_id": package_id,
            "package_ref": package_ref,
            "version": package.version,
            "modules": package.modules.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        }))
    }

    pub async fn get_interface(&self, input: Value) -> ToolResponse {
        let parsed: GetInterfaceInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let package = normalize_address(&parsed.package);
        let env_guard = self.env.lock();

        let modules: Vec<String> = if let Some(module) = parsed.module.as_ref() {
            vec![format!("{}::{}", package, module)]
        } else {
            env_guard
                .list_modules()
                .into_iter()
                .filter(|m| m.starts_with(&format!("{}::", package)))
                .collect()
        };

        let mut interfaces = Vec::new();
        for module_path in modules {
            let functions = env_guard.list_functions(&module_path).unwrap_or_default();
            let structs = env_guard.list_structs(&module_path).unwrap_or_default();
            let function_info: Vec<Value> = functions
                .iter()
                .filter_map(|f| env_guard.get_function_info(&module_path, f))
                .collect();
            let struct_info: Vec<Value> = structs
                .iter()
                .filter_map(|s| {
                    let type_path = format!("{}::{}", module_path, s);
                    env_guard.get_struct_info(&type_path)
                })
                .collect();
            interfaces.push(json!({
                "module": module_path,
                "functions": function_info,
                "structs": struct_info,
            }));
        }

        ToolResponse::ok(json!({
            "package": package,
            "interfaces": interfaces,
        }))
    }

    pub async fn search(&self, input: Value) -> ToolResponse {
        let parsed: SearchInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let kind = parsed.kind.unwrap_or_else(|| "function".to_string());
        let env_guard = self.env.lock();
        let results = match kind.as_str() {
            "type" => env_guard.search_types(&parsed.pattern, None),
            "constructor" => env_guard.find_constructors(&parsed.pattern),
            _ => env_guard.search_functions(&parsed.pattern, parsed.entry_only.unwrap_or(false)),
        };
        ToolResponse::ok(json!({ "results": results }))
    }

    pub async fn get_state(&self, input: Value) -> ToolResponse {
        let parsed: GetStateInput = serde_json::from_value(input).unwrap_or(GetStateInput {
            include: None,
            limit: None,
            cursor: None,
        });
        let include = parsed
            .include
            .unwrap_or_else(|| vec!["summary".to_string()]);
        let limit = parsed.limit.unwrap_or(100);
        let cursor = parsed.cursor.unwrap_or(0);

        let env_guard = self.env.lock();
        let summary = env_guard.get_state_summary();

        let mut response = json!({
            "summary": {
                "object_count": summary.object_count,
                "loaded_packages": summary.loaded_packages,
                "loaded_modules": summary.loaded_modules,
                "sender": summary.sender,
                "timestamp_ms": summary.timestamp_ms,
            }
        });

        if include.iter().any(|v| v == "objects") {
            let objects = env_guard
                .list_objects()
                .into_iter()
                .skip(cursor)
                .take(limit)
                .map(|obj| {
                    let id = obj.id.to_hex_literal();
                    let handle = self.register_object_ref(&id);
                    json!({
                        "object_id": id,
                        "object_ref": handle,
                        "type": format_type_tag(&obj.type_tag),
                        "version": obj.version,
                        "is_shared": obj.is_shared,
                        "is_immutable": obj.is_immutable,
                    })
                })
                .collect::<Vec<_>>();
            response["objects"] = json!(objects);
            response["cursor"] = json!(cursor);
            response["next_cursor"] = if objects.len() == limit {
                json!(cursor + limit)
            } else {
                Value::Null
            };
        }

        if include.iter().any(|v| v == "events") {
            let events = env_guard
                .get_all_events()
                .iter()
                .map(|e| {
                    json!({
                        "type": e.type_tag,
                        "data_b64": encode_b64(&e.data),
                        "sequence": e.sequence,
                    })
                })
                .collect::<Vec<_>>();
            response["events"] = json!(events);
        }

        if include.iter().any(|v| v == "packages") {
            let packages = env_guard
                .list_packages()
                .into_iter()
                .map(|p| p.to_hex_literal())
                .collect::<Vec<_>>();
            response["packages"] = json!(packages);
        }

        if include.iter().any(|v| v == "config") {
            let config = env_guard.config().clone();
            response["config"] = serde_json::to_value(config).unwrap_or(Value::Null);
        }

        ToolResponse::ok(response)
    }

    pub async fn configure(&self, input: Value) -> ToolResponse {
        let parsed: ConfigureInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        match parsed.action.as_str() {
            "set_sender" => {
                let Some(params) = parsed.params else {
                    return ToolResponse::error("set_sender requires params".to_string());
                };
                let sender = match params.get("address").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => return ToolResponse::error("set_sender requires address".to_string()),
                };
                let sender_addr = match parse_address(sender) {
                    Ok(a) => a,
                    Err(e) => return ToolResponse::error(format!("Invalid sender address: {}", e)),
                };
                let mut env_guard = self.env.lock();
                env_guard.set_sender(sender_addr);
                ToolResponse::ok(json!({ "sender": sender_addr.to_hex_literal() }))
            }
            "advance_clock" => {
                let Some(params) = parsed.params else {
                    return ToolResponse::error("advance_clock requires params".to_string());
                };
                let mut env_guard = self.env.lock();
                let new_ts = if let Some(ts) = params.get("timestamp_ms").and_then(|v| v.as_u64()) {
                    ts
                } else if let Some(delta) = params.get("delta_ms").and_then(|v| v.as_u64()) {
                    env_guard.get_clock_timestamp_ms().saturating_add(delta)
                } else {
                    return ToolResponse::error(
                        "advance_clock requires timestamp_ms or delta_ms".to_string(),
                    );
                };
                if let Err(e) = env_guard.advance_clock(new_ts) {
                    return ToolResponse::error(e.to_string());
                }
                ToolResponse::ok(json!({ "timestamp_ms": new_ts }))
            }
            "save_snapshot" => {
                let Some(params) = parsed.params else {
                    return ToolResponse::error("save_snapshot requires params".to_string());
                };
                let path = match params.get("path").and_then(|v| v.as_str()) {
                    Some(p) => p,
                    None => return ToolResponse::error("save_snapshot requires path".to_string()),
                };
                let description = params
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let tags = params
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let env_guard = self.env.lock();
                if let Err(e) = env_guard.save_state_with_metadata(
                    PathBuf::from(path).as_path(),
                    description,
                    tags,
                ) {
                    return ToolResponse::error(e.to_string());
                }
                ToolResponse::ok(json!({ "path": path }))
            }
            "load_snapshot" => {
                let Some(params) = parsed.params else {
                    return ToolResponse::error("load_snapshot requires params".to_string());
                };
                let path = match params.get("path").and_then(|v| v.as_str()) {
                    Some(p) => p,
                    None => return ToolResponse::error("load_snapshot requires path".to_string()),
                };
                let mut env_guard = self.env.lock();
                if let Err(e) = env_guard.load_state(PathBuf::from(path).as_path()) {
                    return ToolResponse::error(e.to_string());
                }
                ToolResponse::ok(json!({ "path": path }))
            }
            "reset" => {
                let mut env_guard = self.env.lock();
                if let Err(e) = env_guard.reset() {
                    return ToolResponse::error(e.to_string());
                }
                ToolResponse::ok(json!({ "reset": true }))
            }
            "set_fork_anchor" => {
                self.set_fork_anchor(parsed.params.clone());
                ToolResponse::ok(json!({ "fork_anchor": parsed.params }))
            }
            "set_network" => {
                let Some(params) = parsed.params else {
                    return ToolResponse::error("set_network requires params".to_string());
                };
                let network = params
                    .get("network")
                    .and_then(|v| v.as_str())
                    .unwrap_or("mainnet");
                let grpc_endpoint = params
                    .get("grpc_endpoint")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let graphql_endpoint = params
                    .get("graphql_endpoint")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                self.set_provider_config(ProviderConfig {
                    network: network.to_string(),
                    grpc_endpoint,
                    graphql_endpoint,
                })
                .await;
                ToolResponse::ok(json!({ "network": network }))
            }
            "cache_stats" => {
                let network = self.provider_config().await.network;
                let cache = match self.cache_for_network(&network) {
                    Ok(c) => c,
                    Err(e) => return ToolResponse::error(e.to_string()),
                };
                ToolResponse::ok(json!({
                    "network": network,
                    "objects": cache.object_count(),
                    "packages": cache.package_count(),
                    "unique_objects": cache.unique_object_count(),
                    "empty": cache.is_empty(),
                }))
            }
            "clear_cache" => {
                let network = self.provider_config().await.network;
                let cache = match self.cache_for_network(&network) {
                    Ok(c) => c,
                    Err(e) => return ToolResponse::error(e.to_string()),
                };
                cache.clear();
                if let Err(e) = cache.flush() {
                    return ToolResponse::error(e.to_string());
                }
                ToolResponse::ok(json!({ "network": network, "cleared": true }))
            }
            "set_logging" => {
                let Some(params) = parsed.params else {
                    return ToolResponse::error("set_logging requires params".to_string());
                };
                let mut config = self.logger.config();
                if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
                    config.enabled = enabled;
                }
                if let Some(path) = params.get("path").and_then(|v| v.as_str()) {
                    config.path = PathBuf::from(path);
                }
                if let Some(level) = params.get("level").and_then(|v| v.as_str()) {
                    config.level = level.to_string();
                }
                if let Some(rotation_mb) = params.get("rotation_mb").and_then(|v| v.as_u64()) {
                    config.rotation_mb = rotation_mb;
                }
                self.logger.update_config(config.clone());
                ToolResponse::ok(json!({ "logging": config }))
            }
            _ => ToolResponse::error(format!("Unknown configure action: {}", parsed.action)),
        }
    }

    pub async fn walrus_fetch_checkpoints(&self, input: Value) -> ToolResponse {
        let parsed: WalrusFetchInput = match extract_input(input) {
            Ok(v) => v,
            Err(e) => return e,
        };

        let network = parsed
            .network
            .as_deref()
            .unwrap_or("mainnet")
            .to_ascii_lowercase();
        let walrus = match network.as_str() {
            "testnet" => WalrusClient::testnet(),
            _ => WalrusClient::mainnet(),
        };

        let max_chunk_bytes = parsed.max_chunk_bytes.unwrap_or(8 * 1024 * 1024);
        let batch_size = parsed.batch_size.unwrap_or(50).max(1);
        let include_summary = parsed.summary.unwrap_or(true);

        let latest = match walrus.get_latest_checkpoint() {
            Ok(v) => v,
            Err(e) => return ToolResponse::error(e.to_string()),
        };

        let checkpoints: Vec<u64> = if let Some(list) = parsed.checkpoints.clone() {
            list
        } else {
            let count = parsed.count.unwrap_or(1).max(1);
            let start = parsed
                .start_checkpoint
                .unwrap_or_else(|| latest.saturating_sub(count - 1));
            (start..start + count).collect()
        };

        let dump_dir = parsed.dump_dir.as_ref().map(PathBuf::from);
        if let Some(dir) = dump_dir.as_ref() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return ToolResponse::error(format!(
                    "failed to create dump_dir {}: {}",
                    dir.display(),
                    e
                ));
            }
        }

        let mut summaries: Vec<Value> = Vec::new();
        let mut fetched = 0usize;
        let start = std::time::Instant::now();

        for chunk in checkpoints.chunks(batch_size) {
            let mut decoded: Vec<(u64, Value)> = Vec::with_capacity(chunk.len());
            match walrus.get_checkpoints_batched(chunk, max_chunk_bytes) {
                Ok(batch) => {
                    for (cp, data) in batch {
                        let value = match serde_json::to_value(&data) {
                            Ok(v) => v,
                            Err(e) => return ToolResponse::error(e.to_string()),
                        };
                        decoded.push((cp, value));
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[walrus] batched fetch failed ({}); falling back to per-checkpoint",
                        e
                    );
                    for &cp in chunk {
                        match walrus.get_checkpoint_json(cp) {
                            Ok(value) => decoded.push((cp, value)),
                            Err(err) => {
                                eprintln!("[walrus] checkpoint {} failed in fallback: {}", cp, err)
                            }
                        }
                    }
                }
            }

            for (cp, value) in decoded {
                if let Some(dir) = dump_dir.as_ref() {
                    let path = dir.join(format!("checkpoint_{}.json", cp));
                    if let Err(e) =
                        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap_or_default())
                    {
                        return ToolResponse::error(format!(
                            "failed to write {}: {}",
                            path.display(),
                            e
                        ));
                    }
                }
                if include_summary {
                    let summary = summarize_checkpoint(&value);
                    summaries.push(json!({
                        "checkpoint": cp,
                        "transactions": summary.transactions,
                        "input_objects": summary.input_objects,
                        "output_objects": summary.output_objects,
                        "packages": summary.packages,
                        "move_objects": summary.move_objects,
                        "dynamic_fields": summary.dynamic_fields,
                    }));
                }
                fetched += 1;
            }
        }

        ToolResponse::ok(json!({
            "network": network,
            "latest_checkpoint": latest,
            "requested": checkpoints.len(),
            "fetched": fetched,
            "dump_dir": dump_dir.as_ref().map(|d| d.display().to_string()),
            "elapsed_ms": start.elapsed().as_millis(),
            "summaries": summaries,
        }))
    }
}

#[derive(Default)]
struct CheckpointSummary {
    transactions: usize,
    input_objects: usize,
    output_objects: usize,
    packages: usize,
    move_objects: usize,
    dynamic_fields: usize,
}

fn summarize_checkpoint(checkpoint_json: &Value) -> CheckpointSummary {
    let mut summary = CheckpointSummary::default();
    let Some(transactions) = checkpoint_json
        .get("transactions")
        .and_then(|v| v.as_array())
    else {
        return summary;
    };
    summary.transactions = transactions.len();
    for tx_json in transactions {
        for key in ["input_objects", "output_objects"] {
            let Some(arr) = tx_json.get(key).and_then(|v| v.as_array()) else {
                continue;
            };
            if key == "input_objects" {
                summary.input_objects += arr.len();
            } else {
                summary.output_objects += arr.len();
            }
            for obj_json in arr {
                if obj_json
                    .get("data")
                    .and_then(|d| d.get("Package"))
                    .is_some()
                {
                    summary.packages += 1;
                    continue;
                }
                if let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) {
                    summary.move_objects += 1;
                    if move_obj
                        .get("type_")
                        .and_then(|t| t.as_str())
                        .map(|t| t.contains("::dynamic_field::Field"))
                        .unwrap_or(false)
                    {
                        summary.dynamic_fields += 1;
                    }
                }
            }
        }
    }
    summary
}

#[derive(Debug, Clone)]
enum ParsedInput {
    Pure(Vec<u8>),
    Object(ObjectSpec),
}

#[derive(Debug, Clone)]
struct ObjectSpec {
    object_id: String,
    object_ref: Option<String>,
    version: Option<u64>,
    mode: Option<String>,
    shared_mutable: Option<bool>,
}

#[derive(Debug)]
enum InputSpec {
    Pure { bytes: Vec<u8> },
    Object(ObjectSpec),
}

fn parse_input_spec(value: &Value) -> Result<InputSpec, String> {
    if let Some(kind) = value.get("kind").and_then(|v| v.as_str()) {
        if kind.eq_ignore_ascii_case("pure") {
            return parse_pure_input(value);
        }
        return Ok(InputSpec::Object(parse_object_spec(value)));
    }

    if value.get("object_id").is_some() || value.get("object_ref").is_some() {
        return Ok(InputSpec::Object(parse_object_spec(value)));
    }

    parse_pure_input(value)
}

fn parse_pure_input(value: &Value) -> Result<InputSpec, String> {
    let type_str = value.get("type").and_then(|v| v.as_str());
    let val = value.get("value").unwrap_or(value);
    let bytes = encode_pure_value(val, type_str).map_err(|e| e.to_string())?;
    Ok(InputSpec::Pure { bytes })
}

fn parse_object_spec(value: &Value) -> ObjectSpec {
    let object_id = value
        .get("object_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let object_ref = value
        .get("object_ref")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let version = value.get("version").and_then(|v| v.as_u64());
    let kind = value.get("kind").and_then(|v| v.as_str());
    let mutable = value.get("mutable").and_then(|v| v.as_bool());
    let mode = if let Some(kind) = kind {
        Some(kind.to_lowercase())
    } else if let Some(true) = mutable {
        Some("mutable".to_string())
    } else {
        None
    };
    let shared_mutable = if mode.as_deref() == Some("shared") {
        mutable
    } else {
        None
    };
    ObjectSpec {
        object_id,
        object_ref,
        version,
        mode,
        shared_mutable,
    }
}

async fn ensure_object_loaded(
    dispatcher: &ToolDispatcher,
    spec: &ObjectSpec,
    cache_policy: Option<CachePolicy>,
) -> Result<()> {
    let object_id = if !spec.object_id.is_empty() {
        spec.object_id.clone()
    } else if let Some(r) = &spec.object_ref {
        dispatcher.resolve_object_ref(r).unwrap_or_default()
    } else {
        String::new()
    };
    if object_id.is_empty() {
        return Err(anyhow!("Object input missing object_id or object_ref"));
    }

    let exists = {
        let env_guard = dispatcher.env.lock();
        let addr = parse_address(&object_id)?;
        if let Some(version) = spec.version {
            env_guard.get_object_at_version(&addr, version).is_some()
        } else {
            env_guard.get_object(&addr).is_some()
        }
    };

    if exists {
        return Ok(());
    }

    let _ = dispatcher
        .fetch_object_to_env(&object_id, spec.version, cache_policy)
        .await?;
    Ok(())
}

fn parse_command(value: &Value) -> Result<PtbCommand> {
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Command missing kind"))?;

    match kind {
        "MoveCall" | "move_call" => {
            let package = value
                .get("package")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("MoveCall requires package"))?;
            let module = value
                .get("module")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("MoveCall requires module"))?;
            let function = value
                .get("function")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("MoveCall requires function"))?;
            let type_args = value
                .get("type_args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let args = parse_args(value.get("args"))?;
            build_move_call_command(package, module, function, &type_args, args)
        }
        "SplitCoins" | "split_coins" => {
            let coin = parse_arg(value.get("coin"))?;
            let amounts = parse_args(value.get("amounts"))?;
            Ok(PtbCommand::SplitCoins { coin, amounts })
        }
        "MergeCoins" | "merge_coins" => {
            let destination = parse_arg(value.get("destination"))?;
            let sources = parse_args(value.get("sources"))?;
            Ok(PtbCommand::MergeCoins {
                destination,
                sources,
            })
        }
        "TransferObjects" | "transfer_objects" => {
            let objects = parse_args(value.get("objects"))?;
            let address = parse_arg(value.get("address"))?;
            Ok(PtbCommand::TransferObjects { objects, address })
        }
        "MakeMoveVec" | "make_move_vec" => {
            let elements = parse_args(value.get("elements"))?;
            let type_tag = value
                .get("type_arg")
                .and_then(|v| v.as_str())
                .map(parse_type_tag)
                .transpose()
                .map_err(|e| anyhow!("Invalid type_arg: {}", e))?;
            Ok(PtbCommand::MakeMoveVec { type_tag, elements })
        }
        "Publish" | "publish" => {
            let modules = value
                .get("modules")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(decode_b64_opt)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let deps = value
                .get("dependencies")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|s| parse_address(s).ok())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(PtbCommand::Publish {
                modules,
                dep_ids: deps,
            })
        }
        "Upgrade" | "upgrade" => {
            let modules = value
                .get("modules")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(decode_b64_opt)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let package = value
                .get("package")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Upgrade requires package"))?;
            let package = parse_address(package)?;
            let ticket = parse_arg(value.get("ticket"))?;
            Ok(PtbCommand::Upgrade {
                modules,
                package,
                ticket,
            })
        }
        "Receive" | "receive" => {
            let object_id = value
                .get("object_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Receive requires object_id"))?;
            let addr = parse_address(object_id)?;
            let object_type = value
                .get("object_type")
                .and_then(|v| v.as_str())
                .map(parse_type_tag)
                .transpose()
                .map_err(|e| anyhow!("Invalid object_type: {}", e))?;
            Ok(PtbCommand::Receive {
                object_id: addr,
                object_type,
            })
        }
        _ => Err(anyhow!("Unknown command kind: {}", kind)),
    }
}

fn parse_arg(value: Option<&Value>) -> Result<Argument> {
    match value {
        Some(v) => parse_single_arg(v),
        None => Err(anyhow!("Argument missing")),
    }
}

fn parse_args(value: Option<&Value>) -> Result<Vec<Argument>> {
    let arr = value
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("args must be an array"))?;
    let mut args = Vec::new();
    for v in arr {
        args.push(parse_single_arg(v)?);
    }
    Ok(args)
}

fn parse_single_arg(value: &Value) -> Result<Argument> {
    if let Some(arg) = parse_arg_reference(value)? {
        return Ok(arg);
    }
    Err(anyhow!("Arguments must reference inputs/results"))
}

fn parse_arg_reference(value: &Value) -> Result<Option<Argument>> {
    if let Some(input_idx) = value.get("input").and_then(|v| v.as_u64()) {
        return Ok(Some(Argument::Input(input_idx as u16)));
    }
    if let Some(result_idx) = value.get("result").and_then(|v| v.as_u64()) {
        return Ok(Some(Argument::Result(result_idx as u16)));
    }
    if let Some(nested) = value.get("nested_result").and_then(|v| v.as_array()) {
        if nested.len() == 2 {
            if let (Some(a), Some(b)) = (nested[0].as_u64(), nested[1].as_u64()) {
                return Ok(Some(Argument::NestedResult(a as u16, b as u16)));
            }
        }
    }
    if value.get("gas_coin").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(Some(Argument::Input(0)));
    }
    if let Some(kind) = value.get("kind").and_then(|v| v.as_str()) {
        match kind {
            "Input" | "input" => {
                let idx = value
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("Input arg requires index"))?;
                return Ok(Some(Argument::Input(idx as u16)));
            }
            "Result" | "result" => {
                let idx = value
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("Result arg requires index"))?;
                return Ok(Some(Argument::Result(idx as u16)));
            }
            "NestedResult" | "nested_result" => {
                let idx = value
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("NestedResult requires index"))?;
                let nested_idx = value
                    .get("nested_index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("NestedResult requires nested_index"))?;
                return Ok(Some(Argument::NestedResult(idx as u16, nested_idx as u16)));
            }
            _ => {}
        }
    }
    Ok(None)
}

fn build_move_call_command(
    package: &str,
    module: &str,
    function: &str,
    type_args: &[String],
    args: Vec<Argument>,
) -> Result<PtbCommand> {
    let pkg_addr = parse_address(package)?;
    let module_id = Identifier::new(module)?;
    let function_id = Identifier::new(function)?;
    let parsed_type_args: Vec<TypeTag> = type_args
        .iter()
        .map(|s| parse_type_tag(s))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PtbCommand::MoveCall {
        package: pkg_addr,
        module: module_id,
        function: function_id,
        type_args: parsed_type_args,
        args,
    })
}

fn encode_pure_value(value: &Value, type_str: Option<&str>) -> Result<Vec<u8>> {
    let inferred = infer_type(value);
    let ty = type_str.unwrap_or(inferred.as_deref().unwrap_or("u64"));
    match ty {
        "u8" => {
            let n = parse_u64(value)? as u8;
            Ok(bcs::to_bytes(&n)?)
        }
        "u16" => Ok(bcs::to_bytes(&(parse_u64(value)? as u16))?),
        "u32" => Ok(bcs::to_bytes(&(parse_u64(value)? as u32))?),
        "u64" => Ok(bcs::to_bytes(&parse_u64(value)?)?),
        "u128" => Ok(bcs::to_bytes(&parse_u128(value)?)?),
        "bool" => Ok(bcs::to_bytes(&parse_bool(value)?)?),
        "address" => {
            let addr_str = value
                .as_str()
                .ok_or_else(|| anyhow!("address expects string"))?;
            let addr = parse_address(addr_str)?;
            Ok(bcs::to_bytes(&addr)?)
        }
        "vector<u8>" | "vector_u8" | "vector_u8_utf8" => {
            let bytes = if let Some(s) = value.as_str() {
                s.as_bytes().to_vec()
            } else if let Some(arr) = value.as_array() {
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as u8))
                    .collect()
            } else {
                return Err(anyhow!("vector<u8> expects string or array"));
            };
            Ok(bcs::to_bytes(&bytes)?)
        }
        "vector_u8_hex" => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow!("vector_u8_hex expects string"))?;
            let s = s.strip_prefix("0x").unwrap_or(s);
            let bytes = hex::decode(s)?;
            Ok(bcs::to_bytes(&bytes)?)
        }
        "vector_address" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("vector_address expects array"))?;
            let addrs: Vec<AccountAddress> = arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(parse_address)
                .collect::<Result<_, _>>()?;
            Ok(bcs::to_bytes(&addrs)?)
        }
        "vector_u64" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("vector_u64 expects array"))?;
            let nums: Vec<u64> = arr.iter().filter_map(|v| v.as_u64()).collect();
            Ok(bcs::to_bytes(&nums)?)
        }
        _ => Err(anyhow!("Unsupported pure type: {}", ty)),
    }
}

fn infer_type(value: &Value) -> Option<String> {
    match value {
        Value::Bool(_) => Some("bool".to_string()),
        Value::Number(_) => Some("u64".to_string()),
        Value::String(s) => {
            if s.starts_with("0x") && s.len() <= 66 {
                Some("address".to_string())
            } else {
                Some("vector_u8_utf8".to_string())
            }
        }
        Value::Array(arr) => {
            if arr.iter().all(|v| v.as_u64().is_some()) {
                Some("vector_u64".to_string())
            } else if arr.iter().all(|v| v.as_str().is_some()) {
                Some("vector_address".to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_u64(value: &Value) -> Result<u64> {
    if let Some(n) = value.as_u64() {
        return Ok(n);
    }
    if let Some(s) = value.as_str() {
        return Ok(s.parse::<u64>()?);
    }
    Err(anyhow!("Expected u64-compatible value"))
}

fn parse_u128(value: &Value) -> Result<u128> {
    if let Some(n) = value.as_u64() {
        return Ok(n as u128);
    }
    if let Some(s) = value.as_str() {
        return Ok(s.parse::<u128>()?);
    }
    Err(anyhow!("Expected u128-compatible value"))
}

fn parse_bool(value: &Value) -> Result<bool> {
    if let Some(b) = value.as_bool() {
        return Ok(b);
    }
    if let Some(s) = value.as_str() {
        return Ok(matches!(s, "true" | "1"));
    }
    Err(anyhow!("Expected bool-compatible value"))
}

// normalize_address is imported from sui_resolver

fn exec_to_json(
    dispatcher: &ToolDispatcher,
    exec: &sui_sandbox_core::simulation::ExecutionResult,
) -> Value {
    let effects_json = exec.effects.as_ref().map(|effects| {
        json!({
            "created": effects.created.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
            "mutated": effects.mutated.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
            "deleted": effects.deleted.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
            "wrapped": effects.wrapped.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
            "unwrapped": effects.unwrapped.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
            "events": effects.events.iter().map(|e| {
                json!({
                    "type": e.type_tag,
                    "data_b64": encode_b64(&e.data),
                    "sequence": e.sequence,
                })
            }).collect::<Vec<_>>(),
            "object_changes": effects
                .object_changes
                .iter()
                .map(|change| object_change_to_json(dispatcher, change))
                .collect::<Vec<_>>(),
        })
    });

    json!({
        "success": exec.success,
        "effects": effects_json,
        "return_values": exec.effects.as_ref().map(|e| {
            e.return_values.iter().map(|vals| {
                vals.iter().map(|bytes| {
                    encode_b64(bytes)
                }).collect::<Vec<_>>()
            }).collect::<Vec<_>>()
        }),
        "gas_used": exec.effects.as_ref().map(|e| e.gas_used).unwrap_or(0),
        "error": exec.error.as_ref().map(|e| e.to_string()),
        "raw_error": exec.raw_error,
        "failed_command_index": exec.failed_command_index,
        "failed_command_description": exec.failed_command_description,
    })
}

fn object_change_to_json(dispatcher: &ToolDispatcher, change: &ptb::ObjectChange) -> Value {
    match change {
        ptb::ObjectChange::Created {
            id,
            owner,
            object_type,
        } => {
            let object_id = id.to_hex_literal();
            let object_ref = dispatcher.register_object_ref(&object_id);
            json!({
                "kind": "created",
                "object_id": object_id,
                "object_ref": object_ref,
                "owner": format!("{:?}", owner),
                "type": object_type.as_ref().map(format_type_tag),
            })
        }
        ptb::ObjectChange::Mutated {
            id,
            owner,
            object_type,
        } => {
            let object_id = id.to_hex_literal();
            let object_ref = dispatcher.register_object_ref(&object_id);
            json!({
                "kind": "mutated",
                "object_id": object_id,
                "object_ref": object_ref,
                "owner": format!("{:?}", owner),
                "type": object_type.as_ref().map(format_type_tag),
            })
        }
        ptb::ObjectChange::Deleted { id, object_type } => {
            let object_id = id.to_hex_literal();
            let object_ref = dispatcher.register_object_ref(&object_id);
            json!({
                "kind": "deleted",
                "object_id": object_id,
                "object_ref": object_ref,
                "type": object_type.as_ref().map(format_type_tag),
            })
        }
        ptb::ObjectChange::Wrapped { id, object_type } => {
            let object_id = id.to_hex_literal();
            let object_ref = dispatcher.register_object_ref(&object_id);
            json!({
                "kind": "wrapped",
                "object_id": object_id,
                "object_ref": object_ref,
                "type": object_type.as_ref().map(format_type_tag),
            })
        }
        ptb::ObjectChange::Unwrapped {
            id,
            owner,
            object_type,
        } => {
            let object_id = id.to_hex_literal();
            let object_ref = dispatcher.register_object_ref(&object_id);
            json!({
                "kind": "unwrapped",
                "object_id": object_id,
                "object_ref": object_ref,
                "owner": format!("{:?}", owner),
                "type": object_type.as_ref().map(format_type_tag),
            })
        }
        ptb::ObjectChange::Transferred {
            id,
            recipient,
            object_type,
            ..
        } => {
            let object_id = id.to_hex_literal();
            let object_ref = dispatcher.register_object_ref(&object_id);
            json!({
                "kind": "transferred",
                "object_id": object_id,
                "object_ref": object_ref,
                "recipient": recipient.to_hex_literal(),
                "type": object_type.as_ref().map(format_type_tag),
            })
        }
    }
}

fn capture_env_state(
    env: &mut SimulationEnvironment,
) -> (AccountAddress, sui_sandbox_core::vm::SimulationConfig) {
    (env.sender(), env.config().clone())
}

fn restore_env_state(
    env: &mut SimulationEnvironment,
    sender: AccountAddress,
    config: sui_sandbox_core::vm::SimulationConfig,
) {
    env.set_sender(sender);
    env.set_config(config);
}

fn apply_ptb_options(env: &mut SimulationEnvironment, options: Option<&PtbOptions>) {
    if let Some(opts) = options {
        if let Some(sender) = opts.sender.as_ref() {
            if let Ok(addr) = parse_address(sender) {
                env.set_sender(addr);
            }
        }
        if let Some(gas_price) = opts.gas_price {
            let mut config = env.config().clone();
            config = config
                .with_gas_price(gas_price)
                .with_reference_gas_price(gas_price);
            env.set_config(config);
        }
    }
}

fn load_package_into_env(env: &mut SimulationEnvironment, package: &PackageData) -> Result<()> {
    let mut linkage_map: BTreeMap<AccountAddress, (AccountAddress, u64)> = BTreeMap::new();
    for (runtime, storage) in &package.linkage {
        linkage_map.insert(*runtime, (*storage, 1));
    }
    env.register_package_with_linkage(
        package.address,
        package.version,
        package.original_id,
        package.modules.clone(),
        linkage_map,
    )?;
    Ok(())
}

fn fetch_dependency_closure_mcp(
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
                        if let Ok(bytes) = decode_b64(&bytes_b64) {
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

#[allow(clippy::too_many_arguments)]
fn replay_with_harness(
    env: &mut SimulationEnvironment,
    replay_state: &sui_state_fetcher::types::ReplayState,
    provider: std::sync::Arc<HistoricalStateProvider>,
    fetch_strategy: FetchStrategy,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    versions_str: &HashMap<String, u64>,
    cached_objects: &HashMap<String, String>,
    version_map: &HashMap<String, u64>,
    reconcile_policy: tx_replay::EffectsReconcilePolicy,
    self_heal_dynamic_fields: bool,
) -> Result<sui_sandbox_core::tx_replay::ReplayResult> {
    let synth_modules = if self_heal_dynamic_fields {
        let modules: Vec<CompiledModule> = env.resolver_mut().iter_modules().cloned().collect();
        if modules.is_empty() {
            None
        } else {
            Some(std::sync::Arc::new(modules))
        }
    } else {
        None
    };
    let config = env
        .config()
        .clone()
        .with_sender_address(replay_state.transaction.sender)
        .with_gas_budget(Some(replay_state.transaction.gas_budget))
        .with_gas_price(replay_state.transaction.gas_price);
    let config = match TransactionDigest::from_str(&replay_state.transaction.digest.0) {
        Ok(digest) => config.with_tx_hash(digest.into_inner()),
        Err(_) => config,
    };
    let mut harness =
        sui_sandbox_core::vm::VMHarness::with_config(env.resolver_mut(), false, config)?;
    harness.set_address_aliases_with_versions(aliases.clone(), versions_str.clone());

    let max_version = version_map.values().copied().max().unwrap_or(0);
    if fetch_strategy == FetchStrategy::Full {
        let provider_clone = provider.clone();
        let provider_clone_for_key = provider.clone();
        let checkpoint = replay_state.checkpoint;
        let synth_modules_for_fetcher = synth_modules.clone();
        let fetcher = move |_parent: AccountAddress, child_id: AccountAddress| {
            fetch_child_object_shared(provider_clone.as_ref(), child_id, checkpoint, max_version)
        };
        harness.set_versioned_child_fetcher(Box::new(fetcher));

        let alias_map = aliases.clone();
        let alias_map_for_fetcher = alias_map.clone();
        let child_id_aliases: std::sync::Arc<
            parking_lot::Mutex<HashMap<AccountAddress, AccountAddress>>,
        > = std::sync::Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let child_id_aliases_for_fetcher = child_id_aliases.clone();
        let debug_df = env_bool("SUI_DEBUG_DF_FETCH");
        let debug_df_full = env_bool("SUI_DEBUG_DF_FETCH_FULL");
        let miss_cache: std::sync::Arc<parking_lot::Mutex<HashMap<String, MissEntry>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let log_self_heal = matches!(
            std::env::var("SUI_SELF_HEAL_LOG")
                .ok()
                .as_deref()
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("1") | Some("true") | Some("yes") | Some("on")
        );
        let key_fetcher = move |parent: AccountAddress,
                                child_id: AccountAddress,
                                key_type: &TypeTag,
                                key_bytes: &[u8]| {
            let options = ChildFetchOptions {
                provider: provider_clone_for_key.as_ref(),
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
            fetch_child_object_by_key(&options, parent, child_id, key_type, key_bytes)
        };
        harness.set_key_based_child_fetcher(Box::new(key_fetcher));
        harness.set_child_id_aliases(child_id_aliases.clone());

        let resolver_cache: std::sync::Arc<Mutex<HashMap<String, TypeTag>>> =
            std::sync::Arc::new(Mutex::new(HashMap::new()));
        let provider_clone_for_resolver = provider.clone();
        let child_id_aliases_for_resolver = child_id_aliases.clone();
        let alias_map_for_resolver = alias_map;
        let resolver_checkpoint = replay_state.checkpoint;
        let key_type_resolver =
            move |parent: AccountAddress, key_bytes: &[u8]| -> Option<TypeTag> {
                let parent_hex = parent.to_hex_literal();
                let key_b64 = encode_b64(key_bytes);
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
                        .find_dynamic_field_by_bcs(&parent_hex, key_bytes, Some(cp), enum_limit)
                        .or_else(|_| {
                            gql.find_dynamic_field_by_bcs(&parent_hex, key_bytes, None, enum_limit)
                        }),
                    None => gql.find_dynamic_field_by_bcs(&parent_hex, key_bytes, None, enum_limit),
                };
                if let Ok(Some(df)) = field {
                    if let Ok(tag) = parse_type_tag(&df.name_type) {
                        if let Some(object_id) = df.object_id.as_deref() {
                            let mut candidate_tags = vec![tag.clone()];
                            let rewritten = rewrite_type_tag(tag.clone(), &alias_map_for_resolver);
                            if rewritten != tag {
                                candidate_tags.push(rewritten);
                            }
                            for candidate in candidate_tags {
                                if let Ok(type_bcs) = bcs::to_bytes(&candidate) {
                                    if let Some(computed_hex) =
                                        compute_dynamic_field_id(&parent_hex, key_bytes, &type_bcs)
                                    {
                                        if let (Ok(computed_id), Ok(actual_id)) = (
                                            AccountAddress::from_hex_literal(&computed_hex),
                                            AccountAddress::from_hex_literal(object_id),
                                        ) {
                                            if computed_id != actual_id {
                                                let mut map = child_id_aliases_for_resolver.lock();
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

    tx_replay::replay_with_version_tracking_with_policy(
        &replay_state.transaction,
        &mut harness,
        cached_objects,
        aliases,
        Some(version_map),
        reconcile_policy,
    )
}

fn synthesize_missing_inputs_for_replay(
    missing: &[tx_replay::MissingInputObject],
    cached_objects: &mut HashMap<String, String>,
    version_map: &mut HashMap<String, u64>,
    resolver: &LocalModuleResolver,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    provider: &std::sync::Arc<HistoricalStateProvider>,
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
            logs.push(format!(
                "missing_type object={} version={} (skipped)",
                object_id, version
            ));
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

        let encoded = encode_b64(&result.bytes);
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

fn b64_matches_bytes(encoded: &str, expected: &[u8]) -> bool {
    if let Ok(decoded) = decode_b64(encoded) {
        return decoded == expected;
    }
    if let Some(decoded) = decode_b64_no_pad_opt(encoded) {
        return decoded == expected;
    }
    false
}

#[derive(Debug, Clone)]
struct MissEntry {
    count: u32,
    last: std::time::Instant,
}

struct ChildFetchOptions<'a> {
    provider: &'a sui_state_fetcher::HistoricalStateProvider,
    checkpoint: Option<u64>,
    max_version: u64,
    aliases: &'a HashMap<AccountAddress, AccountAddress>,
    child_id_aliases:
        &'a std::sync::Arc<parking_lot::Mutex<HashMap<AccountAddress, AccountAddress>>>,
    miss_cache: Option<&'a std::sync::Arc<parking_lot::Mutex<HashMap<String, MissEntry>>>>,
    debug_df: bool,
    debug_df_full: bool,
    self_heal_dynamic_fields: bool,
    synth_modules: Option<std::sync::Arc<Vec<CompiledModule>>>,
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
                    if let Ok(bytes) = decode_b64(&bcs_b64) {
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
    let key_type_str = format_type_tag(key_type);
    let miss_key = miss_cache.map(|_| {
        let key_b64 = encode_b64(key_bytes);
        format!("{}:{}:{}:{}", parent_hex, child_hex, key_type_str, key_b64)
    });
    if let (Some(cache), Some(key)) = (miss_cache, miss_key.as_ref()) {
        if let Some(entry) = cache.lock().get(key).cloned() {
            let backoff_ms: u64 = env_var_or("SUI_DF_MISS_BACKOFF_MS", 250);
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

    let mut name_types = Vec::with_capacity(2);
    name_types.push(key_type_str);
    if !aliases.is_empty() {
        let rewritten = rewrite_type_tag(key_type.clone(), aliases);
        let alt = format_type_tag(&rewritten);
        if alt != name_types[0] {
            name_types.push(alt);
        }
        let mut reverse_aliases: HashMap<AccountAddress, AccountAddress> =
            HashMap::with_capacity(aliases.len());
        let mut reverse_aliases_all: HashMap<AccountAddress, Vec<AccountAddress>> =
            HashMap::with_capacity(aliases.len());
        for (storage, runtime) in aliases {
            reverse_aliases.insert(*runtime, *storage);
            reverse_aliases_all
                .entry(*runtime)
                .or_default()
                .push(*storage);
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
            let Some(computed_hex) = compute_dynamic_field_id(&parent_hex, key_bytes, &type_bcs)
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

    for name_type in &name_types {
        if let Ok(Some(df)) = gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_bytes) {
            if let Some(version) = df.version {
                if version > max_version {
                    continue;
                }
            }
            if let Some(object_id) = df.object_id.as_deref() {
                record_alias(child_id, object_id);
                if let Some(version) = df.version {
                    if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                        if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                            if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                                if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                        if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                            if let Ok(bytes) = decode_b64(&bcs_b64) {
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
            if let (Some(value_type), Some(value_bcs)) =
                (df.value_type.as_deref(), df.value_bcs.as_deref())
            {
                if let Ok(bytes) = decode_b64(value_bcs) {
                    if let Ok(tag) = parse_type_tag(value_type) {
                        if debug_df {
                            eprintln!(
                                "[df_fetch] by_name hit parent={} name_type={} child={} value_type={}",
                                parent_hex, name_type, child_hex, value_type
                            );
                        }
                        return Some((tag, bytes));
                    }
                }
            }
            if let Some(value_type) = df.value_type.as_deref() {
                if let Some(synth) = try_synthesize(value_type, df.object_id.as_deref(), "by_name")
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

    let enum_limit = std::env::var("SUI_DF_ENUM_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200);
    let key_b64 = encode_b64(key_bytes);
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
        if debug_df {
            eprintln!(
                "[df_fetch] enumerate parent={} name_type={} fields={}",
                parent_hex,
                name_type,
                fields.len()
            );
            let key_preview = if debug_df_full {
                key_b64.as_str()
            } else {
                key_b64.get(0..16).unwrap_or("<none>")
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
            if name_bcs != key_b64.as_str() && !b64_matches_bytes(name_bcs, key_bytes) {
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
                            if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                                if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                            if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                if let Ok(bytes) = decode_b64(value_bcs) {
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
                                if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                                    if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                                if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                if let (Some(value_type), Some(value_bcs)) =
                    (df.value_type.as_deref(), df.value_bcs.as_deref())
                {
                    if let Ok(bytes) = decode_b64(value_bcs) {
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
                                if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                                    if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                                if let Ok(bytes) = decode_b64(&bcs_b64) {
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
                    if let Ok(bytes) = decode_b64(value_bcs) {
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
                if let Ok(bytes) = decode_b64(&bcs_b64) {
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

fn resolve_project_file(project_root: &Path, file: &str) -> Result<PathBuf> {
    if std::path::Path::new(file).is_absolute() || file.contains("..") {
        return Err(anyhow!("File path must be relative to project root"));
    }
    let root = project_root.canonicalize()?;
    let candidate = root.join(file);
    if candidate.exists() {
        let canonical = candidate.canonicalize()?;
        if !canonical.starts_with(&root) {
            return Err(anyhow!("File path escapes project root"));
        }
        Ok(canonical)
    } else {
        if !candidate.starts_with(&root) {
            return Err(anyhow!("File path escapes project root"));
        }
        Ok(candidate)
    }
}

impl ToolDispatcher {
    async fn resolve_object_input(
        &self,
        spec: ObjectSpec,
        options: Option<PtbOptions>,
        auto_fetch: bool,
    ) -> Result<ObjectInput> {
        let object_id = if !spec.object_id.is_empty() {
            spec.object_id.clone()
        } else if let Some(reference) = spec.object_ref.as_ref() {
            self.resolve_object_ref(reference)
                .ok_or_else(|| anyhow!("Unknown object_ref: {}", reference))?
        } else {
            return Err(anyhow!("Object input missing object_id or object_ref"));
        };

        if auto_fetch {
            if let Some(cache_policy) = options.as_ref().and_then(|o| o.cache_policy) {
                ensure_object_loaded(self, &spec, Some(cache_policy)).await?;
            } else {
                ensure_object_loaded(self, &spec, None).await?;
            }
        }

        let mode = spec.mode.as_deref();
        let env_guard = self.env.lock();
        let mut obj = if let Some(version) = spec.version {
            env_guard.get_object_for_ptb_at_version(&normalize_address(&object_id), version, mode)
        } else {
            env_guard.get_object_for_ptb_with_mode(&normalize_address(&object_id), mode)
        }?;
        if let (Some(shared_mutable), ObjectInput::Shared { mutable, .. }) =
            (spec.shared_mutable, &mut obj)
        {
            *mutable = shared_mutable;
        }
        Ok(obj)
    }

    async fn prefetch_missing_objects(
        &self,
        inputs: &[ParsedInput],
        auto_fetch: bool,
        cache_policy: Option<CachePolicy>,
    ) -> Result<()> {
        if !auto_fetch {
            return Ok(());
        }
        for input in inputs {
            if let ParsedInput::Object(spec) = input {
                ensure_object_loaded(self, spec, cache_policy).await?;
            }
        }
        Ok(())
    }

    async fn ensure_packages_for_commands(
        &self,
        commands: &[PtbCommand],
        cache_policy: Option<CachePolicy>,
    ) -> Result<()> {
        let mut package_ids = Vec::new();
        for cmd in commands {
            if let PtbCommand::MoveCall {
                package, type_args, ..
            } = cmd
            {
                package_ids.push(*package);
                for tag in type_args {
                    package_ids.extend(extract_package_ids_from_type_tag(tag));
                }
            }
        }
        if package_ids.is_empty() {
            return Ok(());
        }
        let mut unique = package_ids;
        unique.sort();
        unique.dedup();
        let provider = self.provider().await?;
        let bypass_cache = cache_policy
            .map(|policy| policy.is_bypass())
            .unwrap_or(false);
        let packages = if bypass_cache {
            provider
                .fetch_packages_with_deps_no_cache(&unique, None, None)
                .await?
        } else {
            provider
                .fetch_packages_with_deps(&unique, None, None)
                .await?
        };
        let mut env_guard = self.env.lock();
        for pkg in packages.values() {
            let _ = load_package_into_env(&mut env_guard, pkg);
        }
        drop(env_guard);
        if !bypass_cache {
            let _ = provider.flush_cache();
        }
        Ok(())
    }

    async fn fetch_object_to_env(
        &self,
        object_id: &str,
        version: Option<u64>,
        cache_policy: Option<CachePolicy>,
    ) -> Result<Value> {
        let provider = self.provider().await?;
        let normalized = normalize_address(object_id);

        let bypass_cache = cache_policy
            .map(|policy| policy.is_bypass())
            .unwrap_or(false);
        let obj = if let Some(version) = version {
            let mut fetched = if bypass_cache {
                provider
                    .fetch_objects_versioned_no_cache(&[(
                        AccountAddress::from_hex_literal(&normalized)?,
                        version,
                    )])
                    .await?
            } else {
                provider
                    .fetch_objects_versioned(&[(
                        AccountAddress::from_hex_literal(&normalized)?,
                        version,
                    )])
                    .await?
            };
            fetched
                .remove(&AccountAddress::from_hex_literal(&normalized)?)
                .ok_or_else(|| anyhow!("Object not found"))?
        } else {
            let grpc_obj = provider.grpc().get_object(&normalized).await?;
            let Some(grpc_obj) = grpc_obj else {
                return Err(anyhow!("Object not found"));
            };
            grpc_to_versioned(grpc_obj)?
        };

        if !bypass_cache {
            provider.cache().put_object(obj.clone());
            let _ = provider.flush_cache();
        }

        let (is_shared, is_immutable) = (obj.is_shared, obj.is_immutable);
        let type_string = obj.type_tag.clone();

        let mut env_guard = self.env.lock();
        env_guard.load_object_from_data(
            &normalized,
            obj.bcs_bytes.clone(),
            type_string.as_deref(),
            is_shared,
            is_immutable,
            obj.version,
        )?;
        drop(env_guard);

        let object_ref = self.register_object_ref(&normalized);
        let mut response = json!({
            "object_id": normalized,
            "object_ref": object_ref,
            "type": type_string,
            "version": obj.version,
            "is_shared": is_shared,
            "is_immutable": is_immutable,
        });
        if let Some(policy) = cache_policy {
            response["cache_policy"] = json!(policy.as_str());
        }
        Ok(response)
    }

    async fn fetch_package_to_env(
        &self,
        package_id: &str,
        version: Option<u64>,
        cache_policy: Option<CachePolicy>,
    ) -> Result<Value> {
        let provider = self.provider().await?;
        let addr = parse_address(package_id)?;
        let version_map = version.map(|v| {
            let mut map = HashMap::new();
            map.insert(addr, v);
            map
        });
        let bypass_cache = cache_policy
            .map(|policy| policy.is_bypass())
            .unwrap_or(false);
        let packages = if bypass_cache {
            provider
                .fetch_packages_with_deps_no_cache(&[addr], version_map.as_ref(), None)
                .await?
        } else {
            provider
                .fetch_packages_with_deps(&[addr], version_map.as_ref(), None)
                .await?
        };
        let mut env_guard = self.env.lock();
        for pkg in packages.values() {
            let _ = load_package_into_env(&mut env_guard, pkg);
        }
        drop(env_guard);

        let list = packages
            .values()
            .map(|pkg| {
                let package_id = pkg.address.to_hex_literal();
                let package_ref = self.register_object_ref(&package_id);
                json!({
                    "package_id": package_id,
                    "package_ref": package_ref,
                    "version": pkg.version,
                    "original_id": pkg.original_id.map(|id| id.to_hex_literal()),
                    "modules": pkg.modules.iter().map(|(name, _)| name).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "packages": list }))
    }
}

fn grpc_to_versioned(grpc_obj: GrpcObject) -> Result<VersionedObject> {
    let (is_shared, is_immutable) = match grpc_obj.owner {
        GrpcOwner::Shared { .. } => (true, false),
        GrpcOwner::Immutable => (false, true),
        _ => (false, false),
    };
    let bcs_bytes = grpc_obj
        .bcs
        .ok_or_else(|| anyhow!("Object missing bcs bytes"))?;
    Ok(VersionedObject {
        id: parse_address(&grpc_obj.object_id)?,
        version: grpc_obj.version,
        digest: Some(grpc_obj.digest),
        type_tag: grpc_obj.type_string,
        bcs_bytes,
        is_shared,
        is_immutable,
    })
}
