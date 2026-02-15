//! DeepBook Spot: Offline PTB Pool Creation + Order Placement
//!
//! This example shows a full offline PTB flow against real DeepBook bytecode:
//! 1. Fetch DeepBook package graph + registry object from mainnet.
//! 2. Load everything into local `SimulationEnvironment`.
//! 3. Create a new permissionless SUI/STABLECOIN pool.
//! 4. Create a balance manager, deposit synthetic balances, and place orders.
//! 5. Query locked balances to verify order state changed.
//!
//! Run:
//! ```bash
//! cargo run --example deepbook_spot_offline_ptb
//! ```

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::str::FromStr;

use sui_sandbox_core::bootstrap::{
    deploy_package_alias_if_present, ensure_package_registration_success,
};
use sui_sandbox_core::environment_bootstrap::{
    hydrate_build_and_finalize_mainnet_environment, EnvironmentBuildPlan, EnvironmentFinalizePlan,
    MainnetHydrationPlan, MainnetObjectRequest,
};
use sui_sandbox_core::orchestrator::ReplayOrchestrator;
use sui_sandbox_core::ptb::{InputValue, ObjectChange};
use sui_sandbox_core::simulation::{ExecutionResult, SimulationEnvironment};
use sui_sandbox_core::utilities::collect_required_package_roots_from_type_strings;

const DEEPBOOK_PACKAGE_ROOT: &str =
    "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497";
const DEEPBOOK_RUNTIME_PACKAGE: &str =
    "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809";
const DEEPBOOK_REGISTRY: &str =
    "0xaf16199a2dff736e9f07a845f23c5da6df6f756eddb631aed9d24a93efc4549d";

const DEEP_TYPE: &str =
    "0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270::deep::DEEP";
const SUI_TYPE: &str = "0x2::sui::SUI";
const QUOTE_TYPE: &str =
    "0xecf47609d7da919ea98e7fd04f6e0648a0a79b337aaad373fa37aac8febf19c8::stablecoin::STABLECOIN";

// DeepBook pool::create_permissionless_pool constraints:
// - pool_creation_fee() == 500_000_000 (DEEP smallest units)
// - tick_size: power of 10, > 0
// - lot_size: power of 10, >= 1000
// - min_size: power of 10, > 0, and multiple of lot_size
const POOL_CREATION_FEE_DEEP: u64 = 500_000_000;
const TICK_SIZE: u64 = 10;
const LOT_SIZE: u64 = 1_000;
const MIN_SIZE: u64 = 1_000;

fn main() -> Result<()> {
    dotenv::dotenv().ok();
    print_header();

    let rt = tokio::runtime::Runtime::new()?;

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 1: Fetch package graph + registry state");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let explicit_roots = vec![AccountAddress::from_hex_literal(DEEPBOOK_PACKAGE_ROOT)?];
    let type_roots = vec![
        SUI_TYPE.to_string(),
        QUOTE_TYPE.to_string(),
        DEEP_TYPE.to_string(),
    ];
    let package_roots: Vec<AccountAddress> =
        collect_required_package_roots_from_type_strings(&explicit_roots, &type_roots)?
            .into_iter()
            .collect();

    let sender = AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )?;
    let bootstrap = rt.block_on(hydrate_build_and_finalize_mainnet_environment(
        &MainnetHydrationPlan {
            package_roots,
            objects: vec![MainnetObjectRequest {
                object_id: DEEPBOOK_REGISTRY.to_string(),
                version: None,
            }],
            historical_mode: false,
            allow_latest_object_fallback: true,
        },
        &EnvironmentBuildPlan {
            sender,
            fail_on_object_load: true,
        },
        &EnvironmentFinalizePlan {
            dynamic_field_parents: vec![DEEPBOOK_REGISTRY.to_string()],
            dynamic_field_limit: 32,
            configure_fetchers: true,
        },
    ))?;
    let provider = &bootstrap.hydration.provider;
    let packages = &bootstrap.hydration.packages;
    println!("  ✓ gRPC endpoint: {}", provider.grpc_endpoint());
    println!(
        "  ✓ fetched {} packages with dependency closure",
        packages.len()
    );
    for (addr, pkg) in packages {
        let orig = pkg
            .original_id
            .map(|v| v.to_hex_literal())
            .unwrap_or_else(|| "None".to_string());
        println!(
            "    - {} (orig={}, v{}, modules={})",
            addr.to_hex_literal(),
            orig,
            pkg.version,
            pkg.modules.len()
        );
    }

    let registry = bootstrap
        .hydration
        .objects
        .get(DEEPBOOK_REGISTRY)
        .ok_or_else(|| anyhow!("failed to fetch DeepBook registry object"))?;
    println!(
        "  ✓ registry loaded at version {} (shared={})",
        registry.2, registry.3
    );

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 2: Build local sandbox state");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let mut env: SimulationEnvironment = bootstrap.build.env;
    println!("  ✓ sender set to {}", sender.to_hex_literal());

    let registration = &bootstrap.build.package_registration;
    println!(
        "  ✓ package registration: loaded={}, skipped_upgraded={}, failed={}",
        registration.loaded,
        registration.skipped_upgraded,
        registration.failed.len()
    );
    ensure_package_registration_success(registration)?;

    // Both storage package ids can appear (root + runtime). Force deterministic runtime
    // bytecode by explicitly loading the root package's modules at the runtime address.
    let root_addr = AccountAddress::from_hex_literal(DEEPBOOK_PACKAGE_ROOT)?;
    if deploy_package_alias_if_present(&mut env, packages, root_addr, DEEPBOOK_RUNTIME_PACKAGE)? {
        println!(
            "  ✓ normalized runtime modules from {} -> {}",
            DEEPBOOK_PACKAGE_ROOT, DEEPBOOK_RUNTIME_PACKAGE
        );
    }

    if let Some(funcs) = env.list_functions(&format!("{}::pool", DEEPBOOK_RUNTIME_PACKAGE)) {
        println!(
            "  ✓ runtime pool module functions loaded: {} (has create_permissionless_pool={})",
            funcs.len(),
            funcs.iter().any(|f| f == "create_permissionless_pool")
        );
    } else {
        println!(
            "  ⚠ runtime module {}::pool not found after registration",
            DEEPBOOK_RUNTIME_PACKAGE
        );
    }
    let pool_inner_version = query_current_pool_inner_version(&mut env)?;
    println!("  ✓ pool inner current_version: {}", pool_inner_version);
    println!(
        "  ✓ registry object loaded into environment (objects_loaded={})",
        bootstrap.build.objects_loaded
    );
    println!(
        "  ✓ preloaded {} registry dynamic-field wrappers",
        bootstrap.finalize.dynamic_fields_loaded
    );
    println!(
        "  ✓ on-demand fetchers configured: {}",
        bootstrap.finalize.fetchers_configured
    );

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 3: Create synthetic balances (offline)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let deep_coin_id = env.create_coin(DEEP_TYPE, POOL_CREATION_FEE_DEEP)?;
    let base_coin_id = env.create_coin(SUI_TYPE, 10_000_000_000)?; // 10 SUI
    let quote_coin_id = env.create_coin(QUOTE_TYPE, 10_000_000_000)?; // synthetic quote balance
    println!(
        "  ✓ created DEEP fee coin: {}",
        deep_coin_id.to_hex_literal()
    );
    println!(
        "  ✓ created base coin:      {}",
        base_coin_id.to_hex_literal()
    );
    println!(
        "  ✓ created quote coin:     {}",
        quote_coin_id.to_hex_literal()
    );

    let base_tag = TypeTag::from_str(SUI_TYPE)?;
    let quote_tag = TypeTag::from_str(QUOTE_TYPE)?;
    let deepbook_runtime = AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?;

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 4: PTB #1 Create permissionless SUI/STABLECOIN pool");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let mut create_pool_ptb = ReplayOrchestrator::ptb_builder();
    let registry_input = create_pool_ptb.shared_object_from_env(&env, DEEPBOOK_REGISTRY, true)?;
    let tick_input = create_pool_ptb.pure(TICK_SIZE)?;
    let lot_input = create_pool_ptb.pure(LOT_SIZE)?;
    let min_input = create_pool_ptb.pure(MIN_SIZE)?;
    let deep_fee_coin_input =
        create_pool_ptb.owned_object_from_env(&env, &deep_coin_id.to_hex_literal())?;
    create_pool_ptb.move_call(
        ReplayOrchestrator::move_call_builder(
            deepbook_runtime,
            "pool",
            "create_permissionless_pool",
        )
        .with_type_args([base_tag.clone(), quote_tag.clone()])
        .with_args([
            registry_input,
            tick_input,
            lot_input,
            min_input,
            deep_fee_coin_input,
        ]),
    )?;
    let create_pool_exec = ReplayOrchestrator::execute_ptb_with_summary(
        &mut env,
        "create_permissionless_pool",
        create_pool_ptb,
    )?;
    let create_pool_summary = create_pool_exec.summary;
    let create_pool_result = create_pool_exec.result;
    let mut pool_id = ReplayOrchestrator::resolve_created_object_id_from_return_or_effects(
        &mut env,
        &create_pool_result,
        0,
        0,
        "pool",
        "Pool",
        true,
        false,
        1,
    )?;
    if env.get_object(&pool_id).is_none() {
        if let Ok(synth_pool_id) = synthesize_pool_object_from_dynamic_field(
            &mut env,
            &create_pool_result,
            pool_id,
            pool_inner_version,
        ) {
            println!(
                "  ✓ synthesized pool object {} from dynamic-field parent",
                synth_pool_id.to_hex_literal()
            );
            pool_id = synth_pool_id;
        }
    }
    if env.get_object(&pool_id).is_none() {
        if let Some(effects) = &create_pool_result.effects {
            println!("  ! debug: created ids = {}", effects.created.len());
            for id in &effects.created {
                println!("    - created {}", id.to_hex_literal());
            }
            println!(
                "  ! debug: object changes = {}",
                effects.object_changes.len()
            );
            for change in &effects.object_changes {
                match change {
                    ObjectChange::Created {
                        id, object_type, ..
                    } => {
                        let type_str = object_type
                            .as_ref()
                            .map(|t| t.to_canonical_string(true))
                            .unwrap_or_else(|| "None".to_string());
                        let has_bytes = effects.created_object_bytes.contains_key(id);
                        println!(
                            "    - created change {} type={} has_created_bytes={}",
                            id.to_hex_literal(),
                            type_str,
                            has_bytes
                        );
                    }
                    ObjectChange::Mutated {
                        id, object_type, ..
                    } => {
                        let type_str = object_type
                            .as_ref()
                            .map(|t| t.to_canonical_string(true))
                            .unwrap_or_else(|| "None".to_string());
                        println!(
                            "    - mutated change {} type={}",
                            id.to_hex_literal(),
                            type_str
                        );
                    }
                    _ => {}
                }
            }
            println!(
                "  ! debug: dynamic field entries = {}",
                effects.dynamic_field_entries.len()
            );
            for ((parent, child), (type_tag, bytes)) in &effects.dynamic_field_entries {
                println!(
                    "    - df parent={} child={} type={} bytes={}",
                    parent.to_hex_literal(),
                    child.to_hex_literal(),
                    type_tag.to_canonical_string(true),
                    bytes.len()
                );
            }
        }
        return Err(anyhow!(
            "pool object not present in environment after creation (id={})",
            pool_id.to_hex_literal()
        ));
    }
    println!("  ✓ created pool id: {}", pool_id.to_hex_literal());
    println!("  ✓ gas used: {}", create_pool_summary.gas_used);

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 5: PTB #2 Create manager, deposit, place bid+ask orders");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let expiry_ms = env.get_clock_timestamp_ms() + 86_400_000;

    let mut place_orders_ptb = ReplayOrchestrator::ptb_builder();
    let pool_input =
        place_orders_ptb.shared_object_from_env(&env, &pool_id.to_hex_literal(), true)?;
    let clock_input = place_orders_ptb.input(InputValue::Object(env.get_clock_object()?))?;
    let base_coin_input =
        place_orders_ptb.owned_object_from_env(&env, &base_coin_id.to_hex_literal())?;
    let quote_coin_input =
        place_orders_ptb.owned_object_from_env(&env, &quote_coin_id.to_hex_literal())?;
    let bid_client_order_id = place_orders_ptb.pure(101u64)?;
    let bid_no_restriction = place_orders_ptb.pure(0u8)?;
    let bid_self_matching_allowed = place_orders_ptb.pure(0u8)?;
    let bid_price = place_orders_ptb.pure(1_000u64)?;
    let bid_qty = place_orders_ptb.pure(1_000u64)?;
    let bid_is_bid = place_orders_ptb.pure(true)?;
    let bid_pay_with_deep = place_orders_ptb.pure(false)?;
    let bid_expiry = place_orders_ptb.pure(expiry_ms)?;
    let ask_client_order_id = place_orders_ptb.pure(202u64)?;
    let ask_no_restriction = place_orders_ptb.pure(0u8)?;
    let ask_self_matching_allowed = place_orders_ptb.pure(0u8)?;
    let ask_price = place_orders_ptb.pure(2_000u64)?;
    let ask_qty = place_orders_ptb.pure(1_000u64)?;
    let ask_is_bid = place_orders_ptb.pure(false)?;
    let ask_pay_with_deep = place_orders_ptb.pure(false)?;
    let ask_expiry = place_orders_ptb.pure(expiry_ms)?;

    let manager_result = place_orders_ptb.move_call(ReplayOrchestrator::move_call_builder(
        deepbook_runtime,
        "balance_manager",
        "new",
    ))?;
    let trade_cap_result = place_orders_ptb.move_call(
        ReplayOrchestrator::move_call_builder(
            deepbook_runtime,
            "balance_manager",
            "mint_trade_cap",
        )
        .with_args([manager_result]),
    )?;
    let proof_result = place_orders_ptb.move_call(
        ReplayOrchestrator::move_call_builder(
            deepbook_runtime,
            "balance_manager",
            "generate_proof_as_trader",
        )
        .with_args([manager_result, trade_cap_result]),
    )?;
    place_orders_ptb.move_call(
        ReplayOrchestrator::move_call_builder(deepbook_runtime, "balance_manager", "deposit")
            .with_type_args([base_tag.clone()])
            .with_args([manager_result, base_coin_input]),
    )?;
    place_orders_ptb.move_call(
        ReplayOrchestrator::move_call_builder(deepbook_runtime, "balance_manager", "deposit")
            .with_type_args([quote_tag.clone()])
            .with_args([manager_result, quote_coin_input]),
    )?;
    place_orders_ptb.move_call(
        ReplayOrchestrator::move_call_builder(deepbook_runtime, "pool", "place_limit_order")
            .with_type_args([base_tag.clone(), quote_tag.clone()])
            .with_args([
                pool_input,
                manager_result,
                proof_result,
                bid_client_order_id,
                bid_no_restriction,
                bid_self_matching_allowed,
                bid_price,
                bid_qty,
                bid_is_bid,
                bid_pay_with_deep,
                bid_expiry,
                clock_input,
            ]),
    )?;
    place_orders_ptb.move_call(
        ReplayOrchestrator::move_call_builder(deepbook_runtime, "pool", "place_limit_order")
            .with_type_args([base_tag.clone(), quote_tag.clone()])
            .with_args([
                pool_input,
                manager_result,
                proof_result,
                ask_client_order_id,
                ask_no_restriction,
                ask_self_matching_allowed,
                ask_price,
                ask_qty,
                ask_is_bid,
                ask_pay_with_deep,
                ask_expiry,
                clock_input,
            ]),
    )?;

    let place_orders_exec = ReplayOrchestrator::execute_ptb_with_summary(
        &mut env,
        "place_limit_order flow",
        place_orders_ptb,
    )?;
    let place_orders_summary = place_orders_exec.summary;
    let place_orders_result = place_orders_exec.result;
    let manager_id = ReplayOrchestrator::resolve_created_object_id_from_return_or_effects(
        &mut env,
        &place_orders_result,
        0,
        0,
        "balance_manager",
        "BalanceManager",
        false,
        true,
        1,
    )?;
    println!("  ✓ manager id: {}", manager_id.to_hex_literal());
    println!("  ✓ gas used: {}", place_orders_summary.gas_used);
    println!(
        "  ✓ created objects in PTB #2: {}",
        place_orders_summary.created_objects
    );

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 6: PTB #3 Query locked balances");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let mut locked_query_ptb = ReplayOrchestrator::ptb_builder();
    let locked_pool_input =
        locked_query_ptb.shared_object_from_env(&env, &pool_id.to_hex_literal(), false)?;
    let locked_manager_input =
        locked_query_ptb.immutable_object_from_env(&env, &manager_id.to_hex_literal())?;
    locked_query_ptb.move_call(
        ReplayOrchestrator::move_call_builder(deepbook_runtime, "pool", "locked_balance")
            .with_type_args([base_tag, quote_tag])
            .with_args([locked_pool_input, locked_manager_input]),
    )?;
    let locked_exec =
        ReplayOrchestrator::execute_ptb_with_summary(&mut env, "locked_balance", locked_query_ptb)?;
    let locked_result = locked_exec.result;

    let (base_locked, quote_locked, deep_locked) =
        ReplayOrchestrator::decode_execution_command_return_u64_triplet(&locked_result, 0)?;
    println!("  ✓ locked base:  {}", base_locked);
    println!("  ✓ locked quote: {}", quote_locked);
    println!("  ✓ locked DEEP:  {}", deep_locked);

    println!("\n✅ DeepBook spot offline PTB flow completed successfully.");
    println!("   - Pool created");
    println!("   - Two limit orders placed (bid + ask)");
    println!("   - Post-order locked balances queried locally");

    Ok(())
}

fn query_current_pool_inner_version(env: &mut SimulationEnvironment) -> Result<u64> {
    let result = ReplayOrchestrator::execute_noarg_move_call(
        env,
        AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
        "constants",
        "current_version",
    )?;
    ReplayOrchestrator::ensure_execution_success("constants::current_version", &result)?;
    ReplayOrchestrator::decode_execution_command_return_u64(&result, 0, 0)
}

fn synthesize_pool_object_from_dynamic_field(
    env: &mut SimulationEnvironment,
    result: &ExecutionResult,
    pool_id: AccountAddress,
    current_version: u64,
) -> Result<AccountAddress> {
    let versioned_parent = ReplayOrchestrator::find_dynamic_field_parent_by_type_substrings(
        result,
        &["::pool::PoolInner<", "::dynamic_field::Field<"],
    )?;

    // Pool layout:
    //   id: UID                -> 32 bytes (pool id)
    //   inner: versioned::Versioned { id: UID, version: u64 }
    let mut pool_bytes = Vec::with_capacity(72);
    pool_bytes.extend_from_slice(pool_id.as_ref());
    pool_bytes.extend_from_slice(versioned_parent.as_ref());
    pool_bytes.extend_from_slice(&current_version.to_le_bytes());

    let pool_type = format!(
        "{}::pool::Pool<{},{}>",
        DEEPBOOK_RUNTIME_PACKAGE, SUI_TYPE, QUOTE_TYPE
    );
    env.load_object_from_data(
        &pool_id.to_hex_literal(),
        pool_bytes,
        Some(&pool_type),
        true,
        false,
        1,
    )?;
    Ok(pool_id)
}

fn print_header() {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║             DeepBook Spot Offline PTB Example                        ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║  Creates a permissionless pool + places spot orders locally          ║");
    println!("║  using fetched mainnet bytecode/state and synthetic balances.        ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!();
}
