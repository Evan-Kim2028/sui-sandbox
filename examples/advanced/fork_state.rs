//! Fork Mainnet State + Custom Contract Deployment
//!
//! This example demonstrates the core power of sui-sandbox: running your own
//! Move contracts locally against real mainnet state, without deploying anything.
//!
//! ## What You'll See
//!
//! 1. **Fork mainnet** - Fetch real packages (Sui Framework, DeepBook) via gRPC
//! 2. **Load into sandbox** - Create an isolated local environment with that state
//! 3. **Deploy YOUR contract** - Compile and load your own Move code (no mainnet deploy!)
//! 4. **Execute PTBs** - Run Programmable Transaction Blocks against the combined state
//!
//! ## The Key Insight
//!
//! Your custom contract runs in the SAME environment as real mainnet code.
//! You can call DeepBook, Cetus, or any protocol - they behave exactly as on mainnet
//! because they ARE the real bytecode, just running locally.
//!
//! ## Run It
//!
//! ```bash
//! cargo run --example fork_state
//! ```
//!
//! ## Requirements
//!
//! - `.env` file with `SUI_GRPC_ENDPOINT` (optional: `SUI_GRPC_API_KEY`)
//! - `sui` CLI installed for custom contract compilation (optional - example still runs without it)

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use std::path::{Path, PathBuf};

use sui_sandbox_core::bootstrap::ensure_package_registration_success;
use sui_sandbox_core::environment_bootstrap::{
    hydrate_and_build_mainnet_environment, EnvironmentBuildPlan, MainnetHydrationPlan,
    MainnetObjectRequest,
};
use sui_sandbox_core::orchestrator::ReplayOrchestrator;
use sui_sandbox_core::simulation::SimulationEnvironment;

// DeepBook V3 - a real DeFi protocol we'll interact with
const DEEPBOOK_PACKAGE: &str = "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809";
const DEEPBOOK_REGISTRY: &str =
    "0xaf16199a2dff736e9f07a845f23c5da6df6f756eddb631aed9d24a93efc4549d";

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    print_header();

    // =========================================================================
    // STEP 1: Fork mainnet state via gRPC
    // =========================================================================
    // We fetch real package bytecode and object state from Sui mainnet.
    // This is the same code that runs in production.

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 1: Fetching real mainnet state via gRPC");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let packages_to_fetch: [(&str, &str); 3] = [
        ("0x1", "Move Stdlib"),
        ("0x2", "Sui Framework"),
        (DEEPBOOK_PACKAGE, "DeepBook V3"),
    ];
    let package_roots: Vec<AccountAddress> = packages_to_fetch
        .iter()
        .map(|(pkg_id, _)| AccountAddress::from_hex_literal(pkg_id))
        .collect::<Result<Vec<_>, _>>()?;

    let rt = tokio::runtime::Runtime::new()?;
    let sender = AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )?;
    let bootstrap = rt.block_on(hydrate_and_build_mainnet_environment(
        &MainnetHydrationPlan {
            package_roots: package_roots.clone(),
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
    ))?;
    println!(
        "Connected to: {}\n",
        bootstrap.hydration.provider.grpc_endpoint()
    );

    for (pkg_id, name) in packages_to_fetch {
        let addr = AccountAddress::from_hex_literal(pkg_id)?;
        if let Some(pkg) = bootstrap.hydration.packages.get(&addr) {
            println!("  ✓ {} - {} modules fetched", name, pkg.modules.len());
        }
    }

    let registry_obj = bootstrap
        .hydration
        .objects
        .get(DEEPBOOK_REGISTRY)
        .ok_or_else(|| anyhow!("Registry not found"))?;
    println!("  ✓ DeepBook Registry object (version {})", registry_obj.2);

    // =========================================================================
    // STEP 2: Create sandbox and load the forked state
    // =========================================================================
    // The sandbox is a fully isolated Move VM environment.
    // We load the mainnet bytecode into it - now it runs locally!

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 2: Loading state into local sandbox");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let build = bootstrap.build;
    println!("  Sender: 0x{:x}...", sender);

    let registration = &build.package_registration;
    ensure_package_registration_success(registration)?;
    println!("  ✓ Loaded {} packages into sandbox", registration.loaded);
    println!("  ✓ Loaded {} objects into sandbox", build.objects_loaded);
    println!("  ✓ Loaded DeepBook Registry object");
    let mut env: SimulationEnvironment = build.env;

    // =========================================================================
    // STEP 3: Deploy YOUR custom contract into the sandbox
    // =========================================================================
    // This is where it gets powerful: compile your own Move code and deploy it
    // into the same environment as the mainnet protocols. No actual deployment!

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 3: Deploying YOUR custom contract (locally!)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let helper_path = get_helper_contract_path();
    if !helper_path.join("Move.toml").exists() {
        create_helper_contract(&helper_path)?;
        println!("  Created example contract at: {:?}", helper_path);
    }

    let custom_pkg_id = match env.compile_and_deploy(&helper_path) {
        Ok((pkg_id, modules)) => {
            println!("  ✓ Compiled and deployed 'balance_helper' package");
            println!("    Address: 0x{:x}", pkg_id);
            println!("    Modules: {:?}", modules);
            println!("\n  NOTE: This contract exists ONLY in the sandbox.");
            println!("        It was never deployed to mainnet!");
            Some(pkg_id)
        }
        Err(e) => {
            println!("  ⚠ Custom contract deployment skipped");
            println!("    Reason: {}", e);
            println!("    To enable: Install the 'sui' CLI (https://docs.sui.io/build/install)");
            None
        }
    };

    // =========================================================================
    // STEP 4: Execute PTBs - call both mainnet code and your custom contract
    // =========================================================================
    // Now we execute Programmable Transaction Blocks (PTBs) that can call
    // ANY code in the sandbox - both real mainnet protocols and your contracts.

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 4: Executing PTBs (Programmable Transaction Blocks)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // --- Call REAL DeepBook code (from mainnet) ---
    println!("  Calling REAL DeepBook protocol code (forked from mainnet):");

    let deepbook_addr = AccountAddress::from_hex_literal(DEEPBOOK_PACKAGE)?;
    match ReplayOrchestrator::execute_noarg_move_call_with_summary(
        &mut env,
        deepbook_addr,
        "balance_manager",
        "new",
        "deepbook::balance_manager::new",
    ) {
        Ok(summary) => {
            println!("    ✓ deepbook::balance_manager::new() succeeded");
            println!("      Gas used: {} MIST", summary.gas_used);
            println!("      Objects created: {}", summary.created_objects);
        }
        Err(err) => {
            println!("    ✗ Failed: {}", err);
        }
    }

    // --- Call YOUR custom contract ---
    if let Some(pkg_id) = custom_pkg_id {
        println!("\n  Calling YOUR custom contract (sandbox-only):");

        match ReplayOrchestrator::execute_noarg_move_call_with_summary(
            &mut env,
            pkg_id,
            "manager",
            "new",
            "balance_helper::manager::new",
        ) {
            Ok(summary) => {
                println!("    ✓ balance_helper::manager::new() succeeded");
                println!("      Gas used: {} MIST", summary.gas_used);
                println!("      Objects created: {}", summary.created_objects);
            }
            Err(err) => {
                println!("    ✗ Failed: {}", err);
            }
        }
    }

    // =========================================================================
    // Summary
    // =========================================================================
    print_summary(custom_pkg_id.is_some());

    Ok(())
}

fn print_header() {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           SUI SANDBOX: Fork Mainnet + Custom Contracts               ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║  This example shows how to:                                          ║");
    println!("║    1. Fork real mainnet state into a local sandbox                   ║");
    println!("║    2. Deploy your own Move contracts (without mainnet deployment)    ║");
    println!("║    3. Execute PTBs that interact with both                           ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!();
}

fn print_summary(custom_deployed: bool) {
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                           WHAT HAPPENED                              ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║                                                                      ║");
    println!("║  ✓ Fetched REAL bytecode from Sui mainnet via gRPC                   ║");
    println!("║  ✓ Loaded it into a local SimulationEnvironment                      ║");
    if custom_deployed {
        println!("║  ✓ Compiled and deployed YOUR contract into the same environment    ║");
        println!("║  ✓ Executed PTBs calling both mainnet code and your contract        ║");
    } else {
        println!("║  - Custom contract skipped (install 'sui' CLI to enable)            ║");
        println!("║  ✓ Executed PTBs calling real mainnet code locally                  ║");
    }
    println!("║                                                                      ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║                          WHY THIS MATTERS                            ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║                                                                      ║");
    println!("║  You can now develop and test contracts that interact with:          ║");
    println!("║    • DeepBook (order books, trading)                                 ║");
    println!("║    • Cetus (AMM, liquidity pools)                                    ║");
    println!("║    • Any Sui protocol                                                ║");
    println!("║                                                                      ║");
    println!("║  All WITHOUT:                                                        ║");
    println!("║    • Deploying to testnet/mainnet                                    ║");
    println!("║    • Spending gas                                                    ║");
    println!("║    • Waiting for transactions                                        ║");
    println!("║                                                                      ║");
    println!("║  Iterate fast. Test locally. Deploy when ready.                      ║");
    println!("║                                                                      ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
}

fn get_helper_contract_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("advanced")
        .join("fork_state_helper")
}

fn create_helper_contract(path: &Path) -> Result<()> {
    use std::fs;
    fs::create_dir_all(path.join("sources"))?;

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

    // A simple but realistic example contract
    fs::write(
        path.join("sources").join("manager.move"),
        r#"/// Example: A trading position tracker that could interact with DeepBook.
///
/// This demonstrates deploying custom logic alongside forked mainnet state.
/// In a real scenario, you might:
///   - Track positions across multiple DEXs
///   - Implement custom risk management
///   - Build aggregation logic
module balance_helper::manager {
    /// Tracks deposits and withdrawals for a trading account.
    public struct TradingAccount has key, store {
        id: sui::object::UID,
        total_deposited: u64,
        total_withdrawn: u64,
    }

    /// Create a new trading account.
    public fun new(ctx: &mut sui::tx_context::TxContext): TradingAccount {
        TradingAccount {
            id: sui::object::new(ctx),
            total_deposited: 0,
            total_withdrawn: 0,
        }
    }

    /// Record a deposit.
    public fun record_deposit(account: &mut TradingAccount, amount: u64) {
        account.total_deposited = account.total_deposited + amount;
    }

    /// Record a withdrawal.
    public fun record_withdrawal(account: &mut TradingAccount, amount: u64) {
        account.total_withdrawn = account.total_withdrawn + amount;
    }

    /// Get net position (deposits - withdrawals).
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

    Ok(())
}
