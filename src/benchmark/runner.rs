use anyhow::{anyhow, Context, Result};
use move_binary_format::file_format::{CompiledModule, SignatureToken, Visibility};
use move_core_types::account_address::AccountAddress;
use move_core_types::annotated_value::{MoveTypeLayout, MoveValue};
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{ModuleId, TypeTag};
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Write};

use crate::args::BenchmarkLocalArgs;
use crate::benchmark::bytecode_analyzer::{self, StaticFunctionCall};
use crate::benchmark::constructor_map::{ConstructorMap, ConstructorInfo, ParamKind};
use crate::benchmark::resolver::LocalModuleResolver;
use crate::benchmark::validator::Validator;
use crate::benchmark::vm::VMHarness;
use crate::bytecode::{compiled_module_name, module_self_address_hex};

/// Well-known Sui framework addresses
const SUI_FRAMEWORK_ADDR: AccountAddress = AccountAddress::TWO; // 0x2

/// Check if a reference parameter is a synthesizable Sui system type.
/// These are types that the VM harness can provide without real on-chain state.
fn is_synthesizable_sui_param(token: &SignatureToken, module: &CompiledModule) -> Option<&'static str> {
    let inner = match token {
        SignatureToken::MutableReference(inner) => inner,
        SignatureToken::Reference(inner) => inner,
        _ => return None,
    };
    
    // Check if it's a struct type
    let idx = match inner.as_ref() {
        SignatureToken::Datatype(idx) => *idx,
        SignatureToken::DatatypeInstantiation(inst) => inst.0,
        _ => return None,
    };
    
    let handle = module.datatype_handle_at(idx);
    let module_handle = module.module_handle_at(handle.module);
    let address = *module.address_identifier_at(module_handle.address);
    let module_name = module.identifier_at(module_handle.name).as_str();
    let type_name = module.identifier_at(handle.name).as_str();
    
    // Check for well-known synthesizable types
    if address == SUI_FRAMEWORK_ADDR {
        match (module_name, type_name) {
            ("tx_context", "TxContext") => return Some("TxContext"),
            ("clock", "Clock") => return Some("Clock"),
            // Random requires special handling due to its internal state
            // ("random", "Random") => return Some("Random"),
            _ => {}
        }
    }
    
    None
}

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
    /// Modules from non-framework packages that were accessed during execution
    /// This shows which target package modules were actually exercised
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_modules_accessed: Option<Vec<String>>,
    /// Functions called by this entry function (from static bytecode analysis)
    /// Only includes non-framework calls for clarity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_calls: Option<Vec<StaticFunctionCall>>,
}

pub fn run_benchmark(args: &BenchmarkLocalArgs) -> Result<()> {
    // Start with Sui framework modules (0x1 move-stdlib, 0x2 sui-framework)
    // This enables Tier B execution of code that imports from std:: or sui::
    let mut resolver = LocalModuleResolver::with_sui_framework()
        .unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load Sui framework: {}. Continuing without it.", e);
            LocalModuleResolver::new()
        });
    let framework_count = resolver.iter_modules().count();
    if framework_count > 0 {
        eprintln!("Loaded {} Sui framework modules.", framework_count);
    }
    
    eprintln!("Loading corpus from {}...", args.target_corpus.display());
    let count = resolver.load_from_dir(&args.target_corpus)?;
    eprintln!("Loaded {} corpus modules.", count);

    let validator = Validator::new(&resolver);
    
    // Build constructor map for struct param synthesis
    let all_modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
    let constructor_map = ConstructorMap::from_modules(&all_modules);

    let output_file = File::create(&args.output)
        .with_context(|| format!("create output file {}", args.output.display()))?;
    let mut writer = BufWriter::new(output_file);

    // Framework addresses to skip - we only benchmark target package functions
    let framework_addrs = [
        AccountAddress::ONE,  // 0x1 move-stdlib
        AccountAddress::TWO,  // 0x2 sui-framework
        AccountAddress::from_hex_literal("0x3").unwrap_or(AccountAddress::ZERO), // sui-system
    ];
    
    for module in resolver.iter_modules() {
        let addr = *module.self_id().address();
        
        // Skip framework modules - only benchmark target package functions
        if framework_addrs.contains(&addr) {
            continue;
        }
        
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

            let report = attempt_function(
                args,
                &validator,
                &resolver,
                &constructor_map,
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
    constructor_map: &ConstructorMap,
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

    // Handle generic functions: try to instantiate with primitive types
    let type_args: Vec<move_core_types::language_storage::TypeTag> = if !handle.type_parameters.is_empty() {
        // For each type parameter, pick a primitive type that satisfies its constraints
        // All primitives (u64, bool, address) have copy, drop, store, so they satisfy any constraint
        handle.type_parameters
            .iter()
            .map(|_abilities| {
                // Use u64 as default - it has all abilities and is simple to serialize
                move_core_types::language_storage::TypeTag::U64
            })
            .collect()
    } else {
        vec![]
    };

    let params_sig = module.signature_at(handle.parameters);
    let mut resolved_params = Vec::new();
    let mut default_values = Vec::new();
    let mut has_real_object_params = false;  // True only for non-synthesizable object refs
    let mut synthesizable_params: Vec<&'static str> = Vec::new();  // Track synthesizable params
    
    // Track params that need constructor chaining
    // Each entry is (param_index, constructor_info) for params that need to be constructed
    let mut constructor_chain: Vec<(usize, ConstructorInfo)> = Vec::new();

    for (param_idx, token) in params_sig.0.iter().enumerate() {
        // A4: Check for reference parameters
        if matches!(
            token,
            SignatureToken::Reference(_) | SignatureToken::MutableReference(_)
        ) {
            // Check if this is a synthesizable Sui system type (TxContext, Clock, etc.)
            if let Some(synth_type) = is_synthesizable_sui_param(token, module) {
                resolved_params.push(format!("synthesizable:{}", synth_type));
                synthesizable_params.push(synth_type);
                continue;
            }
            // Non-synthesizable object reference - blocks Tier B for now
            has_real_object_params = true;
            resolved_params.push("object".to_string());
            continue;
        }

        // Handle type parameter tokens specially
        if let SignatureToken::TypeParameter(idx) = token {
            // This parameter is a type variable - substitute with our type_arg
            let idx = *idx as usize;
            if idx < type_args.len() {
                // Resolve the substituted type's layout
                let layout = match validator.resolve_type_layout(&type_args[idx]) {
                    Ok(l) => l,
                    Err(e) => {
                        report.failure_stage = Some(FailureStage::A2);
                        report.failure_reason = Some(format!("type param layout resolution failed: {e}"));
                        return Ok(report);
                    }
                };
                
                let default_value = match generate_default_value(&layout) {
                    Some(v) => v,
                    None => {
                        report.failure_stage = Some(FailureStage::A3);
                        report.failure_reason = Some("no default for type param".to_string());
                        return Ok(report);
                    }
                };
                
                let bytes = default_value.simple_serialize()
                    .ok_or_else(|| anyhow!("BCS serialize failed"))?;
                resolved_params.push(format!("type_param[{}]={:?}", idx, layout));
                default_values.push(bytes);
                continue;
            } else {
                report.failure_stage = Some(FailureStage::A5);
                report.failure_reason = Some(format!("type param index {} out of bounds", idx));
                return Ok(report);
            }
        }
        
        // A2: resolve layout (pass type_args for proper substitution)
        let layout = validator
            .resolve_token_to_tag(token, &type_args, module)
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
                // Can't generate default - check if we can construct this type
                // First, get the TypeTag for this token
                let tag = match validator.resolve_token_to_tag(token, &type_args, module) {
                    Ok(t) => t,
                    Err(_) => {
                        report.failure_stage = Some(FailureStage::A3);
                        report.failure_reason = Some("no default value generator for layout".to_string());
                        return Ok(report);
                    }
                };
                
                // Check if this is a struct we can construct
                if let move_core_types::language_storage::TypeTag::Struct(struct_tag) = &tag {
                    // Special case: TreasuryCap needs OTW + coin::create_currency
                    if ConstructorMap::is_treasury_cap(struct_tag) {
                        // Check if we have an OTW type available
                        if let Some(otw) = constructor_map.get_first_otw() {
                            // We can create TreasuryCap via coin::create_currency!
                            // Mark this for special handling during execution
                            resolved_params.push(format!("treasury_cap_via_otw:{}", otw.struct_name));
                            default_values.push(vec![]); // Placeholder
                            // Store OTW info for execution phase
                            // We'll handle this specially in the execution loop
                            constructor_chain.push((param_idx, ConstructorInfo {
                                module_id: ModuleId::new(
                                    AccountAddress::TWO,
                                    Identifier::new("coin").unwrap(),
                                ),
                                function_name: "create_currency".to_string(),
                                type_params: 1, // T for TreasuryCap<T>
                                params: vec![
                                    ParamKind::TypeParam(0), // witness: T
                                    ParamKind::Primitive(TypeTag::U8), // decimals
                                    ParamKind::PrimitiveVector(TypeTag::U8), // symbol
                                    ParamKind::PrimitiveVector(TypeTag::U8), // name  
                                    ParamKind::PrimitiveVector(TypeTag::U8), // description
                                    // option::none for icon_url - we'll handle specially
                                    ParamKind::TxContext,
                                ],
                                returns: struct_tag.as_ref().clone(),
                            }));
                            continue;
                        }
                    }
                    
                    if let Some(ctor) = constructor_map.find_synthesizable_constructor(struct_tag) {
                        // We can construct this! Track it for later
                        constructor_chain.push((param_idx, ctor.clone()));
                        resolved_params.push(format!("construct:{}", struct_tag.name));
                        // Push placeholder - will be replaced during execution
                        default_values.push(vec![]);
                        continue;
                    }
                }
                
                // No constructor found
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
        has_object_params: has_real_object_params,
    });

    // Tier B: only if requested and no real object params (synthesizable params are OK).
    if args.tier_a_only || has_real_object_params {
        return Ok(report);
    }

    let mut harness = VMHarness::new(resolver, args.restricted_state).map_err(|e| {
        report.failure_stage = Some(FailureStage::B1);
        report.failure_reason = Some(format!("failed to create VM harness: {e}"));
        anyhow!("failed to create VM harness")
    })?;

    // Clear trace before execution to get a clean trace for this function
    harness.clear_trace();
    
    // Execute constructor chain if needed
    let mut final_args = default_values.clone();
    for (param_idx, ctor) in &constructor_chain {
        // Special case: coin::create_currency needs OTW handling
        let (ctor_type_args, ctor_args) = if ctor.function_name == "create_currency" 
            && ctor.module_id.address() == &AccountAddress::TWO 
        {
            // Get OTW type for create_currency
            let otw = constructor_map.get_first_otw()
                .ok_or_else(|| anyhow!("no OTW type available for create_currency"))?;
            
            // Type arg is the OTW type
            let type_args = vec![otw.type_tag.clone()];
            
            // Build args for create_currency:
            // witness: T, decimals: u8, symbol: vector<u8>, name: vector<u8>, 
            // description: vector<u8>, icon_url: Option<Url>, ctx: &mut TxContext
            let mut args = Vec::new();
            
            // witness: OTW { dummy: true } - BCS encoded struct with one bool field
            args.push(vec![1u8]); // BCS for struct with bool field = true
            
            // decimals: u8
            args.push(vec![9u8]); // 9 decimals like SUI
            
            // symbol: vector<u8>
            args.push(bcs_encode_vector(b"TEST"));
            
            // name: vector<u8>  
            args.push(bcs_encode_vector(b"Test Token"));
            
            // description: vector<u8>
            args.push(bcs_encode_vector(b"Test token for type inhabitation"));
            
            // icon_url: Option<Url> - None
            args.push(vec![0u8]); // BCS for Option::None
            
            // ctx: &mut TxContext - synthesized
            args.push(harness.synthesize_tx_context()?);
            
            (type_args, args)
        } else {
            // Normal constructor
            let args = match build_constructor_args(&mut harness, ctor, validator) {
                Ok(args) => args,
                Err(e) => {
                    report.failure_stage = Some(FailureStage::B1);
                    report.failure_reason = Some(format!("constructor arg build failed: {e}"));
                    return Ok(report);
                }
            };
            
            let type_args: Vec<_> = (0..ctor.type_params)
                .map(|_| TypeTag::U64)
                .collect();
            
            (type_args, args)
        };
        
        // Execute constructor and get return value
        let returns = match harness.execute_function_with_return(
            &ctor.module_id,
            &ctor.function_name,
            ctor_type_args,
            ctor_args,
        ) {
            Ok(r) => r,
            Err(e) => {
                report.failure_stage = Some(FailureStage::B1);
                report.failure_reason = Some(format!("constructor execution failed: {e}"));
                return Ok(report);
            }
        };
        
        // Use the first return value as the constructed struct
        if let Some(constructed_bytes) = returns.into_iter().next() {
            final_args[*param_idx] = constructed_bytes;
        } else {
            report.failure_stage = Some(FailureStage::B1);
            report.failure_reason = Some("constructor returned no value".to_string());
            return Ok(report);
        }
    }
    
    // Execute function - use entry function path for entry functions, regular for public
    let exec = if func_def.is_entry {
        harness.execute_entry_function_with_synth(
            &module.self_id(),
            func_ident.as_ident_str(),
            type_args.clone(),
            final_args,
            &synthesizable_params,
        )
    } else {
        // For non-entry public functions, we need to inject synthesizable params manually
        let mut augmented_args = final_args;
        for synth_type in &synthesizable_params {
            match *synth_type {
                "TxContext" => {
                    augmented_args.push(harness.synthesize_tx_context()?);
                }
                "Clock" => {
                    augmented_args.push(harness.synthesize_clock()?);
                }
                _ => {}
            }
        }
        harness.execute_function(
            &module.self_id(),
            func_name,
            type_args.clone(),
            augmented_args,
        )
    };

    // Get the execution trace to see which modules were accessed
    let trace = harness.get_trace();
    
    // Filter out framework modules (0x1, 0x2, 0x3) to get target package modules
    let target_modules: Vec<String> = trace.modules_accessed
        .iter()
        .filter(|id| {
            let addr = id.address();
            // Exclude framework addresses
            *addr != AccountAddress::ONE &&  // 0x1 move-stdlib
            *addr != AccountAddress::TWO &&  // 0x2 sui-framework
            *addr != AccountAddress::from_hex_literal("0x3").unwrap_or(AccountAddress::ZERO) // sui-system
        })
        .map(|id| format!("{}::{}", id.address().to_hex_literal(), id.name()))
        .collect();
    
    // Static bytecode analysis: extract function calls from the entry function
    let static_calls = bytecode_analyzer::extract_function_calls_from_function(module, func_def);
    let non_framework_calls = bytecode_analyzer::filter_non_framework_calls(&static_calls);

    match exec {
        Ok(()) => {
            report.status = AttemptStatus::TierBHit;
            report.tier_b_details = Some(TierBDetails {
                execution_success: true,
                error: None,
                target_modules_accessed: if target_modules.is_empty() { None } else { Some(target_modules) },
                static_calls: if non_framework_calls.is_empty() { None } else { Some(non_framework_calls) },
            });
        }
        Err(e) => {
            report.failure_stage = Some(FailureStage::B2);
            report.failure_reason = Some(format!("execution failed: {e}"));
            report.tier_b_details = Some(TierBDetails {
                execution_success: false,
                error: Some(e.to_string()),
                target_modules_accessed: if target_modules.is_empty() { None } else { Some(target_modules) },
                static_calls: if non_framework_calls.is_empty() { None } else { Some(non_framework_calls) },
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

/// BCS encode a byte vector (length-prefixed)
fn bcs_encode_vector(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    // ULEB128 encode the length
    let mut len = data.len();
    loop {
        let mut byte = (len & 0x7f) as u8;
        len >>= 7;
        if len != 0 {
            byte |= 0x80;
        }
        result.push(byte);
        if len == 0 {
            break;
        }
    }
    result.extend_from_slice(data);
    result
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

/// Build arguments for a constructor function.
/// This handles synthesizable params (TxContext, primitives, type params).
fn build_constructor_args(
    harness: &mut VMHarness,
    ctor: &ConstructorInfo,
    validator: &Validator,
) -> Result<Vec<Vec<u8>>> {
    let mut args = Vec::new();
    
    for param in &ctor.params {
        match param {
            ParamKind::Primitive(tag) => {
                // Generate default value for this primitive
                let layout = validator.resolve_type_layout(tag)?;
                let value = generate_default_value(&layout)
                    .ok_or_else(|| anyhow!("no default for primitive"))?;
                let bytes = value.simple_serialize()
                    .ok_or_else(|| anyhow!("serialize failed"))?;
                args.push(bytes);
            }
            ParamKind::PrimitiveVector(_tag) => {
                // Empty vector
                args.push(vec![0]); // BCS encoding of empty vector
            }
            ParamKind::TxContext => {
                // Synthesize TxContext - use the harness's synthesizer
                let ctx_bytes = harness.synthesize_tx_context()?;
                args.push(ctx_bytes);
            }
            ParamKind::Clock => {
                // Synthesize Clock
                let clock_bytes = harness.synthesize_clock()?;
                args.push(clock_bytes);
            }
            ParamKind::TypeParam(_idx) => {
                // Type parameter instantiated with u64 - use default u64
                let bytes = 0u64.to_le_bytes().to_vec();
                args.push(bytes);
            }
            ParamKind::Struct(_) => {
                // Nested struct - would need recursive construction
                // For now, this shouldn't happen since find_synthesizable_constructor
                // only returns constructors with synthesizable params
                return Err(anyhow!("nested struct construction not supported"));
            }
            ParamKind::Unsupported(desc) => {
                return Err(anyhow!("unsupported param type: {}", desc));
            }
        }
    }
    
    Ok(args)
}
