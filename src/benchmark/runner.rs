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
use crate::benchmark::constructor_map::{ConstructorInfo, ConstructorMap, ParamKind};
use crate::benchmark::errors::{is_unsupported_native_error, unsupported_native_error_message};
use crate::benchmark::mm2::{ConstructorGraph, ExecutionChain, ParamRequirement, Producer, ProducerChain, TypeModel, TypeSynthesizer};
use crate::benchmark::resolver::LocalModuleResolver;
use crate::benchmark::validator::Validator;
use crate::benchmark::vm::VMHarness;
use crate::bytecode::{compiled_module_name, module_self_address_hex};

/// Well-known Sui framework addresses
const SUI_FRAMEWORK_ADDR: AccountAddress = AccountAddress::TWO; // 0x2

/// Entry in the constructor chain - either an intermediate dependency or a final param
#[derive(Debug, Clone)]
enum ConstructorChainEntry {
    /// Intermediate constructor - result stored by return type key
    Intermediate(ConstructorInfo),
    /// Final param constructor - result stored at param_idx in final_args
    FinalParam {
        param_idx: usize,
        ctor: ConstructorInfo,
    },
    /// Reference to a constructed value - construct first, then pass by reference
    /// The constructed value is stored in the VM and a reference is passed
    ConstructedRef {
        param_idx: usize,
        ctor: ConstructorInfo,
        is_mut: bool,
    },
    /// Multi-hop execution chain (from ConstructorGraph)
    /// Contains all constructors in topological order
    MultiHopChain {
        param_idx: usize,
        chain: ExecutionChain,
        is_ref: bool,
        is_mut: bool,
    },
    /// Producer chain (from return type analysis)
    /// Handles multi-return functions like create_lst() -> (AdminCap, CollectionFeeCap, LiquidStakingInfo)
    ProducerChain {
        param_idx: usize,
        chain: ProducerChain,
        target_return_idx: usize,
        is_ref: bool,
        is_mut: bool,
    },
    /// Synthesized type (from MM2 type analysis, no execution needed)
    /// Used as fallback when constructors/producers aren't available or fail
    Synthesized {
        param_idx: usize,
        /// Pre-computed BCS bytes for the synthesized value
        bytes: Vec<u8>,
        /// Type description for logging
        type_desc: String,
    },
}

/// Check if a reference parameter is a synthesizable Sui system type.
/// These are types that the VM harness can provide without real on-chain state.
fn is_synthesizable_sui_param(
    token: &SignatureToken,
    module: &CompiledModule,
) -> Option<&'static str> {
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

/// Extract StructTag from a reference parameter token.
/// Returns (struct_tag, is_mutable) if the reference points to a struct.
fn extract_ref_struct_tag(
    token: &SignatureToken,
    module: &CompiledModule,
) -> Option<(move_core_types::language_storage::StructTag, bool)> {
    let (inner, is_mut) = match token {
        SignatureToken::MutableReference(inner) => (inner.as_ref(), true),
        SignatureToken::Reference(inner) => (inner.as_ref(), false),
        _ => return None,
    };

    // Extract the struct index
    let idx = match inner {
        SignatureToken::Datatype(idx) => *idx,
        SignatureToken::DatatypeInstantiation(inst) => inst.0,
        _ => return None,
    };

    let handle = module.datatype_handle_at(idx);
    let module_handle = module.module_handle_at(handle.module);
    let address = *module.address_identifier_at(module_handle.address);
    let module_name = module.identifier_at(module_handle.name).to_owned();
    let name = module.identifier_at(handle.name).to_owned();

    // Handle type args for DatatypeInstantiation
    let type_params = if let SignatureToken::DatatypeInstantiation(inst) = inner {
        inst.1
            .iter()
            .filter_map(|t| token_to_type_tag_simple(t, module))
            .collect()
    } else {
        vec![]
    };

    Some((
        move_core_types::language_storage::StructTag {
            address,
            module: module_name,
            name,
            type_params,
        },
        is_mut,
    ))
}

/// Simple token to TypeTag conversion for reference extraction
fn token_to_type_tag_simple(
    token: &SignatureToken,
    module: &CompiledModule,
) -> Option<TypeTag> {
    match token {
        SignatureToken::Bool => Some(TypeTag::Bool),
        SignatureToken::U8 => Some(TypeTag::U8),
        SignatureToken::U16 => Some(TypeTag::U16),
        SignatureToken::U32 => Some(TypeTag::U32),
        SignatureToken::U64 => Some(TypeTag::U64),
        SignatureToken::U128 => Some(TypeTag::U128),
        SignatureToken::U256 => Some(TypeTag::U256),
        SignatureToken::Address => Some(TypeTag::Address),
        SignatureToken::TypeParameter(_) => Some(TypeTag::U64), // Default to u64
        SignatureToken::Vector(inner) => {
            token_to_type_tag_simple(inner, module).map(|t| TypeTag::Vector(Box::new(t)))
        }
        SignatureToken::Datatype(idx) => {
            let handle = module.datatype_handle_at(*idx);
            let module_handle = module.module_handle_at(handle.module);
            let address = *module.address_identifier_at(module_handle.address);
            let module_name = module.identifier_at(module_handle.name).to_owned();
            let name = module.identifier_at(handle.name).to_owned();
            Some(TypeTag::Struct(Box::new(
                move_core_types::language_storage::StructTag {
                    address,
                    module: module_name,
                    name,
                    type_params: vec![],
                },
            )))
        }
        _ => None,
    }
}

/// Result status of a type inhabitation attempt.
///
/// ## Status Meanings
///
/// | Status | Description |
/// |--------|-------------|
/// | `tier_a_hit` | Arguments synthesized successfully, but execution not attempted or failed |
/// | `tier_b_hit` | Function executed successfully without abort |
/// | `miss` | Failed at some stage (check `failure_stage` and `failure_reason`) |
///
/// ## Tier Definitions
///
/// - **Tier A**: Proves the LLM understands type signatures (code compiles/args build)
/// - **Tier B**: Proves the LLM understands runtime semantics (code executes)
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    /// Arguments synthesized successfully (Tier A passed)
    TierAHit,
    /// Function executed without abort (Tier A + B passed)
    TierBHit,
    /// Failed at some stage (see failure_stage for details)
    Miss,
}

/// Failure stages in the type inhabitation evaluation pipeline.
///
/// The pipeline has two tiers:
/// - **Tier A (Argument Synthesis)**: Can we build valid arguments for the function?
/// - **Tier B (Execution)**: Does the function execute without aborting?
///
/// ## Tier A Stages (Argument Synthesis)
///
/// | Stage | Name | Description |
/// |-------|------|-------------|
/// | A1 | Target Validation | Function doesn't exist or isn't callable (private, missing module) |
/// | A2 | Layout Resolution | Can't determine the memory layout for a parameter type |
/// | A3 | Value Synthesis | Can't generate a valid value for a parameter (no constructor, no default) |
/// | A4 | (Reserved) | Currently unused |
/// | A5 | Type Parameter Bounds | Generic type parameter index out of bounds |
///
/// ## Tier B Stages (Execution)
///
/// | Stage | Name | Description |
/// |-------|------|-------------|
/// | B1 | VM Setup / Constructor | VM harness creation failed, or constructor chaining failed |
/// | B2 | Execution | Function aborted during execution (assertion, unsupported native, etc.) |
///
/// ## Interpreting B2 Failures
///
/// When `failure_stage == B2`, check the `failure_reason`:
/// - Contains "error 1000": Unsupported native function (crypto, randomness, zklogin)
/// - Contains "MoveAbort": Function assertion failed or explicit abort
/// - Contains "MISSING_DEPENDENCY": Required module not loaded
///
/// ## Success Cases
///
/// - `tier_a_hit`: Arguments synthesized successfully (A stages passed)
/// - `tier_b_hit`: Function executed without abort (all stages passed)
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum FailureStage {
    /// A1: Target function doesn't exist, isn't public, or module not found
    A1,
    /// A2: Cannot resolve type layout for parameter (unknown struct, recursive type)
    A2,
    /// A3: Cannot synthesize value for parameter (no constructor, no default generator)
    A3,
    /// A4: Reserved for future use
    A4,
    /// A5: Generic type parameter index out of bounds
    A5,
    /// B1: VM harness creation failed or constructor chaining failed
    B1,
    /// B2: Function execution aborted (assertion, unsupported native, runtime error)
    B2,
}

impl FailureStage {
    /// Get a human-readable description of this failure stage.
    pub fn description(&self) -> &'static str {
        match self {
            FailureStage::A1 => "target validation failed (function not found or not callable)",
            FailureStage::A2 => "type layout resolution failed (unknown or recursive type)",
            FailureStage::A3 => "value synthesis failed (no constructor or default available)",
            FailureStage::A4 => "reserved stage (unused)",
            FailureStage::A5 => "type parameter out of bounds",
            FailureStage::B1 => "VM setup or constructor execution failed",
            FailureStage::B2 => "function execution aborted",
        }
    }

    /// Get the tier (A or B) for this stage.
    pub fn tier(&self) -> &'static str {
        match self {
            FailureStage::A1
            | FailureStage::A2
            | FailureStage::A3
            | FailureStage::A4
            | FailureStage::A5 => "A (argument synthesis)",
            FailureStage::B1 | FailureStage::B2 => "B (execution)",
        }
    }
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
    let mut resolver = LocalModuleResolver::with_sui_framework().unwrap_or_else(|e| {
        eprintln!(
            "Warning: Failed to load Sui framework: {}. Continuing without it.",
            e
        );
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

    // Build constructor map for struct param synthesis (single-hop fallback)
    let all_modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
    let constructor_map = ConstructorMap::from_modules(&all_modules);

    // Build MM2 constructor graph for multi-hop chains
    let type_model = TypeModel::from_modules(all_modules.clone())
        .map_err(|e| anyhow!("Failed to build TypeModel: {:?}", e))?;
    let mut constructor_graph = ConstructorGraph::from_model(&type_model);
    let graph_stats = constructor_graph.stats();
    eprintln!(
        "Built constructor graph: {} types, {} with constructors, {} total constructors, {} with producers, {} total producers",
        graph_stats.total_types, graph_stats.types_with_constructors, graph_stats.total_constructors,
        graph_stats.types_with_producers, graph_stats.total_producers
    );

    let output_file = File::create(&args.output)
        .with_context(|| format!("create output file {}", args.output.display()))?;
    let mut writer = BufWriter::new(output_file);

    // Framework addresses to skip - we only benchmark target package functions
    let framework_addrs = [
        AccountAddress::ONE, // 0x1 move-stdlib
        AccountAddress::TWO, // 0x2 sui-framework
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
                &mut constructor_graph,
                &type_model,
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

#[allow(clippy::too_many_arguments)]
fn attempt_function(
    args: &BenchmarkLocalArgs,
    validator: &Validator,
    resolver: &LocalModuleResolver,
    constructor_map: &ConstructorMap,
    constructor_graph: &mut ConstructorGraph,
    type_model: &TypeModel,
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
    let type_args: Vec<move_core_types::language_storage::TypeTag> =
        if !handle.type_parameters.is_empty() {
            // For each type parameter, pick a primitive type that satisfies its constraints
            // All primitives (u64, bool, address) have copy, drop, store, so they satisfy any constraint
            handle
                .type_parameters
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
    let mut has_real_object_params = false; // True only for non-synthesizable object refs
    let mut synthesizable_params: Vec<&'static str> = Vec::new(); // Track synthesizable params

    // Track params that need constructor chaining
    // Uses ConstructorChainEntry enum to cleanly distinguish intermediate vs final params
    let mut constructor_chain: Vec<ConstructorChainEntry> = Vec::new();

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

            // NEW: Check if the referenced type is constructible
            // If we can construct the value, we can pass a reference to it
            if let Some((struct_tag, is_mut)) = extract_ref_struct_tag(token, module) {
                // Try direct synthesizable constructor first (fastest path)
                if let Some(ctor) = constructor_map.find_synthesizable_constructor(&struct_tag) {
                    constructor_chain.push(ConstructorChainEntry::ConstructedRef {
                        param_idx,
                        ctor: ctor.clone(),
                        is_mut,
                    });
                    let mut_str = if is_mut { "&mut " } else { "&" };
                    resolved_params.push(format!("constructible_ref:{}{}", mut_str, struct_tag.name));
                    default_values.push(vec![]); // Placeholder - filled during execution
                    continue;
                }

                // Try multi-hop constructor chain via ConstructorGraph (MM2-based)
                // This handles chains of any depth up to MAX_CHAIN_DEPTH
                if let Some(chain) = constructor_graph.find_execution_chain(
                    &struct_tag.address,
                    struct_tag.module.as_str(),
                    struct_tag.name.as_str(),
                ) {
                    let depth = chain.depth;
                    constructor_chain.push(ConstructorChainEntry::MultiHopChain {
                        param_idx,
                        chain,
                        is_ref: true,
                        is_mut,
                    });
                    let mut_str = if is_mut { "&mut " } else { "&" };
                    resolved_params.push(format!(
                        "constructible_ref_hop{}:{}{}",
                        depth, mut_str, struct_tag.name
                    ));
                    default_values.push(vec![]); // Placeholder - filled during execution
                    continue;
                }

                // Try producer chain via return type analysis
                // This handles multi-return functions like create_lst() -> (AdminCap, CollectionFeeCap, LiquidStakingInfo)
                if let Some(producer_chain) = constructor_graph.find_producer_chain(
                    &struct_tag.address,
                    struct_tag.module.as_str(),
                    struct_tag.name.as_str(),
                ) {
                    let depth = producer_chain.depth;
                    // Find which return index produces our target type
                    let target_return_idx = producer_chain.steps.last()
                        .map(|s| s.target_return_idx)
                        .unwrap_or(0);
                    constructor_chain.push(ConstructorChainEntry::ProducerChain {
                        param_idx,
                        chain: producer_chain,
                        target_return_idx,
                        is_ref: true,
                        is_mut,
                    });
                    let mut_str = if is_mut { "&mut " } else { "&" };
                    resolved_params.push(format!(
                        "producer_ref_hop{}:{}{}",
                        depth, mut_str, struct_tag.name
                    ));
                    default_values.push(vec![]); // Placeholder - filled during execution
                    continue;
                }

                // Fallback: Try single-hop constructor from bytecode-based map
                if let Some((ctor, dep_ctors)) =
                    constructor_map.find_single_hop_constructor(&struct_tag)
                {
                    // Add dependency constructors first
                    for dep_ctor in dep_ctors.iter() {
                        constructor_chain
                            .push(ConstructorChainEntry::Intermediate((*dep_ctor).clone()));
                    }
                    // Then add the main constructor as a ConstructedRef
                    constructor_chain.push(ConstructorChainEntry::ConstructedRef {
                        param_idx,
                        ctor: ctor.clone(),
                        is_mut,
                    });
                    let mut_str = if is_mut { "&mut " } else { "&" };
                    resolved_params.push(format!("constructible_ref_hop:{}{}", mut_str, struct_tag.name));
                    default_values.push(vec![]); // Placeholder - filled during execution
                    continue;
                }

                // Last resort: Try direct type synthesis via MM2
                // This creates valid BCS bytes without executing any function
                let mut synthesizer = TypeSynthesizer::new(type_model);
                if let Ok(result) = synthesizer.synthesize_struct(
                    &struct_tag.address,
                    struct_tag.module.as_str(),
                    struct_tag.name.as_str(),
                ) {
                    let mut_str = if is_mut { "&mut " } else { "&" };
                    constructor_chain.push(ConstructorChainEntry::Synthesized {
                        param_idx,
                        bytes: result.bytes,
                        type_desc: format!("{}{}", mut_str, struct_tag.name),
                    });
                    resolved_params.push(format!(
                        "synthesized_ref:{}{}",
                        mut_str, struct_tag.name
                    ));
                    default_values.push(vec![]); // Placeholder - filled by Synthesized entry
                    continue;
                }
            }

            // Non-synthesizable and non-constructible object reference - blocks Tier B
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
                        report.failure_reason =
                            Some(format!("type param layout resolution failed: {e}"));
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

                let bytes = default_value
                    .simple_serialize()
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
                        report.failure_reason =
                            Some("no default value generator for layout".to_string());
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
                            resolved_params
                                .push(format!("treasury_cap_via_otw:{}", otw.struct_name));
                            default_values.push(vec![]); // Placeholder
                                                         // Store OTW info for execution phase
                                                         // We'll handle this specially in the execution loop
                            constructor_chain.push(ConstructorChainEntry::FinalParam {
                                param_idx,
                                ctor: ConstructorInfo {
                                    module_id: ModuleId::new(
                                        AccountAddress::TWO,
                                        Identifier::new("coin").unwrap(),
                                    ),
                                    function_name: "create_currency".to_string(),
                                    type_params: 1, // T for TreasuryCap<T>
                                    params: vec![
                                        ParamKind::TypeParam(0),                 // witness: T
                                        ParamKind::Primitive(TypeTag::U8),       // decimals
                                        ParamKind::PrimitiveVector(TypeTag::U8), // symbol
                                        ParamKind::PrimitiveVector(TypeTag::U8), // name
                                        ParamKind::PrimitiveVector(TypeTag::U8), // description
                                        // option::none for icon_url - we'll handle specially
                                        ParamKind::TxContext,
                                    ],
                                    returns: struct_tag.as_ref().clone(),
                                },
                            });
                            continue;
                        }
                    }

                    if let Some(ctor) = constructor_map.find_synthesizable_constructor(struct_tag) {
                        // We can construct this! Track it for later
                        constructor_chain.push(ConstructorChainEntry::FinalParam {
                            param_idx,
                            ctor: ctor.clone(),
                        });
                        resolved_params.push(format!("construct:{}", struct_tag.name));
                        // Push placeholder - will be replaced during execution
                        default_values.push(vec![]);
                        continue;
                    }

                    // Try multi-hop constructor chain via ConstructorGraph (MM2-based)
                    // This handles chains of any depth up to MAX_CHAIN_DEPTH
                    if let Some(chain) = constructor_graph.find_execution_chain(
                        &struct_tag.address,
                        struct_tag.module.as_str(),
                        struct_tag.name.as_str(),
                    ) {
                        let depth = chain.depth;
                        constructor_chain.push(ConstructorChainEntry::MultiHopChain {
                            param_idx,
                            chain,
                            is_ref: false,
                            is_mut: false,
                        });
                        resolved_params.push(format!("construct_hop{}:{}", depth, struct_tag.name));
                        default_values.push(vec![]); // Placeholder - filled during execution
                        continue;
                    }

                    // Try producer chain via return type analysis
                    // This handles multi-return functions like create_lst() -> (AdminCap, CollectionFeeCap, LiquidStakingInfo)
                    if let Some(producer_chain) = constructor_graph.find_producer_chain(
                        &struct_tag.address,
                        struct_tag.module.as_str(),
                        struct_tag.name.as_str(),
                    ) {
                        let depth = producer_chain.depth;
                        let target_return_idx = producer_chain.steps.last()
                            .map(|s| s.target_return_idx)
                            .unwrap_or(0);
                        constructor_chain.push(ConstructorChainEntry::ProducerChain {
                            param_idx,
                            chain: producer_chain,
                            target_return_idx,
                            is_ref: false,
                            is_mut: false,
                        });
                        resolved_params.push(format!("producer_hop{}:{}", depth, struct_tag.name));
                        default_values.push(vec![]); // Placeholder - filled during execution
                        continue;
                    }

                    // Fallback: Try single-hop constructor from bytecode-based map
                    if let Some((ctor, dep_ctors)) =
                        constructor_map.find_single_hop_constructor(struct_tag)
                    {
                        // Add dependency constructors first (they need to run before the main ctor)
                        // These are intermediate results stored by return type key
                        for dep_ctor in dep_ctors.iter() {
                            constructor_chain
                                .push(ConstructorChainEntry::Intermediate((*dep_ctor).clone()));
                        }
                        // Then add the main constructor that uses these dependencies
                        constructor_chain.push(ConstructorChainEntry::FinalParam {
                            param_idx,
                            ctor: ctor.clone(),
                        });
                        resolved_params.push(format!("construct_hop:{}", struct_tag.name));
                        // Push placeholder - will be replaced during execution
                        default_values.push(vec![]);
                        continue;
                    }

                    // Last resort: Try direct type synthesis via MM2
                    let mut synthesizer = TypeSynthesizer::new(type_model);
                    if let Ok(result) = synthesizer.synthesize_struct(
                        &struct_tag.address,
                        struct_tag.module.as_str(),
                        struct_tag.name.as_str(),
                    ) {
                        constructor_chain.push(ConstructorChainEntry::Synthesized {
                            param_idx,
                            bytes: result.bytes,
                            type_desc: struct_tag.name.to_string(),
                        });
                        resolved_params.push(format!("synthesized:{}", struct_tag.name));
                        default_values.push(vec![]); // Placeholder - filled by Synthesized entry
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
    // constructed_intermediates stores results from intermediate constructors (keyed by return type)
    let mut final_args = default_values.clone();
    let mut constructed_intermediates: std::collections::HashMap<String, Vec<u8>> =
        std::collections::HashMap::new();

    for entry in &constructor_chain {
        match entry {
            // MultiHopChain: Execute all steps in the chain, then store the final result
            ConstructorChainEntry::MultiHopChain { param_idx, chain, is_ref: _, is_mut: _ } => {
                // Execute each step in the chain in order
                for step in &chain.steps {
                    let ctor = &step.ctor_info;

                    // Build args, using constructed_intermediates for dependencies
                    let args = match build_constructor_args_with_intermediates(
                        &mut harness,
                        ctor,
                        validator,
                        &constructed_intermediates,
                    ) {
                        Ok(args) => args,
                        Err(e) => {
                            report.failure_stage = Some(FailureStage::B1);
                            report.failure_reason =
                                Some(format!("multi-hop constructor arg build failed: {e}"));
                            return Ok(report);
                        }
                    };

                    let type_args: Vec<_> = (0..ctor.type_params).map(|_| TypeTag::U64).collect();

                    // Execute this step's constructor
                    let returns = match harness.execute_function_with_return(
                        &ctor.module_id,
                        &ctor.function_name,
                        type_args,
                        args,
                    ) {
                        Ok(r) => r,
                        Err(e) => {
                            report.failure_stage = Some(FailureStage::B1);
                            report.failure_reason =
                                Some(format!("multi-hop constructor execution failed: {e}"));
                            return Ok(report);
                        }
                    };

                    // Store result in intermediates keyed by type
                    if let Some(constructed_bytes) = returns.into_iter().next() {
                        constructed_intermediates.insert(step.type_key.clone(), constructed_bytes);
                    } else {
                        report.failure_stage = Some(FailureStage::B1);
                        report.failure_reason =
                            Some("multi-hop constructor returned no value".to_string());
                        return Ok(report);
                    }
                }

                // Get the final constructed value from intermediates
                if let Some(target_key) = chain.target_type() {
                    if let Some(final_bytes) = constructed_intermediates.get(target_key) {
                        if *param_idx < final_args.len() {
                            final_args[*param_idx] = final_bytes.clone();
                        } else {
                            report.failure_stage = Some(FailureStage::B1);
                            report.failure_reason = Some(format!(
                                "MultiHopChain param_idx {} out of bounds (final_args len: {})",
                                param_idx,
                                final_args.len()
                            ));
                            return Ok(report);
                        }
                    } else {
                        report.failure_stage = Some(FailureStage::B1);
                        report.failure_reason =
                            Some(format!("multi-hop chain target type not found: {}", target_key));
                        return Ok(report);
                    }
                }
            }

            // ProducerChain: Execute producer functions that return multiple types
            // e.g., create_lst() -> (AdminCap, CollectionFeeCap, LiquidStakingInfo)
            ConstructorChainEntry::ProducerChain { param_idx, chain, target_return_idx, is_ref: _, is_mut: _ } => {
                // Execute each step in the producer chain
                for step in &chain.steps {
                    let producer = &step.producer;

                    // Build args for the producer, using constructed_intermediates for dependencies
                    let args = match build_producer_args_with_intermediates(
                        &mut harness,
                        producer,
                        validator,
                        &constructed_intermediates,
                    ) {
                        Ok(args) => args,
                        Err(e) => {
                            report.failure_stage = Some(FailureStage::B1);
                            report.failure_reason =
                                Some(format!("producer chain arg build failed: {e}"));
                            return Ok(report);
                        }
                    };

                    let type_args: Vec<_> = (0..producer.type_param_count).map(|_| TypeTag::U64).collect();

                    let module_id = ModuleId::new(
                        producer.module_addr,
                        Identifier::new(producer.module_name.clone())
                            .unwrap_or_else(|_| Identifier::new("unknown").unwrap()),
                    );

                    // Execute producer function - may return multiple values
                    let returns = match harness.execute_function_with_return(
                        &module_id,
                        &producer.function_name,
                        type_args,
                        args,
                    ) {
                        Ok(r) => r,
                        Err(e) => {
                            report.failure_stage = Some(FailureStage::B1);
                            report.failure_reason =
                                Some(format!("producer execution failed: {e}"));
                            return Ok(report);
                        }
                    };

                    // Store ALL return values in intermediates (multi-return support)
                    for (ret_idx, (_, produced_type)) in producer.produces.iter().enumerate() {
                        if ret_idx < returns.len() {
                            constructed_intermediates.insert(
                                produced_type.type_key.clone(),
                                returns[ret_idx].clone(),
                            );
                        }
                    }
                }

                // Get the target return value from intermediates
                if let Some(final_bytes) = constructed_intermediates.get(&chain.target_type_key) {
                    if *param_idx < final_args.len() {
                        final_args[*param_idx] = final_bytes.clone();
                    } else {
                        report.failure_stage = Some(FailureStage::B1);
                        report.failure_reason = Some(format!(
                            "ProducerChain param_idx {} out of bounds (final_args len: {})",
                            param_idx,
                            final_args.len()
                        ));
                        return Ok(report);
                    }
                } else {
                    report.failure_stage = Some(FailureStage::B1);
                    report.failure_reason =
                        Some(format!("producer chain target type not found: {} (return_idx: {})",
                            chain.target_type_key, target_return_idx));
                    return Ok(report);
                }
            }

            // Synthesized entry - no execution needed, just use pre-computed bytes
            ConstructorChainEntry::Synthesized { param_idx, bytes, type_desc: _ } => {
                if *param_idx < final_args.len() {
                    final_args[*param_idx] = bytes.clone();
                } else {
                    report.failure_stage = Some(FailureStage::B1);
                    report.failure_reason = Some(format!(
                        "Synthesized param_idx {} out of bounds (final_args len: {})",
                        param_idx,
                        final_args.len()
                    ));
                    return Ok(report);
                }
                continue;
            }

            // Single-step entries (Intermediate, FinalParam, ConstructedRef)
            _ => {
                // Extract the constructor info from the entry
                let ctor = match entry {
                    ConstructorChainEntry::Intermediate(c) => c,
                    ConstructorChainEntry::FinalParam { ctor, .. } => ctor,
                    ConstructorChainEntry::ConstructedRef { ctor, .. } => ctor,
                    ConstructorChainEntry::MultiHopChain { .. } => unreachable!(),
                    ConstructorChainEntry::ProducerChain { .. } => unreachable!(),
                    ConstructorChainEntry::Synthesized { .. } => unreachable!(),
                };

                // Special case: coin::create_currency needs OTW handling
                let (ctor_type_args, ctor_args) = if ctor.function_name == "create_currency"
                    && ctor.module_id.address() == &AccountAddress::TWO
                {
                    // Get OTW type for create_currency
                    let otw = constructor_map
                        .get_first_otw()
                        .ok_or_else(|| anyhow!("no OTW type available for create_currency"))?;

                    // Type arg is the OTW type
                    let type_args = vec![otw.type_tag.clone()];

                    // Build args for create_currency:
                    // witness: T, decimals: u8, symbol: vector<u8>, name: vector<u8>,
                    // description: vector<u8>, icon_url: Option<Url>, ctx: &mut TxContext
                    let args = vec![
                        vec![1u8], // witness: OTW { dummy: true }
                        vec![9u8], // decimals: 9 like SUI
                        bcs_encode_vector(b"TEST"), // symbol
                        bcs_encode_vector(b"Test Token"), // name
                        bcs_encode_vector(b"Test token for type inhabitation"), // description
                        vec![0u8], // icon_url: None
                        harness.synthesize_tx_context()?, // ctx: &mut TxContext
                    ];

                    (type_args, args)
                } else {
                    // Normal constructor - pass constructed_intermediates to resolve struct params
                    let args = match build_constructor_args_with_intermediates(
                        &mut harness,
                        ctor,
                        validator,
                        &constructed_intermediates,
                    ) {
                        Ok(args) => args,
                        Err(e) => {
                            report.failure_stage = Some(FailureStage::B1);
                            report.failure_reason =
                                Some(format!("constructor arg build failed: {e}"));
                            return Ok(report);
                        }
                    };

                    let type_args: Vec<_> = (0..ctor.type_params).map(|_| TypeTag::U64).collect();

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
                    match entry {
                        ConstructorChainEntry::Intermediate(ctor) => {
                            // Intermediate value - store by return type for later use
                            let key = format!(
                                "{}::{}::{}",
                                ctor.returns.address.to_hex_literal(),
                                ctor.returns.module,
                                ctor.returns.name
                            );
                            constructed_intermediates.insert(key, constructed_bytes);
                        }
                        ConstructorChainEntry::FinalParam { param_idx, ctor: _ } => {
                            // Final param - store in final_args (with bounds check)
                            if *param_idx < final_args.len() {
                                final_args[*param_idx] = constructed_bytes;
                            } else {
                                report.failure_stage = Some(FailureStage::B1);
                                report.failure_reason = Some(format!(
                                    "constructor chain param_idx {} out of bounds (final_args len: {})",
                                    param_idx,
                                    final_args.len()
                                ));
                                return Ok(report);
                            }
                        }
                        ConstructorChainEntry::ConstructedRef {
                            param_idx,
                            ctor: _,
                            is_mut: _,
                        } => {
                            // ConstructedRef: We've constructed the value, and the VM will handle
                            // borrowing when we pass it as an argument. The constructed bytes ARE
                            // the value, and Move's calling convention will borrow from it.
                            if *param_idx < final_args.len() {
                                final_args[*param_idx] = constructed_bytes;
                            } else {
                                report.failure_stage = Some(FailureStage::B1);
                                report.failure_reason = Some(format!(
                                    "ConstructedRef param_idx {} out of bounds (final_args len: {})",
                                    param_idx,
                                    final_args.len()
                                ));
                                return Ok(report);
                            }
                        }
                        ConstructorChainEntry::MultiHopChain { .. } => unreachable!(),
                        ConstructorChainEntry::ProducerChain { .. } => unreachable!(),
                        ConstructorChainEntry::Synthesized { .. } => unreachable!(),
                    }
                } else {
                    report.failure_stage = Some(FailureStage::B1);
                    report.failure_reason = Some("constructor returned no value".to_string());
                    return Ok(report);
                }
            }
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
    let target_modules: Vec<String> = trace
        .modules_accessed
        .iter()
        .filter(|id| {
            let addr = id.address();
            // Exclude framework addresses
            *addr != AccountAddress::ONE &&  // 0x1 move-stdlib
            *addr != AccountAddress::TWO &&  // 0x2 sui-framework
            *addr != AccountAddress::from_hex_literal("0x3").unwrap_or(AccountAddress::ZERO)
            // sui-system
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
                target_modules_accessed: if target_modules.is_empty() {
                    None
                } else {
                    Some(target_modules)
                },
                static_calls: if non_framework_calls.is_empty() {
                    None
                } else {
                    Some(non_framework_calls)
                },
            });
        }
        Err(e) => {
            report.failure_stage = Some(FailureStage::B2);

            // Check for unsupported native error (E_NOT_SUPPORTED = 1000)
            let error_str = e.to_string();
            let failure_reason = if is_unsupported_native_error(&error_str) {
                unsupported_native_error_message()
            } else {
                format!("execution failed: {e}")
            };

            report.failure_reason = Some(failure_reason);
            report.tier_b_details = Some(TierBDetails {
                execution_success: false,
                error: Some(error_str),
                target_modules_accessed: if target_modules.is_empty() {
                    None
                } else {
                    Some(target_modules)
                },
                static_calls: if non_framework_calls.is_empty() {
                    None
                } else {
                    Some(non_framework_calls)
                },
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

/// Build arguments for a constructor function, with support for struct params
/// that have already been constructed (stored in intermediates).
fn build_constructor_args_with_intermediates(
    harness: &mut VMHarness,
    ctor: &ConstructorInfo,
    validator: &Validator,
    intermediates: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<Vec<Vec<u8>>> {
    let mut args = Vec::new();

    for param in &ctor.params {
        match param {
            ParamKind::Primitive(tag) => {
                // Generate default value for this primitive
                let layout = validator.resolve_type_layout(tag)?;
                let value = generate_default_value(&layout)
                    .ok_or_else(|| anyhow!("no default for primitive"))?;
                let bytes = value
                    .simple_serialize()
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
            ParamKind::Struct(struct_tag) => {
                // Look up the previously constructed value in intermediates
                let key = format!(
                    "{}::{}::{}",
                    struct_tag.address.to_hex_literal(),
                    struct_tag.module,
                    struct_tag.name
                );
                let bytes = intermediates
                    .get(&key)
                    .ok_or_else(|| anyhow!("intermediate struct {} not found", key))?;
                args.push(bytes.clone());
            }
            ParamKind::Unsupported(desc) => {
                return Err(anyhow!("unsupported param type: {}", desc));
            }
        }
    }

    Ok(args)
}

/// Build arguments for a producer function (uses ParamRequirement instead of ParamKind)
fn build_producer_args_with_intermediates(
    harness: &mut VMHarness,
    producer: &Producer,
    validator: &Validator,
    intermediates: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<Vec<Vec<u8>>> {
    let mut args = Vec::new();

    for param in &producer.params {
        match param {
            ParamRequirement::Primitive(type_str) => {
                // Convert type string to TypeTag and generate default
                let tag = match type_str.as_str() {
                    "bool" => TypeTag::Bool,
                    "u8" => TypeTag::U8,
                    "u16" => TypeTag::U16,
                    "u32" => TypeTag::U32,
                    "u64" => TypeTag::U64,
                    "u128" => TypeTag::U128,
                    "u256" => TypeTag::U256,
                    "address" => TypeTag::Address,
                    _ => return Err(anyhow!("unknown primitive type: {}", type_str)),
                };
                let layout = validator.resolve_type_layout(&tag)?;
                let value = generate_default_value(&layout)
                    .ok_or_else(|| anyhow!("no default for primitive {}", type_str))?;
                let bytes = value
                    .simple_serialize()
                    .ok_or_else(|| anyhow!("serialize failed"))?;
                args.push(bytes);
            }
            ParamRequirement::Vector(inner) => {
                // For vectors, check if inner is primitive
                if inner.is_synthesizable() {
                    args.push(vec![0]); // BCS encoding of empty vector
                } else {
                    return Err(anyhow!("unsupported vector type"));
                }
            }
            ParamRequirement::TxContext => {
                let ctx_bytes = harness.synthesize_tx_context()?;
                args.push(ctx_bytes);
            }
            ParamRequirement::Clock => {
                let clock_bytes = harness.synthesize_clock()?;
                args.push(clock_bytes);
            }
            ParamRequirement::TypeParam(_idx) => {
                // Type parameter instantiated with u64
                let bytes = 0u64.to_le_bytes().to_vec();
                args.push(bytes);
            }
            ParamRequirement::Type {
                module_addr,
                module_name,
                type_name,
            } => {
                // Look up previously constructed value
                let key = format!("{}::{}::{}", module_addr, module_name, type_name);
                let bytes = intermediates
                    .get(&key)
                    .ok_or_else(|| anyhow!("intermediate struct {} not found", key))?;
                args.push(bytes.clone());
            }
            ParamRequirement::Reference { inner, .. } => {
                // For references, handle the inner type
                match inner.as_ref() {
                    ParamRequirement::TxContext => {
                        let ctx_bytes = harness.synthesize_tx_context()?;
                        args.push(ctx_bytes);
                    }
                    ParamRequirement::Clock => {
                        let clock_bytes = harness.synthesize_clock()?;
                        args.push(clock_bytes);
                    }
                    ParamRequirement::Type {
                        module_addr,
                        module_name,
                        type_name,
                    } => {
                        let key = format!("{}::{}::{}", module_addr, module_name, type_name);
                        let bytes = intermediates
                            .get(&key)
                            .ok_or_else(|| anyhow!("intermediate struct {} not found for ref", key))?;
                        args.push(bytes.clone());
                    }
                    _ => {
                        return Err(anyhow!("unsupported reference inner type"));
                    }
                }
            }
            ParamRequirement::Unsupported(desc) => {
                return Err(anyhow!("unsupported param type: {}", desc));
            }
        }
    }

    Ok(args)
}
