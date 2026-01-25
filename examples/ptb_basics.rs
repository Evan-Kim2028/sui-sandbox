//! PTB Basics - Your First Sui Transaction
//!
//! This is the simplest example - no API keys or network access required.
//! It demonstrates the core concepts of the Move VM sandbox:
//!
//! 1. Create a simulation environment
//! 2. Create test coins
//! 3. Build and execute a PTB (Programmable Transaction Block)
//! 4. Verify the results
//!
//! Run with: cargo run --example ptb_basics

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;

use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectInput};
use sui_sandbox_core::simulation::SimulationEnvironment;

fn main() -> Result<()> {
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                       PTB Basics Example                              ║");
    println!("║                                                                      ║");
    println!("║  No API keys needed - runs entirely locally!                         ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // Step 1: Create a Simulation Environment
    // =========================================================================
    // The SimulationEnvironment is a local Move VM that can execute PTBs.
    // It comes pre-loaded with the Sui Framework (0x2) and Move Stdlib (0x1).

    println!("Step 1: Creating simulation environment...\n");

    let mut env = SimulationEnvironment::new()?;

    // Set up two addresses: a sender and a recipient
    let sender = AccountAddress::from_hex_literal(
        "0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )?;
    let recipient = AccountAddress::from_hex_literal(
        "0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
    )?;

    env.set_sender(sender);

    println!("   Sender:    0x{:x}", sender);
    println!("   Recipient: 0x{:x}", recipient);

    // =========================================================================
    // Step 2: Create Test Coins
    // =========================================================================
    // We'll create SUI coins for testing. In a real scenario, these would
    // come from fetching on-chain state.

    println!("\nStep 2: Creating test SUI coins...\n");

    // Create a coin with 10 SUI (10 billion MIST)
    let coin_id = env.create_coin("0x2::sui::SUI", 10_000_000_000)?;
    println!("   Created coin: 0x{:x}", coin_id);
    println!("   Balance: 10 SUI (10,000,000,000 MIST)");

    // =========================================================================
    // Step 3: Build a PTB to Split and Transfer Coins
    // =========================================================================
    // PTBs (Programmable Transaction Blocks) are how Sui executes transactions.
    // A PTB consists of:
    //   - Inputs: Objects and pure values used by the transaction
    //   - Commands: Operations to perform (MoveCall, SplitCoins, TransferObjects, etc.)

    println!("\nStep 3: Building PTB to transfer 1 SUI...\n");

    // Get the coin object we created
    let coin_obj = env
        .get_object(&coin_id)
        .ok_or_else(|| anyhow::anyhow!("Coin not found"))?;

    // Parse the SUI coin type
    let sui_type: TypeTag = "0x2::sui::SUI".parse()?;
    let coin_type = TypeTag::Struct(Box::new(move_core_types::language_storage::StructTag {
        address: AccountAddress::from_hex_literal("0x2")?,
        module: Identifier::new("coin")?,
        name: Identifier::new("Coin")?,
        type_params: vec![sui_type],
    }));

    // Build the PTB inputs
    let inputs = vec![
        // Input 0: The coin we're splitting (as an owned object)
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: Some(coin_type),
            version: None,
        }),
        // Input 1: Amount to split off (1 SUI = 1 billion MIST)
        InputValue::Pure(bcs::to_bytes(&1_000_000_000u64)?),
        // Input 2: Recipient address
        InputValue::Pure(bcs::to_bytes(&recipient)?),
    ];

    // Build the PTB commands
    let commands = vec![
        // Command 0: Split 1 SUI from the coin
        // SplitCoins(coin, [amount]) -> [new_coin]
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(1)],
        },
        // Command 1: Transfer the split coin to recipient
        // TransferObjects([objects], address)
        Command::TransferObjects {
            objects: vec![Argument::NestedResult(0, 0)], // First result from command 0
            address: Argument::Input(2),
        },
    ];

    println!("   PTB Structure:");
    println!("   ├─ Input 0: Coin (10 SUI)");
    println!("   ├─ Input 1: Amount (1 SUI)");
    println!("   ├─ Input 2: Recipient address");
    println!("   ├─ Command 0: SplitCoins(coin, [1 SUI])");
    println!("   └─ Command 1: TransferObjects([split_coin], recipient)");

    // =========================================================================
    // Step 4: Execute the PTB
    // =========================================================================

    println!("\nStep 4: Executing PTB...\n");

    let result = env.execute_ptb(inputs, commands);

    if result.success {
        println!("   ✓ Transaction succeeded!");

        if let Some(effects) = &result.effects {
            println!("\n   Transaction Effects:");
            println!("   ├─ Gas used: {} MIST", effects.gas_used);
            println!("   ├─ Objects created: {}", effects.created.len());
            println!("   └─ Objects mutated: {}", effects.mutated.len());

            // Show created objects (the split coin)
            for id in &effects.created {
                println!("\n   New coin created: 0x{:x}", id);
                if let Some(obj) = env.get_object(id) {
                    // Extract balance from coin bytes (UID is 32 bytes, then u64 balance)
                    if obj.bcs_bytes.len() >= 40 {
                        let balance =
                            u64::from_le_bytes(obj.bcs_bytes[32..40].try_into().unwrap_or([0; 8]));
                        println!(
                            "   Balance: {} MIST ({} SUI)",
                            balance,
                            balance / 1_000_000_000
                        );
                    }
                }
            }

            // Show the original coin's new balance
            if let Some(mutated_id) = effects.mutated.first() {
                if let Some(obj) = env.get_object(mutated_id) {
                    if obj.bcs_bytes.len() >= 40 {
                        let balance =
                            u64::from_le_bytes(obj.bcs_bytes[32..40].try_into().unwrap_or([0; 8]));
                        println!("\n   Original coin updated: 0x{:x}", mutated_id);
                        println!(
                            "   New balance: {} MIST ({} SUI)",
                            balance,
                            balance / 1_000_000_000
                        );
                    }
                }
            }
        }
    } else {
        println!("   ✗ Transaction failed!");
        if let Some(error) = &result.error {
            println!("   Error: {:?}", error);
        }
        if let Some(raw) = &result.raw_error {
            println!("   Raw: {}", raw);
        }
    }

    // =========================================================================
    // Summary
    // =========================================================================

    println!("\n{}", "=".repeat(74));
    println!("\nWhat we demonstrated:");
    println!("   1. Created a local simulation environment (no network needed)");
    println!("   2. Created test SUI coins");
    println!("   3. Built a PTB with SplitCoins and TransferObjects commands");
    println!("   4. Executed the PTB and verified the results");
    println!("\nKey concepts:");
    println!("   - SimulationEnvironment: Local Move VM for testing");
    println!("   - PTB: Programmable Transaction Block (Sui's transaction format)");
    println!("   - Commands: SplitCoins, TransferObjects, MoveCall, etc.");
    println!("   - Arguments: Input(n), Result(n), NestedResult(cmd, idx)");
    println!("\nNext steps:");
    println!("   - Try fork_state to work with real mainnet data");
    println!("   - See cetus_swap or deepbook_replay for transaction replay");

    println!("\n{}\n", "=".repeat(74));

    Ok(())
}
