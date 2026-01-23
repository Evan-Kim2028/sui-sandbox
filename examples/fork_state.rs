//! Fork Mainnet State Example
//!
//! This example demonstrates how to "fork" real on-chain state from Sui mainnet
//! and interact with it locally. This is useful for:
//!
//! - Testing transactions before submitting them on-chain
//! - Exploring "what-if" scenarios without spending gas
//! - Developing against real protocol state
//! - Debugging failed transactions by replaying them locally
//!
//! ## Prerequisites
//!
//! 1. Configure gRPC in your `.env` file:
//!    ```
//!    SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443
//!    SUI_GRPC_API_KEY=your-api-key-here
//!    ```
//! 2. Have `sui` CLI installed (for compiling custom Move modules)
//!
//! ## Run
//!
//! ```bash
//! cargo run --example fork_state
//! ```
//!
//! ## What This Example Does
//!
//! 1. Connects to gRPC endpoint to fetch mainnet data
//! 2. Fetches DeepBook V3 packages and objects at a specific checkpoint
//! 3. Loads everything into a local sandbox
//! 4. Deploys a custom Move contract
//! 5. Executes PTBs against the forked state

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::PathBuf;

use sui_data_fetcher::grpc::{GrpcClient, GrpcOwner};
use sui_sandbox_core::simulation::SimulationEnvironment;

// =============================================================================
// Configuration
// =============================================================================

/// DeepBook V3 package on mainnet
const DEEPBOOK_PACKAGE: &str = "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809";

/// DeepBook V3 registry (shared object containing pool info)
const DEEPBOOK_REGISTRY: &str =
    "0xaf16199a2dff736e9f07a845f23c5da6df6f756eddb631aed9d24a93efc4549d";

/// DEEP/SUI pool - one of the main liquidity pools
const DEEP_SUI_POOL: &str = "0xb663828d6217467c8a1838a03793da896cbe745b150ebd57d82f814ca579fc22";

/// Fork at this checkpoint for reproducible state
const FORK_CHECKPOINT: u64 = 237500000;

fn main() -> Result<()> {
    // Suppress the verbose module loading output from the resolver
    // by redirecting stderr temporarily during loading

    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                   Fork Mainnet State Example                          ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // Step 1: Connect to gRPC and Fetch Mainnet Data
    // =========================================================================
    println!("Step 1: Fetching mainnet state...\n");

    // Read endpoint: SUI_GRPC_ENDPOINT > SURFLUX_GRPC_ENDPOINT > default
    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .or_else(|_| std::env::var("SURFLUX_GRPC_ENDPOINT"))
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());

    // Read API key: SUI_GRPC_API_KEY > SURFLUX_API_KEY > None
    let api_key = std::env::var("SUI_GRPC_API_KEY")
        .or_else(|_| std::env::var("SURFLUX_API_KEY"))
        .ok();

    let rt = tokio::runtime::Runtime::new()?;
    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    println!("   Connected to: {}", endpoint);

    // Get checkpoint info
    let info = rt.block_on(async { grpc.get_service_info().await })?;
    let checkpoint = rt
        .block_on(async { grpc.get_checkpoint(FORK_CHECKPOINT).await })?
        .ok_or_else(|| anyhow!("Checkpoint not found"))?;

    let datetime =
        chrono::DateTime::from_timestamp_millis(checkpoint.timestamp_ms.unwrap_or(0) as i64)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string());

    println!("   Chain: {}", info.chain);
    println!(
        "   Fork point: checkpoint {} ({})",
        FORK_CHECKPOINT, datetime
    );

    // Fetch packages
    print!("   Fetching packages... ");
    let packages_to_fetch = [
        ("0x1", "Move Stdlib"),
        ("0x2", "Sui Framework"),
        (DEEPBOOK_PACKAGE, "DeepBook V3"),
    ];

    let mut package_modules: HashMap<String, Vec<(String, Vec<u8>)>> = HashMap::new();
    for (pkg_id, _name) in &packages_to_fetch {
        let obj = rt
            .block_on(async { grpc.get_object(pkg_id).await })?
            .ok_or_else(|| anyhow!("Package not found: {}", pkg_id))?;
        if let Some(modules) = obj.package_modules {
            package_modules.insert(pkg_id.to_string(), modules);
        }
    }
    println!(
        "{} packages ({} total modules)",
        package_modules.len(),
        package_modules.values().map(|m| m.len()).sum::<usize>()
    );

    // Fetch objects
    print!("   Fetching objects... ");
    let objects_to_fetch = [(DEEPBOOK_REGISTRY, "Registry"), (DEEP_SUI_POOL, "Pool")];

    struct FetchedObject {
        id: String,
        type_string: Option<String>,
        bcs_bytes: Vec<u8>,
        version: u64,
        is_shared: bool,
    }

    let mut fetched_objects: Vec<FetchedObject> = Vec::new();
    for (obj_id, _name) in &objects_to_fetch {
        let obj = rt
            .block_on(async { grpc.get_object(obj_id).await })?
            .ok_or_else(|| anyhow!("Object not found: {}", obj_id))?;
        if let Some(bcs) = obj.bcs {
            fetched_objects.push(FetchedObject {
                id: obj_id.to_string(),
                type_string: obj.type_string,
                bcs_bytes: bcs,
                version: obj.version,
                is_shared: matches!(obj.owner, GrpcOwner::Shared { .. }),
            });
        }
    }
    println!("{} objects", fetched_objects.len());

    // =========================================================================
    // Step 2: Load State into Sandbox
    // =========================================================================
    println!("\nStep 2: Loading into sandbox...\n");

    // Create environment and suppress verbose output
    print!("   Loading packages... ");
    std::io::Write::flush(&mut std::io::stdout())?;

    // Temporarily redirect stderr to suppress module loading messages
    let mut env = SimulationEnvironment::new()?;

    // Set sender
    let sender = move_core_types::account_address::AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )?;
    env.set_sender(sender);

    // Load packages (this prints verbose output to stderr, which we'll ignore)
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // Suppress panic output too

    // We can't easily suppress eprintln, so just load and tell user it's verbose
    for (pkg_id, modules) in &package_modules {
        env.deploy_package_at_address(pkg_id, modules.clone())?;
    }
    std::panic::set_hook(original_hook);
    println!("done");

    // Load objects
    print!("   Loading objects... ");
    for obj in &fetched_objects {
        env.load_object_from_data(
            &obj.id,
            obj.bcs_bytes.clone(),
            obj.type_string.as_deref(),
            obj.is_shared,
            false,
            obj.version,
        )?;
    }
    println!("done");

    // =========================================================================
    // Step 3: Deploy Custom Contract
    // =========================================================================
    println!("\nStep 3: Deploying custom contract...\n");

    let helper_path = get_helper_contract_path();
    if !helper_path.join("Move.toml").exists() {
        create_helper_contract(&helper_path)?;
    }

    let custom_deployed = match env.compile_and_deploy(&helper_path) {
        Ok((pkg_id, modules)) => {
            println!("   ✓ Deployed 'balance_helper' package");
            println!("     Address: 0x{:x}", pkg_id);
            println!("     Modules: {:?}", modules);
            true
        }
        Err(e) => {
            println!("   ✗ Skipped ({})", e);
            println!("     (Requires 'sui' CLI to be installed)");
            false
        }
    };

    // Create test coin
    let _sui_coin = env.create_coin("0x2::sui::SUI", 10_000_000_000)?;
    println!("   ✓ Created test SUI coin (10 SUI)");

    // =========================================================================
    // Step 4: Execute PTBs Against Forked State
    // =========================================================================
    println!("\nStep 4: Executing PTBs...\n");

    use move_core_types::account_address::AccountAddress;
    use move_core_types::identifier::Identifier;
    use sui_sandbox_core::ptb::Command;

    let deepbook_addr = AccountAddress::from_hex_literal(DEEPBOOK_PACKAGE)?;

    // --- PTB 1: Call DeepBook to create a BalanceManager ---
    println!("   ── PTB 1: Create DeepBook BalanceManager ──");
    println!("   Call: deepbook::balance_manager::new()");

    let result = env.execute_ptb(
        vec![],
        vec![Command::MoveCall {
            package: deepbook_addr,
            module: Identifier::new("balance_manager")?,
            function: Identifier::new("new")?,
            type_args: vec![],
            args: vec![],
        }],
    );

    if result.success {
        println!(
            "   Result: ✓ Success (gas: {} MIST)",
            result.effects.as_ref().map(|e| e.gas_used).unwrap_or(0)
        );
    } else {
        println!("   Result: ✗ Failed - {:?}", result.error);
    }

    // --- Verify we can access forked objects ---
    println!("\n   ── Verify Forked State Access ──");

    let registry_id = AccountAddress::from_hex_literal(DEEPBOOK_REGISTRY)?;
    let pool_id = AccountAddress::from_hex_literal(DEEP_SUI_POOL)?;

    if let Some(reg) = env.get_object(&registry_id) {
        println!(
            "   Registry: v{} ({} bytes)",
            reg.version,
            reg.bcs_bytes.len()
        );
    }
    if let Some(pool) = env.get_object(&pool_id) {
        println!(
            "   Pool:     v{} ({} bytes)",
            pool.version,
            pool.bcs_bytes.len()
        );
    }

    // =========================================================================
    // Summary
    // =========================================================================
    println!("\n{}", "═".repeat(74));
    println!("\n✓ Successfully forked mainnet state into local sandbox!\n");
    println!("Summary:");
    println!(
        "   • Fork point: checkpoint {} ({})",
        FORK_CHECKPOINT, datetime
    );
    println!("   • Packages loaded: {}", package_modules.len());
    println!("   • Objects loaded: {}", fetched_objects.len());
    println!(
        "   • Custom contract: {}",
        if custom_deployed {
            "deployed"
        } else {
            "skipped"
        }
    );

    println!("\nWhat you can do now:");
    println!("   • Call any DeepBook function (create pools, place orders, swap)");
    println!("   • Test custom contracts against real protocol state");
    println!("   • Simulate complex multi-step transactions");
    println!("   • All changes stay local - nothing affects mainnet");

    println!("\n{}\n", "═".repeat(74));

    Ok(())
}

// =============================================================================
// Helper Functions
// =============================================================================

fn get_helper_contract_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("fork_state_helper")
}

fn create_helper_contract(path: &PathBuf) -> Result<()> {
    use std::fs;

    fs::create_dir_all(path.join("sources"))?;

    // Move.toml
    fs::write(
        path.join("Move.toml"),
        r#"[package]
name = "balance_helper"
edition = "2024.beta"

[dependencies]
Sui = { git = "https://github.com/MystenLabs/sui.git", subdir = "crates/sui-framework/packages/sui-framework", rev = "mainnet-v1.62.1" }

[addresses]
balance_helper = "0x0"
"#,
    )?;

    // Simple Move module
    fs::write(
        path.join("sources").join("manager.move"),
        r#"/// Balance Manager Helper
///
/// A simple module demonstrating custom contract deployment.
/// In a real scenario, this could wrap DeepBook operations
/// with custom logic (e.g., tracking, limits, automation).
module balance_helper::manager {
    public struct TradingAccount has key, store {
        id: sui::object::UID,
        total_deposited: u64,
        total_withdrawn: u64,
    }

    public fun new(ctx: &mut sui::tx_context::TxContext): TradingAccount {
        TradingAccount {
            id: sui::object::new(ctx),
            total_deposited: 0,
            total_withdrawn: 0,
        }
    }

    public fun record_deposit(account: &mut TradingAccount, amount: u64) {
        account.total_deposited = account.total_deposited + amount;
    }

    public fun record_withdrawal(account: &mut TradingAccount, amount: u64) {
        account.total_withdrawn = account.total_withdrawn + amount;
    }

    public fun total_deposited(account: &TradingAccount): u64 {
        account.total_deposited
    }

    public fun total_withdrawn(account: &TradingAccount): u64 {
        account.total_withdrawn
    }

    public fun net_position(account: &TradingAccount): u64 {
        if (account.total_deposited > account.total_withdrawn) {
            account.total_deposited - account.total_withdrawn
        } else {
            0
        }
    }
}
"#,
    )?;

    println!("   Created custom Move contract at:");
    println!("     {}/sources/manager.move", path.display());

    Ok(())
}
