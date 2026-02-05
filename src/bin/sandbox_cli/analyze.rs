//! Analyze command - package and replay introspection

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use move_binary_format::CompiledModule;
use serde::Serialize;
use std::path::PathBuf;

use super::network::resolve_graphql_endpoint;
use super::replay::ReplaySource;
use super::SandboxState;
use sui_package_extractor::bytecode::{
    build_bytecode_interface_value_from_compiled_modules, extract_sanity_counts,
    read_local_compiled_modules,
};
use sui_state_fetcher::{HistoricalStateProvider, ReplayStateConfig};
use sui_transport::graphql::GraphQLClient;

#[derive(Parser, Debug)]
pub struct AnalyzeCmd {
    #[command(subcommand)]
    command: AnalyzeCommand,
}

#[derive(Subcommand, Debug)]
enum AnalyzeCommand {
    /// Analyze a package by id or local bytecode directory
    Package(AnalyzePackageCmd),
    /// Analyze replay state hydration for a transaction digest
    Replay(AnalyzeReplayCmd),
}

#[derive(Parser, Debug)]
pub struct AnalyzePackageCmd {
    /// Package id (0x...)
    #[arg(long, value_name = "ID", conflicts_with = "bytecode_dir")]
    pub package_id: Option<String>,

    /// Local package directory containing bytecode_modules/*.mv
    #[arg(long, value_name = "DIR", conflicts_with = "package_id")]
    pub bytecode_dir: Option<PathBuf>,

    /// Include module names in output
    #[arg(long, default_value_t = false)]
    pub list_modules: bool,

    /// Attempt MM2 model build for the package
    #[arg(long, default_value_t = false)]
    pub mm2: bool,
}

#[derive(Parser, Debug)]
pub struct AnalyzeReplayCmd {
    /// Transaction digest
    pub digest: String,

    /// Data source for replay hydration
    #[arg(long, value_enum, default_value = "hybrid")]
    pub source: ReplaySource,

    /// Allow fallback to secondary sources when data is missing
    #[arg(long, default_value_t = true)]
    pub fallback: bool,

    /// Prefetch depth for dynamic fields (default: 3)
    #[arg(long, default_value_t = 3)]
    pub prefetch_depth: usize,

    /// Prefetch limit for dynamic fields (default: 200)
    #[arg(long, default_value_t = 200)]
    pub prefetch_limit: usize,

    /// Disable dynamic field prefetch
    #[arg(long, default_value_t = false)]
    pub no_prefetch: bool,

    /// Auto-inject system objects (Clock/Random) when missing
    #[arg(long, default_value_t = true)]
    pub auto_system_objects: bool,

    /// Attempt MM2 model build across replay packages
    #[arg(long, default_value_t = false)]
    pub mm2: bool,
}

#[derive(Debug, Serialize)]
struct AnalyzePackageOutput {
    pub source: String,
    pub package_id: String,
    pub modules: usize,
    pub structs: usize,
    pub functions: usize,
    pub key_structs: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mm2_model_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mm2_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct AnalyzeReplayOutput {
    pub digest: String,
    pub sender: String,
    pub commands: usize,
    pub inputs: usize,
    pub objects: usize,
    pub packages: usize,
    pub modules: usize,
    pub input_summary: ReplayInputSummary,
    pub command_summaries: Vec<ReplayCommandSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_objects: Option<Vec<ReplayInputObject>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_types: Option<Vec<ReplayObjectType>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_inputs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_packages: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub suggestions: Vec<String>,
    pub epoch: u64,
    pub protocol_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_gas_price: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mm2_model_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mm2_error: Option<String>,
}

#[derive(Debug, Serialize, Default)]
struct ReplayInputSummary {
    pub total: usize,
    pub pure: usize,
    pub owned: usize,
    pub shared_mutable: usize,
    pub shared_immutable: usize,
    pub immutable: usize,
    pub receiving: usize,
}

#[derive(Debug, Serialize)]
struct ReplayCommandSummary {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub type_args: usize,
    pub args: usize,
}

#[derive(Debug, Serialize)]
struct ReplayInputObject {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutable: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ReplayObjectType {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_tag: Option<String>,
    pub version: u64,
    pub shared: bool,
    pub immutable: bool,
}

impl AnalyzeCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        match &self.command {
            AnalyzeCommand::Package(cmd) => {
                let output = cmd.execute(state, verbose).await?;
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    print_package_output(&output);
                }
                Ok(())
            }
            AnalyzeCommand::Replay(cmd) => {
                let output = cmd.execute(state, verbose).await?;
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    print_replay_output(&output);
                }
                Ok(())
            }
        }
    }
}

impl AnalyzePackageCmd {
    async fn execute(&self, state: &SandboxState, verbose: bool) -> Result<AnalyzePackageOutput> {
        let (package_id, modules, module_names, source) = if let Some(dir) = &self.bytecode_dir {
            let compiled = read_local_compiled_modules(dir)?;
            let pkg_id = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("local")
                .to_string();
            let (module_names, interface_value) =
                build_bytecode_interface_value_from_compiled_modules(&pkg_id, &compiled)?;
            let counts = extract_sanity_counts(
                interface_value
                    .get("modules")
                    .unwrap_or(&serde_json::Value::Null),
            );
            let (mm2_ok, mm2_err) = build_mm2_summary(self.mm2, compiled, verbose);
            return Ok(AnalyzePackageOutput {
                source: "local-bytecode".to_string(),
                package_id: pkg_id,
                modules: counts.modules,
                structs: counts.structs,
                functions: counts.functions,
                key_structs: counts.key_structs,
                module_names: if self.list_modules {
                    Some(module_names)
                } else {
                    None
                },
                mm2_model_ok: mm2_ok,
                mm2_error: mm2_err,
            });
        } else if let Some(pkg_id) = &self.package_id {
            let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
            let graphql = GraphQLClient::new(&graphql_endpoint);
            let pkg = graphql
                .fetch_package(pkg_id)
                .with_context(|| format!("fetch package {}", pkg_id))?;
            let mut compiled_modules = Vec::with_capacity(pkg.modules.len());
            let mut names = Vec::with_capacity(pkg.modules.len());
            for module in pkg.modules {
                names.push(module.name.clone());
                let Some(b64) = module.bytecode_base64 else {
                    continue;
                };
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .context("decode module bytecode")?;
                let compiled = CompiledModule::deserialize_with_defaults(&bytes)
                    .context("deserialize module")?;
                compiled_modules.push(compiled);
            }
            names.sort();
            (
                pkg.address,
                compiled_modules,
                if self.list_modules { Some(names) } else { None },
                "graphql".to_string(),
            )
        } else {
            return Err(anyhow!("--package-id or --bytecode-dir is required"));
        };

        let (mm2_ok, mm2_err) = build_mm2_summary(self.mm2, modules.clone(), verbose);
        let counts = {
            let (_, interface_value) =
                build_bytecode_interface_value_from_compiled_modules(&package_id, &modules)?;
            extract_sanity_counts(
                interface_value
                    .get("modules")
                    .unwrap_or(&serde_json::Value::Null),
            )
        };

        Ok(AnalyzePackageOutput {
            source,
            package_id,
            modules: counts.modules,
            structs: counts.structs,
            functions: counts.functions,
            key_structs: counts.key_structs,
            module_names,
            mm2_model_ok: mm2_ok,
            mm2_error: mm2_err,
        })
    }
}

impl AnalyzeReplayCmd {
    async fn execute(&self, state: &SandboxState, verbose: bool) -> Result<AnalyzeReplayOutput> {
        if matches!(self.source, ReplaySource::Walrus | ReplaySource::Hybrid)
            && !cfg!(feature = "walrus")
        {
            return Err(anyhow!("Walrus source requires the `walrus` feature"));
        }
        if self.mm2 && !cfg!(feature = "mm2") {
            return Err(anyhow!("MM2 analysis requires the `mm2` feature"));
        }

        if matches!(self.source, ReplaySource::Walrus) && !self.fallback {
            std::env::set_var("SUI_WALRUS_PACKAGE_ONLY", "1");
        }

        let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
        let grpc = sui_transport::grpc::GrpcClient::with_api_key(
            &state.rpc_url,
            std::env::var("SUI_GRPC_API_KEY").ok(),
        )
        .await?;
        let graphql = GraphQLClient::new(&graphql_endpoint);

        let mut provider = HistoricalStateProvider::with_clients(grpc, graphql);
        if matches!(self.source, ReplaySource::Walrus | ReplaySource::Hybrid) {
            provider = provider
                .with_walrus_from_env()
                .with_local_object_store_from_env();
        }

        let config = ReplayStateConfig {
            prefetch_dynamic_fields: !self.no_prefetch,
            df_depth: self.prefetch_depth,
            df_limit: self.prefetch_limit,
            auto_system_objects: self.auto_system_objects,
        };
        let replay_state = provider
            .replay_state_builder()
            .with_config(config)
            .build(&self.digest)
            .await?;

        let (input_summary, input_objects) =
            summarize_inputs(&replay_state.transaction.inputs, verbose);
        let command_summaries = summarize_commands(&replay_state.transaction.commands);

        let mut modules_total = 0usize;
        for pkg in replay_state.packages.values() {
            modules_total += pkg.modules.len();
        }
        let package_ids = if verbose {
            Some(
                replay_state
                    .packages
                    .keys()
                    .map(|id| id.to_hex_literal())
                    .collect(),
            )
        } else {
            None
        };
        let object_ids = if verbose {
            Some(
                replay_state
                    .objects
                    .keys()
                    .map(|id| id.to_hex_literal())
                    .collect(),
            )
        } else {
            None
        };

        let object_types = if verbose {
            Some(
                replay_state
                    .objects
                    .values()
                    .map(|obj| ReplayObjectType {
                        id: obj.id.to_hex_literal(),
                        type_tag: obj.type_tag.clone(),
                        version: obj.version,
                        shared: obj.is_shared,
                        immutable: obj.is_immutable,
                    })
                    .collect(),
            )
        } else {
            None
        };

        let (missing_inputs, missing_packages, suggestions) = build_readiness_notes(
            &replay_state,
            &input_summary,
            self.source,
            self.fallback,
            self.no_prefetch,
            self.mm2,
        );

        let (mm2_ok, mm2_err) = if self.mm2 {
            let modules: Vec<CompiledModule> = replay_state
                .packages
                .values()
                .flat_map(|pkg| {
                    pkg.modules.iter().filter_map(|(_, bytes)| {
                        CompiledModule::deserialize_with_defaults(bytes).ok()
                    })
                })
                .collect();
            build_mm2_summary(true, modules, verbose)
        } else {
            (None, None)
        };

        Ok(AnalyzeReplayOutput {
            digest: replay_state.transaction.digest.0.clone(),
            sender: replay_state.transaction.sender.to_hex_literal(),
            commands: replay_state.transaction.commands.len(),
            inputs: replay_state.transaction.inputs.len(),
            objects: replay_state.objects.len(),
            packages: replay_state.packages.len(),
            modules: modules_total,
            input_summary,
            command_summaries,
            input_objects,
            object_types,
            missing_inputs,
            missing_packages,
            suggestions,
            epoch: replay_state.epoch,
            protocol_version: replay_state.protocol_version,
            checkpoint: replay_state.checkpoint,
            reference_gas_price: replay_state.reference_gas_price,
            package_ids,
            object_ids,
            mm2_model_ok: mm2_ok,
            mm2_error: mm2_err,
        })
    }
}

fn build_mm2_summary(
    enabled: bool,
    modules: Vec<CompiledModule>,
    verbose: bool,
) -> (Option<bool>, Option<String>) {
    if !enabled {
        return (None, None);
    }
    #[cfg(feature = "mm2")]
    {
        match sui_sandbox_core::mm2::TypeModel::from_modules(modules) {
            Ok(_) => (Some(true), None),
            Err(err) => {
                if verbose {
                    eprintln!("[mm2] type model build failed: {}", err);
                }
                (Some(false), Some(err.to_string()))
            }
        }
    }
    #[cfg(not(feature = "mm2"))]
    {
        let _ = modules;
        (Some(false), Some("mm2 feature disabled".to_string()))
    }
}

fn summarize_inputs(
    inputs: &[sui_sandbox_types::TransactionInput],
    verbose: bool,
) -> (ReplayInputSummary, Option<Vec<ReplayInputObject>>) {
    let mut summary = ReplayInputSummary {
        total: inputs.len(),
        ..Default::default()
    };
    let mut objects = Vec::new();

    for input in inputs {
        match input {
            sui_sandbox_types::TransactionInput::Pure { .. } => summary.pure += 1,
            sui_sandbox_types::TransactionInput::Object { object_id, .. } => {
                summary.owned += 1;
                if verbose {
                    objects.push(ReplayInputObject {
                        id: object_id.clone(),
                        kind: "owned".to_string(),
                        mutable: None,
                    });
                }
            }
            sui_sandbox_types::TransactionInput::SharedObject {
                object_id, mutable, ..
            } => {
                if *mutable {
                    summary.shared_mutable += 1;
                } else {
                    summary.shared_immutable += 1;
                }
                if verbose {
                    objects.push(ReplayInputObject {
                        id: object_id.clone(),
                        kind: "shared".to_string(),
                        mutable: Some(*mutable),
                    });
                }
            }
            sui_sandbox_types::TransactionInput::ImmutableObject { object_id, .. } => {
                summary.immutable += 1;
                if verbose {
                    objects.push(ReplayInputObject {
                        id: object_id.clone(),
                        kind: "immutable".to_string(),
                        mutable: None,
                    });
                }
            }
            sui_sandbox_types::TransactionInput::Receiving { object_id, .. } => {
                summary.receiving += 1;
                if verbose {
                    objects.push(ReplayInputObject {
                        id: object_id.clone(),
                        kind: "receiving".to_string(),
                        mutable: None,
                    });
                }
            }
        }
    }

    let objects = if verbose { Some(objects) } else { None };
    (summary, objects)
}

fn summarize_commands(commands: &[sui_sandbox_types::PtbCommand]) -> Vec<ReplayCommandSummary> {
    commands
        .iter()
        .map(|cmd| match cmd {
            sui_sandbox_types::PtbCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => ReplayCommandSummary {
                kind: "MoveCall".to_string(),
                target: Some(format!("{}::{}::{}", package, module, function)),
                type_args: type_arguments.len(),
                args: arguments.len(),
            },
            sui_sandbox_types::PtbCommand::SplitCoins { amounts, .. } => ReplayCommandSummary {
                kind: "SplitCoins".to_string(),
                target: None,
                type_args: 0,
                args: 1 + amounts.len(),
            },
            sui_sandbox_types::PtbCommand::MergeCoins { sources, .. } => ReplayCommandSummary {
                kind: "MergeCoins".to_string(),
                target: None,
                type_args: 0,
                args: 1 + sources.len(),
            },
            sui_sandbox_types::PtbCommand::TransferObjects { objects, .. } => {
                ReplayCommandSummary {
                    kind: "TransferObjects".to_string(),
                    target: None,
                    type_args: 0,
                    args: 1 + objects.len(),
                }
            }
            sui_sandbox_types::PtbCommand::MakeMoveVec { elements, type_arg } => {
                ReplayCommandSummary {
                    kind: "MakeMoveVec".to_string(),
                    target: None,
                    type_args: usize::from(type_arg.is_some()),
                    args: elements.len(),
                }
            }
            sui_sandbox_types::PtbCommand::Publish { dependencies, .. } => ReplayCommandSummary {
                kind: "Publish".to_string(),
                target: None,
                type_args: 0,
                args: dependencies.len(),
            },
            sui_sandbox_types::PtbCommand::Upgrade { package, .. } => ReplayCommandSummary {
                kind: "Upgrade".to_string(),
                target: Some(package.clone()),
                type_args: 0,
                args: 1,
            },
        })
        .collect()
}

fn build_readiness_notes(
    replay_state: &sui_state_fetcher::ReplayState,
    input_summary: &ReplayInputSummary,
    source: ReplaySource,
    fallback: bool,
    no_prefetch: bool,
    mm2_enabled: bool,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    use move_core_types::account_address::AccountAddress;
    use std::collections::BTreeSet;

    let mut missing_inputs = Vec::new();
    for input in &replay_state.transaction.inputs {
        let id = match input {
            sui_sandbox_types::TransactionInput::Object { object_id, .. } => Some(object_id),
            sui_sandbox_types::TransactionInput::SharedObject { object_id, .. } => Some(object_id),
            sui_sandbox_types::TransactionInput::ImmutableObject { object_id, .. } => {
                Some(object_id)
            }
            sui_sandbox_types::TransactionInput::Receiving { object_id, .. } => Some(object_id),
            sui_sandbox_types::TransactionInput::Pure { .. } => None,
        };
        if let Some(id) = id {
            match AccountAddress::from_hex_literal(id) {
                Ok(addr) => {
                    if !replay_state.objects.contains_key(&addr) {
                        missing_inputs.push(addr.to_hex_literal());
                    }
                }
                Err(_) => missing_inputs.push(id.clone()),
            }
        }
    }

    let mut required_packages: BTreeSet<AccountAddress> = BTreeSet::new();
    for cmd in &replay_state.transaction.commands {
        match cmd {
            sui_sandbox_types::PtbCommand::MoveCall {
                package,
                type_arguments,
                ..
            } => {
                if let Ok(addr) = AccountAddress::from_hex_literal(package) {
                    required_packages.insert(addr);
                }
                for ty in type_arguments {
                    for pkg in sui_sandbox_core::utilities::extract_package_ids_from_type(ty) {
                        if let Ok(addr) = AccountAddress::from_hex_literal(&pkg) {
                            required_packages.insert(addr);
                        }
                    }
                }
            }
            sui_sandbox_types::PtbCommand::Upgrade { package, .. } => {
                if let Ok(addr) = AccountAddress::from_hex_literal(package) {
                    required_packages.insert(addr);
                }
            }
            sui_sandbox_types::PtbCommand::Publish { dependencies, .. } => {
                for dep in dependencies {
                    if let Ok(addr) = AccountAddress::from_hex_literal(dep) {
                        required_packages.insert(addr);
                    }
                }
            }
            _ => {}
        }
    }

    let mut missing_packages = Vec::new();
    for addr in required_packages {
        if !replay_state.packages.contains_key(&addr) {
            missing_packages.push(addr.to_hex_literal());
        }
    }

    let mut suggestions = Vec::new();
    if !missing_inputs.is_empty() {
        suggestions.push(format!(
            "Missing {} input object(s); try replay with --synthesize or ensure historical gRPC/Walrus access",
            missing_inputs.len()
        ));
    }
    if !missing_packages.is_empty() {
        suggestions.push(format!(
            "Missing {} package(s); try `sui-sandbox fetch package <ID> --with-deps`",
            missing_packages.len()
        ));
    }
    if input_summary.shared_mutable + input_summary.shared_immutable > 0 {
        suggestions.push(
            "Shared inputs detected; ensure you have historical versions (Walrus or archive gRPC)"
                .to_string(),
        );
    }
    if no_prefetch {
        suggestions.push(
            "Dynamic-field prefetch is disabled; enable prefetch for more complete replay data"
                .to_string(),
        );
    }
    if !fallback {
        suggestions.push(
            "Fallback disabled; enable fallback to allow GraphQL current-state fills when historical data is missing".to_string(),
        );
    }
    if !mm2_enabled {
        suggestions
            .push("Run `analyze replay --mm2` for synthesis + type-model diagnostics".to_string());
    }
    if matches!(source, ReplaySource::Grpc) {
        suggestions.push("If data is incomplete, try `--source walrus` when available".to_string());
    }

    (missing_inputs, missing_packages, suggestions)
}

fn print_package_output(output: &AnalyzePackageOutput) {
    println!("Package Analysis: {}", output.package_id);
    println!("  Source:   {}", output.source);
    println!(
        "  Counts:   modules={} structs={} functions={} key_structs={}",
        output.modules, output.structs, output.functions, output.key_structs
    );
    if let Some(names) = output.module_names.as_ref() {
        println!("  Modules:  {}", names.join(", "));
    }
    if let Some(ok) = output.mm2_model_ok {
        println!("  MM2:      {}", if ok { "ok" } else { "failed" });
    }
    if let Some(err) = output.mm2_error.as_ref() {
        println!("  MM2 Err:  {}", err);
    }
}

fn print_replay_output(output: &AnalyzeReplayOutput) {
    println!("Replay Analysis: {}", output.digest);
    println!("  Sender:   {}", output.sender);
    println!(
        "  Tx:       commands={} inputs={}",
        output.commands, output.inputs
    );
    println!(
        "  State:    objects={} packages={} modules={}",
        output.objects, output.packages, output.modules
    );
    println!(
        "  Inputs:   total={} pure={} owned={} shared(mutable/imm)={}/{} immutable={} receiving={}",
        output.input_summary.total,
        output.input_summary.pure,
        output.input_summary.owned,
        output.input_summary.shared_mutable,
        output.input_summary.shared_immutable,
        output.input_summary.immutable,
        output.input_summary.receiving
    );
    if !output.command_summaries.is_empty() {
        println!("  Commands:");
        for (idx, cmd) in output.command_summaries.iter().enumerate() {
            if let Some(target) = cmd.target.as_ref() {
                println!(
                    "    [{}] {} {} (type_args={}, args={})",
                    idx, cmd.kind, target, cmd.type_args, cmd.args
                );
            } else {
                println!(
                    "    [{}] {} (type_args={}, args={})",
                    idx, cmd.kind, cmd.type_args, cmd.args
                );
            }
        }
    }
    println!(
        "  Epoch:    {} (protocol v{})",
        output.epoch, output.protocol_version
    );
    if let Some(cp) = output.checkpoint {
        println!("  Checkpoint: {}", cp);
    }
    if let Some(rgp) = output.reference_gas_price {
        println!("  RGP:      {}", rgp);
    }
    if !output.missing_inputs.is_empty() {
        println!("  Missing inputs: {}", output.missing_inputs.join(", "));
    }
    if !output.missing_packages.is_empty() {
        println!("  Missing packages: {}", output.missing_packages.join(", "));
    }
    if let Some(objs) = output.input_objects.as_ref() {
        println!("  Input objects:");
        for obj in objs {
            match obj.mutable {
                Some(mutable) => println!("    {} ({}, mutable={})", obj.id, obj.kind, mutable),
                None => println!("    {} ({})", obj.id, obj.kind),
            }
        }
    }
    if let Some(types) = output.object_types.as_ref() {
        println!("  Object types:");
        for obj in types {
            if let Some(tag) = obj.type_tag.as_ref() {
                println!(
                    "    {} v{} {} (shared={}, immutable={})",
                    obj.id, obj.version, tag, obj.shared, obj.immutable
                );
            } else {
                println!(
                    "    {} v{} <unknown> (shared={}, immutable={})",
                    obj.id, obj.version, obj.shared, obj.immutable
                );
            }
        }
    }
    if let Some(ids) = output.package_ids.as_ref() {
        println!("  Packages: {}", ids.join(", "));
    }
    if let Some(ids) = output.object_ids.as_ref() {
        println!("  Objects:  {}", ids.join(", "));
    }
    if let Some(ok) = output.mm2_model_ok {
        println!("  MM2:      {}", if ok { "ok" } else { "failed" });
    }
    if let Some(err) = output.mm2_error.as_ref() {
        println!("  MM2 Err:  {}", err);
    }
    if !output.suggestions.is_empty() {
        println!("  Suggestions:");
        for suggestion in &output.suggestions {
            println!("    - {}", suggestion);
        }
    }
}
