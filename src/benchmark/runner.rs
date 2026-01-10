use anyhow::{anyhow, Context, Result};
use move_binary_format::file_format::{CompiledModule, SignatureToken, Visibility};
use move_core_types::account_address::AccountAddress;
use move_core_types::annotated_value::{MoveTypeLayout, MoveValue};
use move_core_types::identifier::Identifier;
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Write};

use crate::args::BenchmarkLocalArgs;
use crate::benchmark::resolver::LocalModuleResolver;
use crate::benchmark::validator::Validator;
use crate::benchmark::vm::VMHarness;
use crate::bytecode::{compiled_module_name, module_self_address_hex};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    TierAHit,
    TierBHit,
    Miss,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum FailureStage {
    A1,
    A2,
    A3,
    A4,
    A5,
    B1,
    B2,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkReport {
    pub target_package: String,
    pub target_module: String,
    pub target_function: String,
    pub status: AttemptStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<FailureStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_a_details: Option<TierADetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_b_details: Option<TierBDetails>,
}

#[derive(Debug, Serialize)]
pub struct TierADetails {
    pub resolved_params: Vec<String>,
    pub bcs_roundtrip_verified: bool,
    pub has_object_params: bool,
}

#[derive(Debug, Serialize)]
pub struct TierBDetails {
    pub execution_success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn run_benchmark(args: &BenchmarkLocalArgs) -> Result<()> {
    let mut resolver = LocalModuleResolver::new();
    eprintln!("Loading corpus from {}...", args.target_corpus.display());
    let count = resolver.load_from_dir(&args.target_corpus)?;
    eprintln!("Loaded {} modules.", count);

    let validator = Validator::new(&resolver);

    let output_file = File::create(&args.output)
        .with_context(|| format!("create output file {}", args.output.display()))?;
    let mut writer = BufWriter::new(output_file);

    for module in resolver.iter_modules() {
        let _package_addr = module_self_address_hex(module);
        let module_name = compiled_module_name(module);

        for def in module.function_defs() {
            let handle = module.function_handle_at(def.function);
            let func_name = module.identifier_at(handle.name).to_string();

            let is_public = matches!(def.visibility, Visibility::Public);
            let is_entry = def.is_entry;

            if !is_public && !is_entry {
                continue;
            }

            let addr = *module.self_id().address();
            let report = attempt_function(
                args,
                &validator,
                &resolver,
                addr,
                module,
                &module_name,
                &func_name,
            )?;
            write_report(&mut writer, &report)?;
        }
    }

    Ok(())
}

fn attempt_function(
    args: &BenchmarkLocalArgs,
    validator: &Validator,
    resolver: &LocalModuleResolver,
    package_addr: AccountAddress,
    module: &CompiledModule,
    module_name: &str,
    func_name: &str,
) -> Result<BenchmarkReport> {
    let mut report = BenchmarkReport {
        target_package: module_self_address_hex(module),
        target_module: module_name.to_string(),
        target_function: func_name.to_string(),
        status: AttemptStatus::Miss,
        failure_stage: None,
        failure_reason: None,
        tier_a_details: None,
        tier_b_details: None,
    };

    // A1: target exists and is invokable
    if let Err(e) = validator.validate_target(package_addr, module_name, func_name) {
        report.failure_stage = Some(FailureStage::A1);
        report.failure_reason = Some(format!("target validation failed: {e}"));
        return Ok(report);
    }

    let func_ident = Identifier::new(func_name)?;
    let func_def = module
        .function_defs()
        .iter()
        .find(|def| {
            let handle = module.function_handle_at(def.function);
            let name = module.identifier_at(handle.name);
            name == func_ident.as_ident_str()
        })
        .ok_or_else(|| anyhow!("function not found after validate_target"))?;
    let handle = module.function_handle_at(func_def.function);

    // For now: skip generics deterministically (future: integrate generator)
    if !handle.type_parameters.is_empty() {
        report.failure_stage = Some(FailureStage::A5);
        report.failure_reason = Some("generic functions not supported yet".to_string());
        return Ok(report);
    }

    let params_sig = module.signature_at(handle.parameters);
    let mut resolved_params = Vec::new();
    let mut default_values = Vec::new();
    let mut has_object_params = false;

    for token in &params_sig.0 {
        // A4: object parameters are recognized (typed object validation comes later)
        if matches!(
            token,
            SignatureToken::Reference(_) | SignatureToken::MutableReference(_)
        ) {
            has_object_params = true;
            resolved_params.push("object".to_string());
            continue;
        }

        // A2: resolve layout
        let layout = validator
            .resolve_token_to_tag(token, &[], module)
            .and_then(|tag| validator.resolve_type_layout(&tag))
            .map_err(|e| anyhow!("layout resolution failed: {e}"));

        let layout = match layout {
            Ok(l) => l,
            Err(e) => {
                report.failure_stage = Some(FailureStage::A2);
                report.failure_reason = Some(e.to_string());
                return Ok(report);
            }
        };

        // A3: roundtrip default value (strict but limited)
        let default_value = match generate_default_value(&layout) {
            Some(v) => v,
            None => {
                // No default generation for structs/enums yet; still a miss for now.
                report.failure_stage = Some(FailureStage::A3);
                report.failure_reason = Some("no default value generator for layout".to_string());
                return Ok(report);
            }
        };

        let bytes = default_value
            .simple_serialize()
            .ok_or_else(|| anyhow!("BCS serialize failed (value too deep?)"))?;
        if let Err(e) = validator.validate_bcs_roundtrip(&layout, &bytes) {
            report.failure_stage = Some(FailureStage::A3);
            report.failure_reason = Some(format!("BCS roundtrip failed: {e}"));
            return Ok(report);
        }

        resolved_params.push(format!("{layout:?}"));
        default_values.push(bytes);
    }

    report.status = AttemptStatus::TierAHit;
    report.tier_a_details = Some(TierADetails {
        resolved_params,
        bcs_roundtrip_verified: true,
        has_object_params,
    });

    // Tier B: only if requested and no object params.
    if args.tier_a_only || has_object_params {
        return Ok(report);
    }

    let mut harness = VMHarness::new(resolver, args.restricted_state).map_err(|e| {
        report.failure_stage = Some(FailureStage::B1);
        report.failure_reason = Some(format!("failed to create VM harness: {e}"));
        anyhow!("failed to create VM harness")
    })?;

    let exec = harness.execute_entry_function(
        &module.self_id(),
        func_ident.as_ident_str(),
        vec![],
        default_values,
    );

    match exec {
        Ok(()) => {
            report.status = AttemptStatus::TierBHit;
            report.tier_b_details = Some(TierBDetails {
                execution_success: true,
                error: None,
            });
        }
        Err(e) => {
            report.failure_stage = Some(FailureStage::B2);
            report.failure_reason = Some(format!("execution failed: {e}"));
            report.tier_b_details = Some(TierBDetails {
                execution_success: false,
                error: Some(e.to_string()),
            });
        }
    }

    Ok(report)
}

fn write_report(writer: &mut BufWriter<File>, report: &BenchmarkReport) -> Result<()> {
    serde_json::to_writer(&mut *writer, report)?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn generate_default_value(layout: &MoveTypeLayout) -> Option<MoveValue> {
    match layout {
        MoveTypeLayout::Bool => Some(MoveValue::Bool(false)),
        MoveTypeLayout::U8 => Some(MoveValue::U8(0)),
        MoveTypeLayout::U16 => Some(MoveValue::U16(0)),
        MoveTypeLayout::U32 => Some(MoveValue::U32(0)),
        MoveTypeLayout::U64 => Some(MoveValue::U64(0)),
        MoveTypeLayout::U128 => Some(MoveValue::U128(0)),
        MoveTypeLayout::U256 => Some(MoveValue::U256(move_core_types::u256::U256::zero())),
        MoveTypeLayout::Address => Some(MoveValue::Address(
            move_core_types::account_address::AccountAddress::ZERO,
        )),
        MoveTypeLayout::Signer => Some(MoveValue::Signer(
            move_core_types::account_address::AccountAddress::ZERO,
        )),
        MoveTypeLayout::Vector(_) => Some(MoveValue::Vector(vec![])), // Empty vector is always valid
        MoveTypeLayout::Struct(_) => None,
        MoveTypeLayout::Enum(_) => None,
    }
}
