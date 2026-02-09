use anyhow::{anyhow, Context, Result};
use clap::ValueEnum;
use move_binary_format::CompiledModule;

use super::mm2_common::build_mm2_summary;
use super::{
    AnalyzeReplayCmd, AnalyzeReplayHydrationSummary, AnalyzeReplayOutput, ReplayCommandSummary,
    ReplayInputObject, ReplayInputSummary, ReplayObjectType,
};
use crate::sandbox_cli::replay::hydration::{
    build_historical_state_provider, build_replay_state, ReplayHydrationConfig,
};
use crate::sandbox_cli::replay::ReplaySource;
use crate::sandbox_cli::SandboxState;
use sui_state_fetcher::checkpoint_to_replay_state;

impl AnalyzeReplayCmd {
    pub(super) async fn execute(
        &self,
        state: &SandboxState,
        verbose: bool,
    ) -> Result<AnalyzeReplayOutput> {
        if matches!(
            self.hydration.source,
            ReplaySource::Walrus | ReplaySource::Hybrid
        ) && !cfg!(feature = "walrus")
        {
            return Err(anyhow!("Walrus source requires the `walrus` feature"));
        }
        if self.mm2 && !cfg!(feature = "mm2") {
            return Err(anyhow!("MM2 analysis requires the `mm2` feature"));
        }

        // Walrus-first path: --checkpoint provided, skip gRPC entirely
        #[cfg(feature = "walrus")]
        if let Some(checkpoint_num) = self.checkpoint {
            use sui_transport::walrus::WalrusClient;

            if verbose {
                eprintln!(
                    "[walrus] fetching checkpoint {} for digest {}",
                    checkpoint_num, self.digest
                );
            }
            let digest_clone = self.digest.clone();
            let checkpoint_data = tokio::task::spawn_blocking(move || {
                let walrus = WalrusClient::mainnet();
                walrus.get_checkpoint(checkpoint_num)
            })
            .await
            .context("Walrus fetch task panicked")?
            .context("Failed to fetch checkpoint from Walrus")?;

            let replay_state = checkpoint_to_replay_state(&checkpoint_data, &digest_clone)
                .context("Failed to convert checkpoint to replay state")?;

            return self.build_output(&replay_state, verbose);
        }

        #[cfg(not(feature = "walrus"))]
        if self.checkpoint.is_some() {
            return Err(anyhow!(
                "--checkpoint requires the `walrus` feature to be enabled"
            ));
        }

        let provider = build_historical_state_provider(
            state,
            self.hydration.source,
            self.hydration.allow_fallback,
            verbose,
        )
        .await?;
        let replay_state = build_replay_state(
            provider.as_ref(),
            &self.digest,
            ReplayHydrationConfig {
                prefetch_dynamic_fields: !self.hydration.no_prefetch,
                prefetch_depth: self.hydration.prefetch_depth,
                prefetch_limit: self.hydration.prefetch_limit,
                auto_system_objects: self.hydration.auto_system_objects,
            },
        )
        .await?;

        self.build_output(&replay_state, verbose)
    }

    fn build_output(
        &self,
        replay_state: &sui_state_fetcher::ReplayState,
        verbose: bool,
    ) -> Result<AnalyzeReplayOutput> {
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
            replay_state,
            &input_summary,
            self.hydration.source,
            self.hydration.allow_fallback,
            self.hydration.no_prefetch,
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
        let source = self
            .hydration
            .source
            .to_possible_value()
            .map_or_else(|| "unknown".to_string(), |v| v.get_name().to_string());

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
            hydration: AnalyzeReplayHydrationSummary {
                source,
                allow_fallback: self.hydration.allow_fallback,
                auto_system_objects: self.hydration.auto_system_objects,
                dynamic_field_prefetch: !self.hydration.no_prefetch,
                prefetch_depth: self.hydration.prefetch_depth,
                prefetch_limit: self.hydration.prefetch_limit,
            },
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
            "Fallback disabled; pass --allow-fallback to permit secondary data sources when historical data is missing".to_string(),
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

pub(super) fn print_replay_output(output: &AnalyzeReplayOutput) {
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
        "  Hydration: source={} allow_fallback={} auto_system_objects={} prefetch={} depth={} limit={}",
        output.hydration.source,
        output.hydration.allow_fallback,
        output.hydration.auto_system_objects,
        output.hydration.dynamic_field_prefetch,
        output.hydration.prefetch_depth,
        output.hydration.prefetch_limit
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
