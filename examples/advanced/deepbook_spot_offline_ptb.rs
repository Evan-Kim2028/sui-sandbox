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

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use std::collections::HashMap;
use std::str::FromStr;

use sui_sandbox_core::fetcher::GrpcFetcher;
use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectChange, ObjectInput};
use sui_sandbox_core::simulation::{ExecutionResult, FetcherConfig, SimulationEnvironment};
use sui_sandbox_core::utilities::collect_required_package_roots_from_type_strings;

#[path = "../common/mod.rs"]
mod common;
use common::create_child_fetcher;

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

    let provider = rt.block_on(common::create_mainnet_provider(false))?;
    println!("  ✓ gRPC endpoint: {}", provider.grpc_endpoint());

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

    let packages = rt.block_on(async {
        provider
            .fetch_packages_with_deps(&package_roots, None, None)
            .await
    })?;
    println!(
        "  ✓ fetched {} packages with dependency closure",
        packages.len()
    );
    for (addr, pkg) in &packages {
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

    let registry = common::fetch_object_data(&rt, provider.grpc(), DEEPBOOK_REGISTRY, None, false)
        .ok_or_else(|| anyhow!("failed to fetch DeepBook registry object"))?;
    println!(
        "  ✓ registry loaded at version {} (shared={})",
        registry.2, registry.3
    );

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 2: Build local sandbox state");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let mut env = SimulationEnvironment::new()?;
    let sender = AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )?;
    env.set_sender(sender);
    println!("  ✓ sender set to {}", sender.to_hex_literal());

    let registration_plan = common::build_package_registration_plan(&packages);
    let registration =
        common::register_packages_with_linkage_plan(&mut env, &packages, &registration_plan);
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

    env.load_object_from_data(
        DEEPBOOK_REGISTRY,
        registry.0.clone(),
        registry.1.as_deref(),
        registry.3,
        false,
        registry.2,
    )?;
    println!("  ✓ registry object loaded into environment");

    // Preload versioned dynamic-field wrappers under registry so pool creation can
    // read/write registry internals without missing child-object aborts.
    let df_wrappers = common::preload_dynamic_field_objects(
        &rt,
        provider.graphql(),
        provider.grpc(),
        &[DEEPBOOK_REGISTRY],
        32,
    );
    let df_loaded = common::load_fetched_objects_into_env(&mut env, &df_wrappers, false)?;
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
        shared_object_input(&env, DEEPBOOK_REGISTRY, true)?,
        pure_input(TICK_SIZE)?,
        pure_input(LOT_SIZE)?,
        pure_input(MIN_SIZE)?,
        owned_object_input(&env, &deep_coin_id.to_hex_literal())?,
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
    ensure_success("create_permissionless_pool", &create_pool_result)?;
    let mut pool_id = decode_first_return_object_id(&create_pool_result)?;
    if env.get_object(&pool_id).is_none() {
        if let Ok(fallback_pool_id) = find_created_pool_id(&env, &create_pool_result) {
            println!(
                "  ✓ return ID {} is logical pool_id; using created Pool object {}",
                pool_id.to_hex_literal(),
                fallback_pool_id.to_hex_literal()
            );
            pool_id = fallback_pool_id;
        } else if let Ok(recovered_pool_id) =
            recover_created_pool_object(&mut env, &create_pool_result)
        {
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
        shared_object_input(&env, &pool_id.to_hex_literal(), true)?, // 0 pool
        InputValue::Object(env.get_clock_object()?),                 // 1 clock
        owned_object_input(&env, &base_coin_id.to_hex_literal())?,   // 2 base coin
        owned_object_input(&env, &quote_coin_id.to_hex_literal())?,  // 3 quote coin
        pure_input(101u64)?,                                         // 4 bid client_order_id
        pure_input(0u8)?,                                            // 5 no_restriction
        pure_input(0u8)?,                                            // 6 self_matching_allowed
        pure_input(1_000u64)?,                                       // 7 bid price
        pure_input(1_000u64)?,                                       // 8 bid qty
        pure_input(true)?,                                           // 9 is_bid
        pure_input(false)?,                                          // 10 pay_with_deep
        pure_input(expiry_ms)?,                                      // 11 expiry
        pure_input(202u64)?,                                         // 12 ask client_order_id
        pure_input(0u8)?,                                            // 13 no_restriction
        pure_input(0u8)?,                                            // 14 self_matching_allowed
        pure_input(2_000u64)?,                                       // 15 ask price
        pure_input(1_000u64)?,                                       // 16 ask qty
        pure_input(false)?,                                          // 17 is_bid (ask)
        pure_input(false)?,                                          // 18 pay_with_deep
        pure_input(expiry_ms)?,                                      // 19 expiry
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
    ensure_success("place_limit_order flow", &place_orders_result)?;
    let manager_id = find_created_balance_manager_id(&env, &place_orders_result)?;
    println!("  ✓ manager id: {}", manager_id.to_hex_literal());
    if let Some(effects) = &place_orders_result.effects {
        println!("  ✓ gas used: {}", effects.gas_used);
        println!("  ✓ created objects in PTB #2: {}", effects.created.len());
    }

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 6: PTB #3 Query locked balances");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let locked_query_inputs = vec![
        shared_object_input(&env, &pool_id.to_hex_literal(), false)?,
        imm_object_input(&env, &manager_id.to_hex_literal())?,
    ];
    let locked_query_cmds = vec![Command::MoveCall {
        package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
        module: Identifier::new("pool")?,
        function: Identifier::new("locked_balance")?,
        type_args: vec![base_tag, quote_tag],
        args: vec![Argument::Input(0), Argument::Input(1)],
    }];
    let locked_result = env.execute_ptb(locked_query_inputs, locked_query_cmds);
    ensure_success("locked_balance", &locked_result)?;

    let (base_locked, quote_locked, deep_locked) = decode_first_return_u64_triplet(&locked_result)?;
    println!("  ✓ locked base:  {}", base_locked);
    println!("  ✓ locked quote: {}", quote_locked);
    println!("  ✓ locked DEEP:  {}", deep_locked);

    println!("\n✅ DeepBook spot offline PTB flow completed successfully.");
    println!("   - Pool created");
    println!("   - Two limit orders placed (bid + ask)");
    println!("   - Post-order locked balances queried locally");

    Ok(())
}

fn pure_input<T: serde::Serialize>(value: T) -> Result<InputValue> {
    Ok(InputValue::Pure(bcs::to_bytes(&value)?))
}

fn owned_object_input(env: &SimulationEnvironment, object_id: &str) -> Result<InputValue> {
    Ok(InputValue::Object(
        env.get_object_for_ptb_with_mode(object_id, Some("owned"))?,
    ))
}

fn imm_object_input(env: &SimulationEnvironment, object_id: &str) -> Result<InputValue> {
    Ok(InputValue::Object(env.get_object_for_ptb_with_mode(
        object_id,
        Some("immutable"),
    )?))
}

fn shared_object_input(
    env: &SimulationEnvironment,
    object_id: &str,
    mutable: bool,
) -> Result<InputValue> {
    let id = AccountAddress::from_hex_literal(object_id)
        .with_context(|| format!("invalid object id: {object_id}"))?;
    let obj = env
        .get_object(&id)
        .ok_or_else(|| anyhow!("object not found in env: {object_id}"))?;
    Ok(InputValue::Object(ObjectInput::Shared {
        id,
        bytes: obj.bcs_bytes.clone(),
        type_tag: Some(obj.type_tag.clone()),
        version: Some(obj.version),
        mutable,
    }))
}

fn ensure_success(step: &str, result: &ExecutionResult) -> Result<()> {
    if result.success {
        return Ok(());
    }

    let mut msg = format!("{step} failed");
    if let Some(err) = &result.error {
        msg.push_str(&format!("; error={err}"));
    }
    if let Some(raw) = &result.raw_error {
        msg.push_str(&format!("; raw={raw}"));
    }
    Err(anyhow!(msg))
}

fn decode_first_return_object_id(result: &ExecutionResult) -> Result<AccountAddress> {
    let effects = result
        .effects
        .as_ref()
        .ok_or_else(|| anyhow!("missing effects"))?;
    let id_bytes = effects
        .return_values
        .first()
        .and_then(|values| values.first())
        .ok_or_else(|| anyhow!("missing return value"))?;
    if id_bytes.len() != 32 {
        return Err(anyhow!(
            "expected 32-byte object::ID return, got {} bytes",
            id_bytes.len()
        ));
    }
    let mut raw = [0u8; 32];
    raw.copy_from_slice(id_bytes);
    Ok(AccountAddress::new(raw))
}

fn decode_first_return_u64_triplet(result: &ExecutionResult) -> Result<(u64, u64, u64)> {
    let effects = result
        .effects
        .as_ref()
        .ok_or_else(|| anyhow!("missing effects"))?;
    let cmd_returns = effects
        .return_values
        .first()
        .ok_or_else(|| anyhow!("missing return values for first command"))?;

    let decode_u64 = |bytes: &[u8]| -> Result<u64> {
        if bytes.len() < 8 {
            return Err(anyhow!("expected u64 bytes len>=8, got {}", bytes.len()));
        }
        let arr: [u8; 8] = bytes[0..8]
            .try_into()
            .map_err(|_| anyhow!("invalid u64 bytes"))?;
        Ok(u64::from_le_bytes(arr))
    };

    if cmd_returns.len() >= 3 {
        return Ok((
            decode_u64(&cmd_returns[0])?,
            decode_u64(&cmd_returns[1])?,
            decode_u64(&cmd_returns[2])?,
        ));
    }

    let bytes = cmd_returns
        .first()
        .ok_or_else(|| anyhow!("missing return payload"))?;
    if bytes.len() < 24 {
        return Err(anyhow!(
            "expected tuple payload >=24 bytes, got {}",
            bytes.len()
        ));
    }
    let decode = |offset: usize| -> Result<u64> {
        let slice = bytes
            .get(offset..offset + 8)
            .ok_or_else(|| anyhow!("missing bytes at offset {}", offset))?;
        let arr: [u8; 8] = slice.try_into().map_err(|_| anyhow!("invalid u64 slice"))?;
        Ok(u64::from_le_bytes(arr))
    };
    Ok((decode(0)?, decode(8)?, decode(16)?))
}

fn query_current_pool_inner_version(env: &mut SimulationEnvironment) -> Result<u64> {
    let cmd = Command::MoveCall {
        package: AccountAddress::from_hex_literal(DEEPBOOK_RUNTIME_PACKAGE)?,
        module: Identifier::new("constants")?,
        function: Identifier::new("current_version")?,
        type_args: vec![],
        args: vec![],
    };
    let result = env.execute_ptb(vec![], vec![cmd]);
    ensure_success("constants::current_version", &result)?;
    let effects = result
        .effects
        .as_ref()
        .ok_or_else(|| anyhow!("missing effects"))?;
    let bytes = effects
        .return_values
        .first()
        .and_then(|values| values.first())
        .ok_or_else(|| anyhow!("missing current_version return"))?;
    if bytes.len() < 8 {
        return Err(anyhow!(
            "invalid current_version return bytes len={}",
            bytes.len()
        ));
    }
    let arr: [u8; 8] = bytes[0..8]
        .try_into()
        .map_err(|_| anyhow!("invalid current_version bytes"))?;
    Ok(u64::from_le_bytes(arr))
}

fn find_created_balance_manager_id(
    env: &SimulationEnvironment,
    result: &ExecutionResult,
) -> Result<AccountAddress> {
    let effects = result
        .effects
        .as_ref()
        .ok_or_else(|| anyhow!("missing effects"))?;
    for object_id in &effects.created {
        if let Some(obj) = env.get_object(object_id) {
            if let TypeTag::Struct(s) = &obj.type_tag {
                if s.module.as_ident_str().as_str() == "balance_manager"
                    && s.name.as_ident_str().as_str() == "BalanceManager"
                {
                    return Ok(*object_id);
                }
            }
        }
    }

    Err(anyhow!(
        "could not find created balance_manager::BalanceManager object"
    ))
}

fn find_created_pool_id(
    env: &SimulationEnvironment,
    result: &ExecutionResult,
) -> Result<AccountAddress> {
    let effects = result
        .effects
        .as_ref()
        .ok_or_else(|| anyhow!("missing effects"))?;
    for object_id in &effects.created {
        if let Some(obj) = env.get_object(object_id) {
            if let TypeTag::Struct(s) = &obj.type_tag {
                if s.module.as_ident_str().as_str() == "pool"
                    && s.name.as_ident_str().as_str() == "Pool"
                {
                    return Ok(*object_id);
                }
            }
        }
    }

    Err(anyhow!("could not find created pool::Pool object"))
}

fn recover_created_pool_object(
    env: &mut SimulationEnvironment,
    result: &ExecutionResult,
) -> Result<AccountAddress> {
    let effects = result
        .effects
        .as_ref()
        .ok_or_else(|| anyhow!("missing effects"))?;

    for change in &effects.object_changes {
        let (id, object_type) = match change {
            ObjectChange::Created {
                id,
                object_type: Some(object_type),
                ..
            } => (id, object_type),
            _ => continue,
        };

        let is_pool = match object_type {
            TypeTag::Struct(s) => {
                s.module.as_ident_str().as_str() == "pool"
                    && s.name.as_ident_str().as_str() == "Pool"
            }
            _ => false,
        };
        if !is_pool {
            continue;
        }

        let bytes = effects
            .created_object_bytes
            .get(id)
            .ok_or_else(|| anyhow!("pool object bytes missing from created_object_bytes"))?;
        let type_str = object_type.to_canonical_string(true);
        env.load_object_from_data(
            &id.to_hex_literal(),
            bytes.clone(),
            Some(&type_str),
            true,
            false,
            1,
        )?;
        return Ok(*id);
    }

    Err(anyhow!(
        "no pool::Pool entry found in object_changes for recovery"
    ))
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
