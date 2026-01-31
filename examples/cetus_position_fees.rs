//! Cetus Position Fee Inspection (Sandbox)
//!
//! This example mirrors the Cetus SDK flow in
//! `cetus-clmm-sui-sdk/src/modules/positionModule.ts` and runs it inside the
//! local Move VM sandbox.
//!
//! We call:
//!   `fetcher_script::fetch_position_fees`
//! from the Cetus integrate package, using either:
//! - real pool + position objects fetched from gRPC, or
//! - **synthetic BCS blobs** supplied via `.env` for local-only testing.
//!
//! ## Quickstart (two-step synthetic flow)
//!
//! Step 1: discover a live position + emit a synthetic template
//! ```bash
//! CETUS_AUTO_DISCOVER=1 CETUS_WRITE_SYNTHETIC_TEMPLATE=1 \
//!   cargo run --example cetus_position_fees
//! ```
//!
//! Step 2: copy the printed `.env` snippet and run fully synthetic
//! ```bash
//! CETUS_USE_SYNTHETIC=1 \
//!   cargo run --example cetus_position_fees
//! ```
//!
//! ## Why this works (and when it won’t)
//! - **Move execution requires bytecode.** The sandbox must load the Cetus
//!   package modules to execute `fetch_position_fees`.
//! - **The SDK’s `devInspectTransactionBlock` does the same thing** on a fullnode:
//!   it runs the real on-chain bytecode with the provided objects.
//! - For synthetic testing, you can inject BCS bytes for objects, but they must
//!   conform to the expected Move struct layouts. If the bytes are invalid or
//!   the package bytecode doesn’t match, execution will fail.
//!
//! This example still fetches **package bytecode from gRPC** (mainnet) because
//! there is no bundled Cetus bytecode in this repo. Synthetic mode only replaces
//! object fetching. For fully offline runs, you would need cached packages too.
//!
//! ## Data requirements
//! - **Packages**: Cetus CLMM + integrate bytecode (fetched by package ID).
//! - **Objects**: GlobalConfig, Pool, Position (type string, version, BCS).
//! - **Coin types**: inferred from Pool type params (or overridden via env).
//!
//! ## Setup
//! Configure your `.env`:
//! ```bash
//! SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
//! SUI_GRPC_API_KEY=your-api-key-here  # Optional, provider-specific
//!
//! # Required object IDs:
//! CETUS_POOL_ID=0x...
//! CETUS_POSITION_ID=0x...
//!
//! # Synthetic mode (optional, object BCS only):
//! CETUS_USE_SYNTHETIC=1
//! CETUS_POOL_TYPE=0x...::pool::Pool<...>
//! CETUS_POOL_VERSION=123
//! CETUS_POOL_BCS_BASE64=...
//! CETUS_POSITION_TYPE=0x...::position::Position<...>
//! CETUS_POSITION_VERSION=123
//! CETUS_POSITION_BCS_BASE64=...
//! CETUS_GLOBAL_CONFIG_TYPE=0x...::config::GlobalConfig
//! CETUS_GLOBAL_CONFIG_VERSION=123
//! CETUS_GLOBAL_CONFIG_BCS_BASE64=...
//!
//! # Optional: write a synthetic template from real objects
//! CETUS_WRITE_SYNTHETIC_TEMPLATE=1
//!
//! # Optional override if pool type parsing fails:
//! CETUS_COIN_A=0x2::sui::SUI
//! CETUS_COIN_B=0x...::usdc::USDC
//!
//! # Optional: auto-discover a live position
//! CETUS_AUTO_DISCOVER=1
//! ```
//!
//! Run with:
//! ```bash
//! cargo run --example cetus_position_fees
//! ```

mod common;

use anyhow::{anyhow, Result};
use base64::Engine;
use move_binary_format::file_format::SignatureToken;
use move_core_types::account_address::AccountAddress;
use move_core_types::annotated_value::MoveValue;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectInput};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::simulation::SimulationEnvironment;
use sui_sandbox_core::validator::Validator;
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcClient, GrpcOwner};

use common::{create_child_fetcher, parse_type_tag};
use std::collections::{HashSet, VecDeque};
use sui_sandbox_core::utilities::is_framework_package;

// -----------------------------------------------------------------------------
// Cetus mainnet config (from cetus-clmm-sui-sdk/src/config/mainnet.ts)
// -----------------------------------------------------------------------------
const CETUS_CLMM_PACKAGE_ID: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
const CETUS_CLMM_PUBLISHED_AT: &str =
    "0xc6faf3703b0e8ba9ed06b7851134bbbe7565eb35ff823fd78432baa4cbeaa12e";
const CETUS_INTEGRATE_PUBLISHED_AT: &str =
    "0x2d8c2e0fc6dd25b0214b3fa747e0fd27fd54608142cd2e4f64c1cd350cc4add4";
const CETUS_INTEGRATE_PACKAGE_ID: &str =
    "0x996c4d9480708fb8b92aa7acf819fb0497b5ec8e65ba06601cae2fb6db3312c3";
const CETUS_GLOBAL_CONFIG_ID: &str =
    "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f";

const FETCH_POSITION_FEES_FN: &str = "fetch_position_fees";

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                    Cetus Position Fee Sandbox                       ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let auto_discover = env_truthy("CETUS_AUTO_DISCOVER");
    let pool_id = std::env::var("CETUS_POOL_ID").ok();
    let position_id = std::env::var("CETUS_POSITION_ID").ok();
    let use_synthetic = env_truthy("CETUS_USE_SYNTHETIC");
    let write_template = env_truthy("CETUS_WRITE_SYNTHETIC_TEMPLATE");

    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let rt = tokio::runtime::Runtime::new()?;
    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    let graphql = GraphQLClient::mainnet();

    println!("Connected to: {}\n", endpoint);
    if use_synthetic && auto_discover {
        println!(
            "Note: CETUS_USE_SYNTHETIC=1 + CETUS_AUTO_DISCOVER=1 will only auto-fill IDs.\n\
             You still need matching synthetic BCS blobs for those IDs.\n"
        );
    }

    // -------------------------------------------------------------------------
    // Fetch required objects
    // -------------------------------------------------------------------------
    println!("Step 1/4: Resolve target IDs");
    let (pool_id, position_id) = if auto_discover {
        println!("Auto-discovering a Cetus position via GraphQL...");
        let (pool, position) = discover_cetus_position(&graphql)?;
        println!("  ✓ Found position: {}", position);
        println!("  ✓ Pool: {}", pool);
        (pool, position)
    } else {
        let pool = pool_id.ok_or_else(|| anyhow!("Missing CETUS_POOL_ID"))?;
        let position = position_id.ok_or_else(|| anyhow!("Missing CETUS_POSITION_ID"))?;
        (pool, position)
    };

    println!("Step 2/4: Load object data");
    let (global_config, pool_obj, position_obj) = if use_synthetic {
        println!("Synthetic mode enabled (using local BCS blobs).");
        let global_config = load_synthetic_object(
            CETUS_GLOBAL_CONFIG_ID,
            "CETUS_GLOBAL_CONFIG_TYPE",
            "CETUS_GLOBAL_CONFIG_VERSION",
            "CETUS_GLOBAL_CONFIG_BCS_BASE64",
            true,
        )?;
        let pool_obj = load_synthetic_object(
            &pool_id,
            "CETUS_POOL_TYPE",
            "CETUS_POOL_VERSION",
            "CETUS_POOL_BCS_BASE64",
            true,
        )?;
        let position_obj = load_synthetic_object(
            &position_id,
            "CETUS_POSITION_TYPE",
            "CETUS_POSITION_VERSION",
            "CETUS_POSITION_BCS_BASE64",
            false,
        )?;
        (global_config, pool_obj, position_obj)
    } else {
        let global_config = rt.block_on(fetch_object(&grpc, CETUS_GLOBAL_CONFIG_ID))?;
        let pool_obj = rt.block_on(fetch_object(&grpc, &pool_id))?;
        let position_obj = rt.block_on(fetch_object(&grpc, &position_id))?;
        if write_template {
            write_synthetic_template(&global_config, &pool_obj, &position_obj)?;
        }
        (global_config, pool_obj, position_obj)
    };

    // -------------------------------------------------------------------------
    // Initialize sandbox environment
    // -------------------------------------------------------------------------
    println!("Step 3/4: Load packages + objects into sandbox");
    let mut env = SimulationEnvironment::new()?;
    env.set_sender(AccountAddress::ZERO);

    let mut loaded_packages: HashSet<String> = HashSet::new();
    load_packages_with_deps(
        &mut env,
        &grpc,
        &rt,
        vec![
            CETUS_CLMM_PUBLISHED_AT.to_string(),
            CETUS_INTEGRATE_PUBLISHED_AT.to_string(),
            CETUS_INTEGRATE_PACKAGE_ID.to_string(),
        ],
        &mut loaded_packages,
    )?;

    load_object(&mut env, &global_config)?;
    load_object(&mut env, &pool_obj)?;
    load_object(&mut env, &position_obj)?;

    // -------------------------------------------------------------------------
    // Derive type arguments (coin types)
    // -------------------------------------------------------------------------
    let (coin_a, coin_b) = resolve_pool_coin_types(&pool_obj)?;
    println!("Pool coin types:");
    println!("  A: {}", coin_a);
    println!("  B: {}", coin_b);

    // -------------------------------------------------------------------------
    // Ensure coin type packages are loaded (non-framework packages only)
    // -------------------------------------------------------------------------
    let mut package_ids: HashSet<String> = HashSet::new();
    for tag in [&coin_a, &coin_b] {
        for pkg in common::extract_package_ids_from_type(&tag.to_string()) {
            if !is_framework_package(&pkg) {
                package_ids.insert(pkg);
            }
        }
    }

    load_packages_with_deps(
        &mut env,
        &grpc,
        &rt,
        package_ids.into_iter().collect(),
        &mut loaded_packages,
    )?;

    // On-demand child object loading (use latest versions if needed)
    let child_fetcher = create_child_fetcher(grpc, Default::default(), None);
    env.set_child_fetcher(child_fetcher);

    // -------------------------------------------------------------------------
    // Build PTB (fetcher_script::fetch_position_fees)
    // -------------------------------------------------------------------------
    println!("Step 4/4: Execute fetcher PTB");
    let mut inputs: Vec<InputValue> = Vec::new();
    let global_idx = push_object_input(&mut inputs, &global_config)?;
    let pool_idx = push_object_input(&mut inputs, &pool_obj)?;
    let position_addr = AccountAddress::from_hex_literal(&position_id)?;
    let position_idx = push_pure_address(&mut inputs, position_addr)?;

    let integrate_published = AccountAddress::from_hex_literal(CETUS_INTEGRATE_PUBLISHED_AT)?;
    let integrate_pkg = AccountAddress::from_hex_literal(CETUS_INTEGRATE_PACKAGE_ID)?;
    let (call_pkg, module_name, function_name, param_tokens) = {
        let resolver = env.resolver_mut();
        if let Some((module, fn_name, params)) =
            select_fetcher_function(resolver, &integrate_published)
        {
            (integrate_published, module, fn_name, params)
        } else if let Some((module, fn_name, params)) =
            select_fetcher_function(resolver, &integrate_pkg)
        {
            println!("fetcher_script not found at published_at, falling back to package_id");
            (integrate_pkg, module, fn_name, params)
        } else {
            return Err(anyhow!(
                "fetch_position_fees not found in integrate packages"
            ));
        }
    };

    let module_ref = {
        let resolver = env.resolver_mut();
        resolver
            .get_module_by_addr_name(&call_pkg, &module_name)
            .ok_or_else(|| anyhow!("Module not found: {}::{}", call_pkg, module_name))?
            .clone()
    };

    let args = build_args_from_signature(
        &module_ref,
        &param_tokens,
        &mut inputs,
        global_idx,
        pool_idx,
        position_idx,
        position_addr,
    )?;

    let command = Command::MoveCall {
        package: call_pkg,
        module: Identifier::new(module_name.as_str())?,
        function: Identifier::new(function_name.as_str())?,
        type_args: vec![coin_a, coin_b],
        args,
    };

    let result = env.execute_ptb(inputs, vec![command]);

    println!("\nExecution result:");
    if result.success {
        println!("  ✓ Success");
        if let Some(effects) = &result.effects {
            println!("  Gas used: {}", effects.gas_used);
            println!("  Events emitted: {}", effects.events.len());
        }
    } else {
        println!("  ✗ Failure");
        if let Some(err) = &result.error {
            println!("  Error: {:?}", err);
        }
        if let Some(raw) = &result.raw_error {
            println!("  Raw: {}", raw);
        }
    }

    // -------------------------------------------------------------------------
    // Decode FetchPositionFeesEvent (best-effort)
    // -------------------------------------------------------------------------
    let events = env.get_last_tx_events().to_vec();
    if events.is_empty() {
        println!("\nNo events captured.");
        return Ok(());
    }

    println!("\nCaptured events:");
    for event in &events {
        println!("  - {}", event.type_tag);
    }

    let validator = Validator::new(env.resolver_mut());
    let mut decoded_any = false;

    for event in &events {
        if !event.type_tag.contains("FetchPositionFeesEvent") {
            continue;
        }
        decoded_any = true;
        println!("\nFetchPositionFeesEvent:");
        if let Some(tag) = parse_type_tag(&event.type_tag) {
            match validator.resolve_type_layout(&tag) {
                Ok(layout) => match MoveValue::simple_deserialize(&event.data, &layout) {
                    Ok(value) => {
                        println!("  value: {:?}", value);
                    }
                    Err(err) => {
                        println!("  ! Failed to decode event data: {}", err);
                    }
                },
                Err(err) => {
                    println!("  ! Failed to resolve event layout: {}", err);
                }
            }
        } else {
            println!("  ! Failed to parse event type tag");
        }
    }

    if !decoded_any {
        println!("\nNo FetchPositionFeesEvent found. Check pool/position IDs.");
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

fn require_env(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow!("Missing required env var: {}", key))
}

fn env_truthy(key: &str) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn load_synthetic_object(
    object_id: &str,
    type_env: &str,
    version_env: &str,
    bcs_env: &str,
    is_shared: bool,
) -> Result<sui_transport::grpc::GrpcObject> {
    let type_string = require_env(type_env)?;
    let version = require_env(version_env)?.parse::<u64>()?;
    let bcs_b64 = require_env(bcs_env)?;
    let bcs = base64::engine::general_purpose::STANDARD
        .decode(bcs_b64.as_bytes())
        .map_err(|e| anyhow!("Invalid base64 in {}: {}", bcs_env, e))?;

    Ok(sui_transport::grpc::GrpcObject {
        object_id: object_id.to_string(),
        version,
        digest: "0x0".to_string(),
        type_string: Some(type_string),
        owner: if is_shared {
            GrpcOwner::Shared {
                initial_version: version,
            }
        } else {
            GrpcOwner::Address(AccountAddress::ZERO.to_hex_literal())
        },
        bcs: Some(bcs),
        bcs_full: None,
        package_modules: None,
        package_linkage: None,
        package_original_id: None,
    })
}

fn write_synthetic_template(
    global_config: &sui_transport::grpc::GrpcObject,
    pool: &sui_transport::grpc::GrpcObject,
    position: &sui_transport::grpc::GrpcObject,
) -> Result<()> {
    println!("\nSynthetic template (.env snippet):");
    println!("# Copy into .env and rerun with CETUS_USE_SYNTHETIC=1");
    println!("CETUS_USE_SYNTHETIC=1");
    println!("CETUS_POOL_ID={}", pool.object_id);
    println!("CETUS_POSITION_ID={}", position.object_id);
    println!(
        "CETUS_GLOBAL_CONFIG_TYPE={}",
        global_config.type_string.as_deref().unwrap_or("")
    );
    println!("CETUS_GLOBAL_CONFIG_VERSION={}", global_config.version);
    println!(
        "CETUS_GLOBAL_CONFIG_BCS_BASE64={}",
        base64::engine::general_purpose::STANDARD.encode(
            global_config
                .bcs
                .as_ref()
                .ok_or_else(|| anyhow!("Global config missing BCS"))?
        )
    );
    println!(
        "CETUS_POOL_TYPE={}",
        pool.type_string.as_deref().unwrap_or("")
    );
    println!("CETUS_POOL_VERSION={}", pool.version);
    println!(
        "CETUS_POOL_BCS_BASE64={}",
        base64::engine::general_purpose::STANDARD.encode(
            pool.bcs
                .as_ref()
                .ok_or_else(|| anyhow!("Pool missing BCS"))?
        )
    );
    println!(
        "CETUS_POSITION_TYPE={}",
        position.type_string.as_deref().unwrap_or("")
    );
    println!("CETUS_POSITION_VERSION={}", position.version);
    println!(
        "CETUS_POSITION_BCS_BASE64={}",
        base64::engine::general_purpose::STANDARD.encode(
            position
                .bcs
                .as_ref()
                .ok_or_else(|| anyhow!("Position missing BCS"))?
        )
    );
    println!();
    Ok(())
}

fn discover_cetus_position(graphql: &GraphQLClient) -> Result<(String, String)> {
    let position_type = format!("{}::position::Position", CETUS_CLMM_PACKAGE_ID);
    let positions = graphql.search_objects_by_type(&position_type, 200)?;

    for pos in positions {
        let position_id = pos.address.clone();
        let detailed = graphql.fetch_object(&position_id)?;
        if let Some(json) = detailed.content_json.as_ref() {
            if let Some(pool_id) = extract_pool_from_json(json) {
                return Ok((pool_id, position_id));
            }
        }
    }

    Err(anyhow!("Failed to auto-discover a Cetus position"))
}

fn extract_pool_from_json(value: &serde_json::Value) -> Option<String> {
    // Handle { fields: { pool: ... } } or { pool: ... }
    let pool_val = value
        .get("fields")
        .and_then(|f| f.get("pool"))
        .or_else(|| value.get("pool"))?;

    // Common shapes:
    // - "0x..." (string)
    // - { "id": "0x..." }
    // - { "id": { "id": "0x..." } }
    if let Some(s) = pool_val.as_str() {
        return Some(s.to_string());
    }
    if let Some(id_obj) = pool_val.get("id") {
        if let Some(s) = id_obj.as_str() {
            return Some(s.to_string());
        }
        if let Some(nested) = id_obj.get("id").and_then(|v| v.as_str()) {
            return Some(nested.to_string());
        }
    }
    None
}

async fn fetch_package_modules(
    grpc: &GrpcClient,
    package_id: &str,
) -> Result<Vec<(String, Vec<u8>)>> {
    let obj = grpc
        .get_object(package_id)
        .await?
        .ok_or_else(|| anyhow!("Package not found: {}", package_id))?;

    obj.package_modules
        .ok_or_else(|| anyhow!("No package modules for {}", package_id))
}

async fn fetch_object(
    grpc: &GrpcClient,
    object_id: &str,
) -> Result<sui_transport::grpc::GrpcObject> {
    grpc.get_object(object_id)
        .await?
        .ok_or_else(|| anyhow!("Object not found: {}", object_id))
}

fn load_object(
    env: &mut SimulationEnvironment,
    obj: &sui_transport::grpc::GrpcObject,
) -> Result<()> {
    let bcs = obj
        .bcs
        .clone()
        .ok_or_else(|| anyhow!("Object missing BCS: {}", obj.object_id))?;
    let is_shared = matches!(&obj.owner, GrpcOwner::Shared { .. });
    let is_immutable = matches!(&obj.owner, GrpcOwner::Immutable);
    env.load_object_from_data(
        &obj.object_id,
        bcs,
        obj.type_string.as_deref(),
        is_shared,
        is_immutable,
        obj.version,
    )?;
    Ok(())
}

fn resolve_pool_coin_types(
    pool_obj: &sui_transport::grpc::GrpcObject,
) -> Result<(TypeTag, TypeTag)> {
    // Prefer explicit overrides if provided.
    let coin_a_override = std::env::var("CETUS_COIN_A").ok();
    let coin_b_override = std::env::var("CETUS_COIN_B").ok();
    if let (Some(a), Some(b)) = (coin_a_override, coin_b_override) {
        let a_tag = parse_type_tag(&a).ok_or_else(|| anyhow!("Invalid CETUS_COIN_A: {}", a))?;
        let b_tag = parse_type_tag(&b).ok_or_else(|| anyhow!("Invalid CETUS_COIN_B: {}", b))?;
        return Ok((a_tag, b_tag));
    }

    let type_str = pool_obj
        .type_string
        .as_deref()
        .ok_or_else(|| anyhow!("Pool object missing type string"))?;
    let tag = parse_type_tag(type_str).ok_or_else(|| anyhow!("Failed to parse pool type"))?;
    let TypeTag::Struct(struct_tag) = tag else {
        return Err(anyhow!("Pool type is not a struct"));
    };
    if struct_tag.type_params.len() < 2 {
        return Err(anyhow!(
            "Pool type does not contain coin type params (got {})",
            struct_tag.type_params.len()
        ));
    }
    Ok((
        struct_tag.type_params[0].clone(),
        struct_tag.type_params[1].clone(),
    ))
}

fn push_object_input(
    inputs: &mut Vec<InputValue>,
    obj: &sui_transport::grpc::GrpcObject,
) -> Result<u16> {
    let id = AccountAddress::from_hex_literal(&obj.object_id)?;
    let bytes = obj
        .bcs
        .clone()
        .ok_or_else(|| anyhow!("Object missing BCS: {}", obj.object_id))?;
    let type_tag = obj.type_string.as_deref().and_then(parse_type_tag);
    let version = Some(obj.version);

    let input = match &obj.owner {
        GrpcOwner::Shared { .. } => ObjectInput::Shared {
            id,
            bytes,
            type_tag,
            version,
            mutable: true,
        },
        _ => ObjectInput::ImmRef {
            id,
            bytes,
            type_tag,
            version,
        },
    };

    let idx = inputs.len() as u16;
    inputs.push(InputValue::Object(input));
    Ok(idx)
}

fn push_pure_address(inputs: &mut Vec<InputValue>, addr: AccountAddress) -> Result<u16> {
    let bytes = bcs::to_bytes(&addr)?;
    let idx = inputs.len() as u16;
    inputs.push(InputValue::Pure(bytes));
    Ok(idx)
}

fn load_packages_with_deps(
    env: &mut SimulationEnvironment,
    grpc: &GrpcClient,
    rt: &tokio::runtime::Runtime,
    seeds: Vec<String>,
    loaded: &mut HashSet<String>,
) -> Result<()> {
    let mut queue: VecDeque<String> = seeds.into_iter().collect();

    while let Some(pkg) = queue.pop_front() {
        if loaded.contains(&pkg) || is_framework_package(&pkg) {
            continue;
        }

        let modules = match rt.block_on(fetch_package_modules(grpc, &pkg)) {
            Ok(mods) => mods,
            Err(err) => {
                eprintln!("Warning: failed to fetch package {}: {}", pkg, err);
                continue;
            }
        };

        env.deploy_package_at_address(&pkg, modules.clone())?;
        loaded.insert(pkg.clone());
        println!("Loaded package: {}", pkg);

        for (_, bytes) in modules {
            for dep in common::extract_dependencies_from_bytecode(&bytes) {
                if !loaded.contains(&dep) && !is_framework_package(&dep) {
                    queue.push_back(dep);
                }
            }
        }
    }

    Ok(())
}

fn select_fetcher_function(
    resolver: &LocalModuleResolver,
    package_addr: &AccountAddress,
) -> Option<(String, String, Vec<SignatureToken>)> {
    let modules = resolver.get_package_modules(package_addr);
    let mut fallbacks: Vec<(String, String, Vec<SignatureToken>)> = Vec::new();

    for module_name in modules {
        let module = resolver.get_module_by_addr_name(package_addr, &module_name)?;
        for def in module.function_defs() {
            let handle = module.function_handle_at(def.function);
            let name = module.identifier_at(handle.name).to_string();
            let params = module.signature_at(handle.parameters).0.clone();
            if name == FETCH_POSITION_FEES_FN {
                return Some((module_name.clone(), name, params));
            }
            if name.contains("fetch_position") {
                fallbacks.push((module_name.clone(), name, params));
            }
        }
    }

    if let Some((module, name, params)) = fallbacks.into_iter().next() {
        println!("Using fallback function: {}::{}", module, name);
        return Some((module, name, params));
    }

    None
}

fn build_args_from_signature(
    module: &move_binary_format::CompiledModule,
    params: &[SignatureToken],
    inputs: &mut Vec<InputValue>,
    global_idx: u16,
    pool_idx: u16,
    position_idx: u16,
    position_addr: AccountAddress,
) -> Result<Vec<Argument>> {
    let mut args = Vec::new();

    for token in params {
        let token = unwrap_ref(token);
        match token {
            SignatureToken::Address => args.push(Argument::Input(position_idx)),
            SignatureToken::Vector(inner) => {
                let inner = unwrap_ref(inner);
                let is_address_vec = matches!(inner, SignatureToken::Address);
                let is_object_id_vec = match inner {
                    SignatureToken::Datatype(handle_idx) => {
                        struct_name_for_handle(module, *handle_idx).ends_with("::object::ID")
                            || struct_name_for_handle(module, *handle_idx).ends_with("::ID")
                    }
                    SignatureToken::DatatypeInstantiation(inner) => {
                        let (handle_idx, _targs) = &**inner;
                        let name = struct_name_for_handle(module, *handle_idx);
                        name.ends_with("::object::ID") || name.ends_with("::ID")
                    }
                    _ => false,
                };

                if is_address_vec || is_object_id_vec {
                    let vec_bytes = bcs::to_bytes(&vec![position_addr])?;
                    let vec_idx = inputs.len() as u16;
                    inputs.push(InputValue::Pure(vec_bytes));
                    args.push(Argument::Input(vec_idx));
                } else {
                    return Err(anyhow!("Unsupported vector param: {:?}", inner));
                }
            }
            SignatureToken::U64 => {
                let limit_bytes = bcs::to_bytes(&1u64)?;
                let limit_idx = inputs.len() as u16;
                inputs.push(InputValue::Pure(limit_bytes));
                args.push(Argument::Input(limit_idx));
            }
            SignatureToken::Datatype(handle_idx) => {
                let name = struct_name_for_handle(module, *handle_idx);
                if name.contains("GlobalConfig") {
                    args.push(Argument::Input(global_idx));
                } else if name.contains("Pool") {
                    args.push(Argument::Input(pool_idx));
                } else {
                    return Err(anyhow!("Unsupported struct param: {}", name));
                }
            }
            SignatureToken::DatatypeInstantiation(inner) => {
                let (handle_idx, _type_args) = &**inner;
                let name = struct_name_for_handle(module, *handle_idx);
                if name.contains("GlobalConfig") {
                    args.push(Argument::Input(global_idx));
                } else if name.contains("Pool") {
                    args.push(Argument::Input(pool_idx));
                } else {
                    return Err(anyhow!("Unsupported struct param: {}", name));
                }
            }
            _ => {
                return Err(anyhow!("Unsupported param token: {:?}", token));
            }
        }
    }

    Ok(args)
}

fn unwrap_ref(token: &SignatureToken) -> &SignatureToken {
    match token {
        SignatureToken::Reference(inner) => inner,
        SignatureToken::MutableReference(inner) => inner,
        _ => token,
    }
}

fn struct_name_for_handle(
    module: &move_binary_format::CompiledModule,
    handle_idx: move_binary_format::file_format::DatatypeHandleIndex,
) -> String {
    let handle = module.datatype_handle_at(handle_idx);
    let module_handle = module.module_handle_at(handle.module);
    let module_name = module.identifier_at(module_handle.name).to_string();
    let struct_name = module.identifier_at(handle.name).to_string();
    format!("{}::{}", module_name, struct_name)
}
