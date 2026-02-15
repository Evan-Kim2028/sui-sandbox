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
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use std::collections::HashMap;
use std::str::FromStr;

use sui_sandbox_core::bootstrap::{
    create_child_fetcher, load_fetched_objects_into_env, preload_dynamic_field_objects,
};
use sui_sandbox_core::environment_bootstrap::{
    build_environment_from_hydrated_state, hydrate_mainnet_state, EnvironmentBuildPlan,
    MainnetHydrationPlan, MainnetObjectRequest,
};
use sui_sandbox_core::fetcher::GrpcFetcher;
use sui_sandbox_core::orchestrator::ReplayOrchestrator;
use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectChange};
use sui_sandbox_core::simulation::{ExecutionResult, FetcherConfig, SimulationEnvironment};
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

    let hydration = rt.block_on(hydrate_mainnet_state(&MainnetHydrationPlan {
        package_roots,
        objects: vec![MainnetObjectRequest {
            object_id: DEEPBOOK_REGISTRY.to_string(),
            version: None,
        }],
        historical_mode: false,
        allow_latest_object_fallback: true,
    }))?;
    let provider = &hydration.provider;
    let packages = &hydration.packages;
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

    let registry = hydration
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

    let sender = AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )?;
    let build = build_environment_from_hydrated_state(
        &hydration,
        &EnvironmentBuildPlan {
            sender,
            fail_on_object_load: true,
        },
    )?;
    let mut env: SimulationEnvironment = build.env;
    println!("  ✓ sender set to {}", sender.to_hex_literal());

    let registration = &build.package_registration;
    println!(
        "  ✓ package registration: loaded={}, skipped_upgraded={}, failed={}",
        registration.loaded,
        registration.skipped_upgraded,
        registration.failed.len()
    );
    if !registration.failed.is_empty() {
        for (addr, err) in &registration.failed {
            println!("    - failed {}: {}", addr.to_hex_literal(), err);
        }
        return Err(anyhow!("package registration failed"));
    }
    // Both storage package ids can appear (root + runtime). Force deterministic runtime
    // bytecode by explicitly loading the root package's modules at the runtime address.
    let root_addr = AccountAddress::from_hex_literal(DEEPBOOK_PACKAGE_ROOT)?;
    if let Some(root_pkg) = packages.get(&root_addr) {
        env.deploy_package_at_address(DEEPBOOK_RUNTIME_PACKAGE, root_pkg.modules.clone())?;
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
        build.objects_loaded
    );

    // Preload versioned dynamic-field wrappers under registry so pool creation can
    // read/write registry internals without missing child-object aborts.
    let df_wrappers = preload_dynamic_field_objects(
        &rt,
        provider.graphql(),
        provider.grpc(),
        &[DEEPBOOK_REGISTRY],
        32,
    );
    let df_loaded = load_fetched_objects_into_env(&mut env, &df_wrappers, false)?;
    println!(
        "  ✓ preloaded {} registry dynamic-field wrappers",
        df_loaded
    );

    // Keep on-demand fetchers enabled for any dynamic fields not preloaded.
    let grpc_endpoint = provider.grpc_endpoint().to_string();
    env = env.with_fetcher(
        Box::new(GrpcFetcher::custom(&grpc_endpoint)),
        FetcherConfig {
            enabled: true,
            network: Some("mainnet".to_string()),
            endpoint: Some(grpc_endpoint.clone()),
            use_archive: true,
        },
    );
    let child_grpc = rt.block_on(async {
        let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
        sui_transport::grpc::GrpcClient::with_api_key(&grpc_endpoint, api_key).await
    })?;
    env.set_child_fetcher(create_child_fetcher(child_grpc, HashMap::new(), None));

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

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 4: PTB #1 Create permissionless SUI/STABLECOIN pool");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let create_pool_inputs = vec![
        ReplayOrchestrator::shared_object_input(&env, DEEPBOOK_REGISTRY, true)?,
        ReplayOrchestrator::pure_input(TICK_SIZE)?,
        ReplayOrchestrator::pure_input(LOT_SIZE)?,
        ReplayOrchestrator::pure_input(MIN_SIZE)?,
        ReplayOrchestrator::owned_object_input(&env, &deep_coin_id.to_hex_literal())?,
    ];

    let create_pool_cmds = vec![Command::MoveCall {
        package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
        module: Identifier::new("pool")?,
        function: Identifier::new("create_permissionless_pool")?,
        type_args: vec![base_tag.clone(), quote_tag.clone()],
        args: vec![
            Argument::Input(0),
            Argument::Input(1),
            Argument::Input(2),
            Argument::Input(3),
            Argument::Input(4),
        ],
    }];

    let create_pool_result = env.execute_ptb(create_pool_inputs, create_pool_cmds);
    ReplayOrchestrator::ensure_execution_success(
        "create_permissionless_pool",
        &create_pool_result,
    )?;
    let mut pool_id =
        ReplayOrchestrator::decode_execution_command_return_object_id(&create_pool_result, 0, 0)?;
    if env.get_object(&pool_id).is_none() {
        if let Ok(fallback_pool_id) = ReplayOrchestrator::find_created_object_id_by_struct_tag(
            &env,
            &create_pool_result,
            "pool",
            "Pool",
        ) {
            println!(
                "  ✓ return ID {} is logical pool_id; using created Pool object {}",
                pool_id.to_hex_literal(),
                fallback_pool_id.to_hex_literal()
            );
            pool_id = fallback_pool_id;
        } else if let Ok(recovered_pool_id) = ReplayOrchestrator::recover_created_object_into_env(
            &mut env,
            &create_pool_result,
            "pool",
            "Pool",
            true,
            false,
            1,
        ) {
            println!(
                "  ✓ recovered pool object {} from PTB effects",
                recovered_pool_id.to_hex_literal()
            );
            pool_id = recovered_pool_id;
        } else if let Ok(synth_pool_id) = synthesize_pool_object_from_dynamic_field(
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
    if let Some(effects) = &create_pool_result.effects {
        println!("  ✓ gas used: {}", effects.gas_used);
    }

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 5: PTB #2 Create manager, deposit, place bid+ask orders");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let expiry_ms = env.get_clock_timestamp_ms() + 86_400_000;

    let place_orders_inputs = vec![
        ReplayOrchestrator::shared_object_input(&env, &pool_id.to_hex_literal(), true)?, // 0 pool
        InputValue::Object(env.get_clock_object()?),                                     // 1 clock
        ReplayOrchestrator::owned_object_input(&env, &base_coin_id.to_hex_literal())?, // 2 base coin
        ReplayOrchestrator::owned_object_input(&env, &quote_coin_id.to_hex_literal())?, // 3 quote coin
        ReplayOrchestrator::pure_input(101u64)?, // 4 bid client_order_id
        ReplayOrchestrator::pure_input(0u8)?,    // 5 no_restriction
        ReplayOrchestrator::pure_input(0u8)?,    // 6 self_matching_allowed
        ReplayOrchestrator::pure_input(1_000u64)?, // 7 bid price
        ReplayOrchestrator::pure_input(1_000u64)?, // 8 bid qty
        ReplayOrchestrator::pure_input(true)?,   // 9 is_bid
        ReplayOrchestrator::pure_input(false)?,  // 10 pay_with_deep
        ReplayOrchestrator::pure_input(expiry_ms)?, // 11 expiry
        ReplayOrchestrator::pure_input(202u64)?, // 12 ask client_order_id
        ReplayOrchestrator::pure_input(0u8)?,    // 13 no_restriction
        ReplayOrchestrator::pure_input(0u8)?,    // 14 self_matching_allowed
        ReplayOrchestrator::pure_input(2_000u64)?, // 15 ask price
        ReplayOrchestrator::pure_input(1_000u64)?, // 16 ask qty
        ReplayOrchestrator::pure_input(false)?,  // 17 is_bid (ask)
        ReplayOrchestrator::pure_input(false)?,  // 18 pay_with_deep
        ReplayOrchestrator::pure_input(expiry_ms)?, // 19 expiry
    ];

    let place_orders_cmds = vec![
        Command::MoveCall {
            package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
            module: Identifier::new("balance_manager")?,
            function: Identifier::new("new")?,
            type_args: vec![],
            args: vec![],
        },
        Command::MoveCall {
            package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
            module: Identifier::new("balance_manager")?,
            function: Identifier::new("mint_trade_cap")?,
            type_args: vec![],
            args: vec![Argument::Result(0)],
        },
        Command::MoveCall {
            package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
            module: Identifier::new("balance_manager")?,
            function: Identifier::new("generate_proof_as_trader")?,
            type_args: vec![],
            args: vec![Argument::Result(0), Argument::Result(1)],
        },
        Command::MoveCall {
            package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
            module: Identifier::new("balance_manager")?,
            function: Identifier::new("deposit")?,
            type_args: vec![base_tag.clone()],
            args: vec![Argument::Result(0), Argument::Input(2)],
        },
        Command::MoveCall {
            package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
            module: Identifier::new("balance_manager")?,
            function: Identifier::new("deposit")?,
            type_args: vec![quote_tag.clone()],
            args: vec![Argument::Result(0), Argument::Input(3)],
        },
        Command::MoveCall {
            package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
            module: Identifier::new("pool")?,
            function: Identifier::new("place_limit_order")?,
            type_args: vec![base_tag.clone(), quote_tag.clone()],
            args: vec![
                Argument::Input(0),
                Argument::Result(0),
                Argument::Result(2),
                Argument::Input(4),
                Argument::Input(5),
                Argument::Input(6),
                Argument::Input(7),
                Argument::Input(8),
                Argument::Input(9),
                Argument::Input(10),
                Argument::Input(11),
                Argument::Input(1),
            ],
        },
        Command::MoveCall {
            package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
            module: Identifier::new("pool")?,
            function: Identifier::new("place_limit_order")?,
            type_args: vec![base_tag.clone(), quote_tag.clone()],
            args: vec![
                Argument::Input(0),
                Argument::Result(0),
                Argument::Result(2),
                Argument::Input(12),
                Argument::Input(13),
                Argument::Input(14),
                Argument::Input(15),
                Argument::Input(16),
                Argument::Input(17),
                Argument::Input(18),
                Argument::Input(19),
                Argument::Input(1),
            ],
        },
    ];

    let place_orders_result = env.execute_ptb(place_orders_inputs, place_orders_cmds);
    ReplayOrchestrator::ensure_execution_success("place_limit_order flow", &place_orders_result)?;
    let manager_id = ReplayOrchestrator::find_created_object_id_by_struct_tag(
        &env,
        &place_orders_result,
        "balance_manager",
        "BalanceManager",
    )?;
    println!("  ✓ manager id: {}", manager_id.to_hex_literal());
    if let Some(effects) = &place_orders_result.effects {
        println!("  ✓ gas used: {}", effects.gas_used);
        println!("  ✓ created objects in PTB #2: {}", effects.created.len());
    }

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 6: PTB #3 Query locked balances");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let locked_query_inputs = vec![
        ReplayOrchestrator::shared_object_input(&env, &pool_id.to_hex_literal(), false)?,
        ReplayOrchestrator::immutable_object_input(&env, &manager_id.to_hex_literal())?,
    ];
    let locked_query_cmds = vec![Command::MoveCall {
        package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
        module: Identifier::new("pool")?,
        function: Identifier::new("locked_balance")?,
        type_args: vec![base_tag, quote_tag],
        args: vec![Argument::Input(0), Argument::Input(1)],
    }];
    let locked_result = env.execute_ptb(locked_query_inputs, locked_query_cmds);
    ReplayOrchestrator::ensure_execution_success("locked_balance", &locked_result)?;

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
    let effects = result
        .effects
        .as_ref()
        .ok_or_else(|| anyhow!("missing effects"))?;

    let versioned_parent = effects
        .dynamic_field_entries
        .iter()
        .find_map(|((parent, _child), (type_tag, _bytes))| {
            let type_str = type_tag.to_canonical_string(true);
            if type_str.contains("::pool::PoolInner<")
                && type_str.contains("::dynamic_field::Field<")
            {
                Some(*parent)
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow!("pool inner dynamic field not found in effects"))?;

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
