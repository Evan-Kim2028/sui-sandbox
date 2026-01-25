//! Transaction Replay Module
//!
//! This module provides types and utilities for replaying Sui transactions
//! in the local Move VM sandbox. This enables:
//!
//! 1. **Validation**: Compare local execution with on-chain effects
//! 2. **Training Data**: Generate input/output pairs for LLM training
//! 3. **Testing**: Use real transaction patterns for testing
//!
//! ## Architecture
//!
//! Transactions are fetched via GraphQL (see `DataFetcher`) and cached locally.
//! The cached transactions can then be replayed using the `FetchedTransaction::replay()` method.
//!
//! ```text
//! GraphQL → CachedTransaction → PTBCommands → LocalExecution → CompareEffects
//! ```
//!
//! ## Usage
//!
//! See `examples/` for complete transaction replay examples.
//! Requires a `.tx-cache` directory with cached transaction data.
//!
//! ## Known Limitations: Dynamic Field Traversal
//!
//! Some DeFi protocols (Cetus, Turbos) use `skip_list` data structures that store
//! tick data as dynamic fields. These present a replay challenge:
//!
//! 1. The skip_list computes tick indices at runtime during traversal
//! 2. Each computed index becomes a dynamic field lookup via `derive_dynamic_field_id()`
//! 3. We can pre-fetch known dynamic fields, but not indices computed during execution
//!
//! **Example**: A Cetus swap traverses: `head(0) → 481316 → 512756 → tail(887272)`.
//! If the swap needs tick `500000`, this is computed at runtime and we can't know
//! to pre-fetch it without simulating the entire traversal.
//!
//! **Workarounds**:
//! - Cache all dynamic field children at transaction time
//! - Use synthetic/mocked transactions for testing (see `synthetic_ptb_case_study.rs`)
//! - Pre-fetch all known tick indices for a pool
//!
//! ## Version Tracking
//!
//! The module supports full version tracking for replayed transactions, allowing validation
//! that object versions are correctly computed. Use [`replay_with_version_tracking()`] for
//! version-aware replay.
//!
//! ### Version Tracking Flow
//!
//! ```text
//! 1. Fetch Transaction
//!    └── gRPC response includes object versions in:
//!        - changed_objects: {object_id: input_version}
//!        - unchanged_loaded_runtime_objects: {object_id: version}
//!        - inputs (Object/SharedObject/Receiving variants)
//!
//! 2. Build Version Map
//!    └── HashMap<String, u64> mapping object IDs to their input versions
//!
//! 3. Convert to PTB Commands with Versions
//!    └── to_ptb_commands_with_versions() creates InputValue::Object with version set
//!
//! 4. Execute with Version Tracking
//!    └── PTBExecutor::set_track_versions(true)
//!    └── PTBExecutor::set_lamport_timestamp(max_version + 1)
//!    └── Effects include object_versions: HashMap<ObjectId, VersionInfo>
//!
//! 5. Compare and Validate
//!    └── EffectsComparison::compare_with_versions() checks:
//!        - Input versions match expected (from gRPC)
//!        - Output versions = input + 1 for mutated objects
//!        - Reports VersionMismatch for any discrepancies
//! ```
//!
//! ### Version Tracking Example
//!
//! ```rust,ignore
//! let object_versions: HashMap<String, u64> = /* from gRPC */;
//! let result = replay_with_version_tracking(
//!     &tx,
//!     &mut harness,
//!     &cached_objects,
//!     &address_aliases,
//!     Some(&object_versions),
//! )?;
//!
//! if let Some(comparison) = &result.comparison {
//!     println!("Input versions matched: {}/{}",
//!         comparison.input_versions_matched,
//!         comparison.input_versions_total);
//!     println!("Version increments valid: {}/{}",
//!         comparison.version_increments_valid,
//!         comparison.version_increments_total);
//! }
//! ```

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use serde::Serialize;

use crate::ptb::{Argument, Command, InputValue, ObjectInput};
use crate::vm::VMHarness;

// Re-export type parsing functions from the canonical location (types module)
// This maintains backwards compatibility while centralizing the implementation.
pub use crate::types::{
    clear_type_cache as clear_type_tag_cache, parse_type_tag,
    type_cache_size as type_tag_cache_size,
};

// ============================================================================
// Re-export all types from sui-sandbox-types
// ============================================================================

pub use sui_sandbox_types::{
    transaction::base64_bytes, CachedDynamicField, CachedTransaction, DynamicFieldEntry,
    EffectsComparison, FetchedObject, FetchedTransaction, GasSummary, LocalVersionInfo, ObjectID,
    PtbArgument, PtbCommand, ReplayResult, TransactionCache, TransactionDigest,
    TransactionEffectsSummary, TransactionInput, TransactionStatus, VersionMismatch,
    VersionMismatchType, VersionSummary,
};

// ============================================================================
// Dynamic Field ID Derivation
// ============================================================================

/// Derive the object ID for a dynamic field given the parent UID, key type, and key value.
///
/// This implements the same formula as Sui's `dynamic_field::derive_dynamic_field_id`:
/// ```text
/// Blake2b256(0xf0 || parent || len(key_bytes) || key_bytes || bcs(key_type_tag))
/// ```
///
/// Where:
/// - `0xf0` is the `HashingIntentScope::ChildObjectId` prefix
/// - `parent` is the 32-byte parent UID address
/// - `len(key_bytes)` is the length as 8-byte little-endian (usize)
/// - `key_bytes` is the BCS-serialized key value
/// - `bcs(key_type_tag)` is the BCS-serialized TypeTag of the key
///
/// # Arguments
/// * `parent` - The parent object's UID address (32 bytes)
/// * `key_type_tag` - The Move TypeTag of the key (e.g., TypeTag::U64)
/// * `key_bytes` - The BCS-serialized key value
///
/// # Returns
/// The derived ObjectID (32 bytes) as an AccountAddress
///
/// # Example
/// ```
/// use sui_sandbox_core::tx_replay::derive_dynamic_field_id;
/// use move_core_types::account_address::AccountAddress;
/// use move_core_types::language_storage::TypeTag;
///
/// let parent = AccountAddress::from_hex_literal("0x2").unwrap();
/// let key: u64 = 481316;
/// let key_bytes = bcs::to_bytes(&key).unwrap();
/// let obj_id = derive_dynamic_field_id(parent, &TypeTag::U64, &key_bytes).unwrap();
/// assert!(obj_id.to_hex_literal().starts_with("0x"));
/// ```
pub fn derive_dynamic_field_id(
    parent: AccountAddress,
    key_type_tag: &TypeTag,
    key_bytes: &[u8],
) -> Result<AccountAddress> {
    use fastcrypto::hash::{Blake2b256, HashFunction};

    // HashingIntentScope::ChildObjectId = 0xf0
    const CHILD_OBJECT_ID_SCOPE: u8 = 0xf0;

    // BCS-serialize the type tag
    let type_tag_bytes = bcs::to_bytes(key_type_tag)
        .map_err(|e| anyhow!("Failed to BCS-serialize type tag: {}", e))?;

    // Build the input: scope || parent || len(key) || key || type_tag
    let mut input = Vec::with_capacity(1 + 32 + 8 + key_bytes.len() + type_tag_bytes.len());
    input.push(CHILD_OBJECT_ID_SCOPE);
    input.extend_from_slice(parent.as_ref());
    input.extend_from_slice(&(key_bytes.len() as u64).to_le_bytes());
    input.extend_from_slice(key_bytes);
    input.extend_from_slice(&type_tag_bytes);

    // Hash with Blake2b-256
    let hash = Blake2b256::digest(&input);

    // Convert to AccountAddress (hash.digest is [u8; 32])
    Ok(AccountAddress::new(hash.digest))
}

/// Derive the object ID for a dynamic field with a u64 key.
///
/// Convenience wrapper around `derive_dynamic_field_id` for the common case
/// of u64 keys (used by skip_list, table, etc.).
///
/// # Arguments
/// * `parent` - The parent object's UID address
/// * `key` - The u64 key value
///
/// # Returns
/// The derived ObjectID as an AccountAddress
pub fn derive_dynamic_field_id_u64(parent: AccountAddress, key: u64) -> Result<AccountAddress> {
    let key_bytes =
        bcs::to_bytes(&key).map_err(|e| anyhow!("Failed to BCS-serialize u64 key: {}", e))?;
    derive_dynamic_field_id(parent, &TypeTag::U64, &key_bytes)
}

// ============================================================================
// Parallel Replay
// ============================================================================

/// Result of a parallel replay operation.
#[derive(Debug, Clone, Serialize)]
pub struct ParallelReplayResult {
    /// Total transactions processed
    pub total: usize,
    /// Successfully executed locally
    pub successful: usize,
    /// Status matched with on-chain
    pub status_matched: usize,
    /// Individual results
    pub results: Vec<ReplayResult>,
    /// Processing time in milliseconds
    pub elapsed_ms: u64,
    /// Transactions per second
    pub tps: f64,
}

/// Build address alias map by examining the bytecode self-addresses.
/// Returns a map: on-chain package ID (runtime) -> bytecode self-address
/// This allows module resolution: when looking for runtime address, fall back to bytecode.
///
/// Note: For hash rewriting (bytecode -> runtime), callers should build an inverted map.
fn build_address_aliases(
    cached: &CachedTransaction,
) -> std::collections::HashMap<AccountAddress, AccountAddress> {
    use move_binary_format::file_format::CompiledModule;

    let mut aliases = std::collections::HashMap::new();

    for pkg_id in cached.packages.keys() {
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            // Get the runtime address (on-chain package ID)
            let runtime_addr = match AccountAddress::from_hex_literal(pkg_id) {
                Ok(addr) => addr,
                Err(_) => continue,
            };

            // Find the bytecode address from the module
            for (_name, bytes) in &modules {
                if bytes.is_empty() {
                    continue;
                }
                if let Ok(module) = CompiledModule::deserialize_with_defaults(bytes) {
                    let bytecode_addr = *module.self_id().address();
                    if bytecode_addr != runtime_addr {
                        // Map runtime address -> bytecode address (for module resolution)
                        aliases.insert(runtime_addr, bytecode_addr);
                    }
                    break; // All modules in a package have the same address
                }
            }
        }
    }

    aliases
}

/// Public wrapper for testing - builds address aliases for a cached transaction.
pub fn build_address_aliases_for_test(
    cached: &CachedTransaction,
) -> std::collections::HashMap<AccountAddress, AccountAddress> {
    build_address_aliases(cached)
}

/// Build a comprehensive address alias map from multiple sources.
///
/// This combines:
/// 1. Bytecode self-address mappings from loaded packages
/// 2. Package linkage/upgrade mappings (original -> upgraded)
///
/// The resulting map allows the VM to resolve addresses correctly when:
/// - A type refers to an original package ID that has been upgraded
/// - The bytecode self-address differs from the on-chain package ID
///
/// # Arguments
/// * `cached` - The cached transaction with loaded packages
/// * `linkage_upgrades` - Map of original package IDs to their upgraded versions
///
/// # Returns
/// A map: runtime/on-chain address -> bytecode/original address
pub fn build_comprehensive_address_aliases(
    cached: &CachedTransaction,
    linkage_upgrades: &std::collections::HashMap<String, String>,
) -> std::collections::HashMap<AccountAddress, AccountAddress> {
    // Start with bytecode-based aliases
    let mut aliases = build_address_aliases(cached);

    // Add linkage-based aliases (upgraded -> original)
    // This maps the upgraded package ID back to the original package ID
    for (original_id, upgraded_id) in linkage_upgrades {
        // Normalize both addresses
        let original_normalized = crate::utilities::normalize_address(original_id);
        let upgraded_normalized = crate::utilities::normalize_address(upgraded_id);

        if let (Ok(original_addr), Ok(upgraded_addr)) = (
            AccountAddress::from_hex_literal(&format!("0x{}", original_normalized)),
            AccountAddress::from_hex_literal(&format!("0x{}", upgraded_normalized)),
        ) {
            // Map upgraded -> original (so when we see a type with upgraded ID,
            // we can resolve it to the original for module lookup)
            if original_addr != upgraded_addr {
                aliases.insert(upgraded_addr, original_addr);
            }
        }
    }

    aliases
}

/// Replay multiple transactions in parallel.
///
/// This function uses rayon for parallel execution, creating a separate
/// VMHarness for each thread to avoid contention.
pub fn replay_parallel(
    transactions: &[CachedTransaction],
    resolver: &crate::resolver::LocalModuleResolver,
    num_threads: Option<usize>,
) -> Result<ParallelReplayResult> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    // Configure thread pool
    if let Some(threads) = num_threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .ok(); // Ignore if already configured
    }

    let start = Instant::now();
    let total = transactions.len();
    let successful = AtomicUsize::new(0);
    let status_matched = AtomicUsize::new(0);

    // Process transactions in parallel
    let results: Vec<ReplayResult> = transactions
        .par_iter()
        .map(|cached| {
            // Create a resolver with cached packages
            let mut local_resolver = resolver.clone();

            // Build address alias map for this transaction
            let address_aliases = build_address_aliases(cached);

            // Load cached packages into the resolver
            for pkg_id in cached.packages.keys() {
                if let Some(modules) = cached.get_package_modules(pkg_id) {
                    // Don't use target address aliasing - we'll rewrite the transaction instead
                    let _ = local_resolver.add_package_modules(modules);
                }
            }

            // Create harness and replay with address rewriting
            match VMHarness::new(&local_resolver, false) {
                Ok(mut harness) => {
                    match replay_with_objects_and_aliases(
                        &cached.transaction,
                        &mut harness,
                        &cached.objects,
                        &address_aliases,
                    ) {
                        Ok(result) => {
                            if result.local_success {
                                successful.fetch_add(1, Ordering::Relaxed);
                            }
                            if result
                                .comparison
                                .as_ref()
                                .map(|c| c.status_match)
                                .unwrap_or(false)
                            {
                                status_matched.fetch_add(1, Ordering::Relaxed);
                            }
                            result
                        }
                        Err(e) => ReplayResult {
                            digest: cached.transaction.digest.clone(),
                            local_success: false,
                            local_error: Some(e.to_string()),
                            comparison: None,
                            commands_executed: 0,
                            commands_failed: cached.transaction.commands.len(),
                            objects_tracked: 0,
                            lamport_timestamp: None,
                            version_summary: None,
                            gas_used: 0,
                        },
                    }
                }
                Err(e) => ReplayResult {
                    digest: cached.transaction.digest.clone(),
                    local_success: false,
                    local_error: Some(format!("Failed to create harness: {}", e)),
                    comparison: None,
                    commands_executed: 0,
                    commands_failed: cached.transaction.commands.len(),
                    objects_tracked: 0,
                    lamport_timestamp: None,
                    version_summary: None,
                    gas_used: 0,
                },
            }
        })
        .collect();

    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    let tps = if elapsed_ms > 0 {
        (total as f64 * 1000.0) / elapsed_ms as f64
    } else {
        0.0
    };

    Ok(ParallelReplayResult {
        total,
        successful: successful.load(Ordering::Relaxed),
        status_matched: status_matched.load(Ordering::Relaxed),
        results,
        elapsed_ms,
        tps,
    })
}

// ============================================================================
// FetchedTransaction Extension Methods
// ============================================================================

// These are extension functions that work on FetchedTransaction but depend on
// VM and PTB types that can't be in sui-sandbox-types.

/// Convert a FetchedTransaction to PTB commands for local execution.
pub fn to_ptb_commands(tx: &FetchedTransaction) -> Result<(Vec<InputValue>, Vec<Command>)> {
    // Use a large default balance for simulation (1 billion SUI = 10^18 MIST)
    // This ensures SplitCoins won't fail due to insufficient balance
    // The actual gas coin balance on-chain is typically much larger than gas_budget
    const DEFAULT_GAS_BALANCE: u64 = 1_000_000_000_000_000_000; // 1B SUI in MIST
    to_ptb_commands_internal(tx, DEFAULT_GAS_BALANCE, &std::collections::HashMap::new())
}

/// Convert a FetchedTransaction to PTB commands using cached object data.
pub fn to_ptb_commands_with_objects(
    tx: &FetchedTransaction,
    cached_objects: &std::collections::HashMap<String, String>,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    const DEFAULT_GAS_BALANCE: u64 = 1_000_000_000_000_000_000;
    to_ptb_commands_internal(tx, DEFAULT_GAS_BALANCE, cached_objects)
}

/// Convert a FetchedTransaction to PTB commands with address rewriting.
/// The aliases map on-chain package addresses to bytecode self-addresses.
pub fn to_ptb_commands_with_objects_and_aliases(
    tx: &FetchedTransaction,
    cached_objects: &std::collections::HashMap<String, String>,
    address_aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    const DEFAULT_GAS_BALANCE: u64 = 1_000_000_000_000_000_000;
    to_ptb_commands_internal_with_aliases(tx, DEFAULT_GAS_BALANCE, cached_objects, address_aliases)
}

/// Convert to PTB commands with gas budget.
pub fn to_ptb_commands_with_gas_budget(
    tx: &FetchedTransaction,
    gas_balance: u64,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    to_ptb_commands_internal(tx, gas_balance, &std::collections::HashMap::new())
}

/// Convert a FetchedTransaction to PTB commands with explicit version information.
///
/// This function allows overriding the version information from the transaction
/// with more accurate historical versions (e.g., from gRPC `changed_objects` or
/// `unchanged_loaded_runtime_objects` fields).
///
/// The `object_versions` map should contain object IDs (hex strings) mapped to
/// their historical versions at transaction execution time.
///
/// # Arguments
/// * `tx` - The fetched transaction to convert
/// * `cached_objects` - Map of object IDs to base64-encoded BCS bytes
/// * `object_versions` - Map of object IDs to their versions (overrides TransactionInput versions)
///
/// # Example
/// ```ignore
/// let mut versions = HashMap::new();
/// // From gRPC response:
/// for (id, ver) in grpc_tx.changed_objects {
///     versions.insert(id, ver);
/// }
/// for (id, ver) in grpc_tx.unchanged_loaded_runtime_objects {
///     versions.insert(id, ver);
/// }
///
/// let (inputs, commands) = to_ptb_commands_with_versions(&tx, &objects, &versions)?;
/// ```
pub fn to_ptb_commands_with_versions(
    tx: &FetchedTransaction,
    cached_objects: &std::collections::HashMap<String, String>,
    object_versions: &std::collections::HashMap<String, u64>,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    const DEFAULT_GAS_BALANCE: u64 = 1_000_000_000_000_000_000;
    to_ptb_commands_internal_with_versions(tx, DEFAULT_GAS_BALANCE, cached_objects, object_versions)
}

/// Internal method that converts to PTB commands with gas balance and optional cached objects.
fn to_ptb_commands_internal(
    tx: &FetchedTransaction,
    gas_balance: u64,
    cached_objects: &std::collections::HashMap<String, String>,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    let mut inputs = Vec::new();
    let mut commands = Vec::new();

    // Helper to parse object ID with proper error handling
    let parse_object_id = |object_id: &str| -> Result<AccountAddress> {
        AccountAddress::from_hex_literal(object_id).map_err(|e| {
            anyhow::anyhow!("invalid object ID '{}': {}", object_id, e)
        })
    };

    // Helper to get object bytes from cache.
    // Returns an error if the object is not found - missing objects should be
    // fetched before replay, not silently replaced with placeholders.
    let get_object_bytes = |object_id: &str| -> Result<Vec<u8>> {
        cached_objects
            .get(object_id)
            .ok_or_else(|| anyhow::anyhow!("object '{}' not found in cache - ensure all input objects are fetched before replay", object_id))
            .and_then(|b64| {
                base64::engine::general_purpose::STANDARD.decode(b64)
                    .map_err(|e| anyhow::anyhow!("failed to decode object '{}' from base64: {}", object_id, e))
            })
    };

    // Check if any command uses GasCoin
    let uses_gas_coin = tx.commands.iter().any(|cmd| match cmd {
        PtbCommand::SplitCoins { coin, .. } => matches!(coin, PtbArgument::GasCoin),
        PtbCommand::MergeCoins {
            destination,
            sources,
        } => {
            matches!(destination, PtbArgument::GasCoin)
                || sources.iter().any(|s| matches!(s, PtbArgument::GasCoin))
        }
        PtbCommand::TransferObjects { objects, .. } => {
            objects.iter().any(|o| matches!(o, PtbArgument::GasCoin))
        }
        _ => false,
    });

    // Input index offset: if we prepend GasCoin, all other input indices shift by 1
    let input_offset: u16 = if uses_gas_coin { 1 } else { 0 };

    // If uses GasCoin, prepend a synthetic gas coin object
    if uses_gas_coin {
        // Create a synthetic Coin<SUI> with the gas budget as balance
        // Coin<T> layout: id (UID = 32 bytes) + balance (u64 = 8 bytes) = 40 bytes
        let mut gas_coin_bytes = vec![0u8; 32]; // UID (placeholder)
        gas_coin_bytes.extend_from_slice(&gas_balance.to_le_bytes()); // Balance
        inputs.push(InputValue::Object(ObjectInput::Owned {
            id: AccountAddress::ZERO, // Placeholder gas coin ID
            bytes: gas_coin_bytes,
            type_tag: None, // Gas coin type is known to be Coin<SUI>
            version: None,
        }));
    }

    // Convert inputs, using cached object data when available
    for input in &tx.inputs {
        match input {
            TransactionInput::Pure { bytes } => {
                inputs.push(InputValue::Pure(bytes.clone()));
            }
            TransactionInput::Object {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                inputs.push(InputValue::Object(ObjectInput::Owned {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(*version),
                }));
            }
            TransactionInput::SharedObject {
                object_id,
                initial_shared_version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                inputs.push(InputValue::Object(ObjectInput::Shared {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(*initial_shared_version),
                }));
            }
            TransactionInput::ImmutableObject {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                // Use ImmRef for immutable objects (e.g., packages, Clock)
                inputs.push(InputValue::Object(ObjectInput::ImmRef {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(*version),
                }));
            }
            TransactionInput::Receiving {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                // Use the Receiving variant to properly track receiving object semantics.
                // The parent_id is not available from TransactionInput alone - it would
                // need to be determined from the object's on-chain owner field or by
                // examining the transaction arguments.
                inputs.push(InputValue::Object(ObjectInput::Receiving {
                    id,
                    bytes,
                    type_tag: None,
                    parent_id: None, // Unknown from transaction input data
                    version: Some(*version),
                }));
            }
        }
    }

    // Helper to convert arguments with input offset
    let convert_arg = |arg: &PtbArgument| -> Argument {
        match arg {
            PtbArgument::Input { index } => Argument::Input(index + input_offset),
            PtbArgument::Result { index } => Argument::Result(*index),
            PtbArgument::NestedResult {
                index,
                result_index,
            } => Argument::NestedResult(*index, *result_index),
            PtbArgument::GasCoin => Argument::Input(0), // GasCoin is always input 0 (prepended)
        }
    };

    // Convert commands (with input offset if using GasCoin)
    for cmd in &tx.commands {
        match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => {
                let package_addr = AccountAddress::from_hex_literal(package)
                    .map_err(|e| anyhow!("Invalid package address: {}", e))?;
                let module_id = Identifier::new(module.clone())
                    .map_err(|e| anyhow!("Invalid module name: {}", e))?;
                let function_id = Identifier::new(function.clone())
                    .map_err(|e| anyhow!("Invalid function name: {}", e))?;

                // Parse type arguments from RPC strings
                let type_args: Vec<TypeTag> = type_arguments
                    .iter()
                    .filter_map(|s| parse_type_tag(s).ok())
                    .collect();

                // Convert arguments
                let args: Vec<Argument> = arguments.iter().map(&convert_arg).collect();

                commands.push(Command::MoveCall {
                    package: package_addr,
                    module: module_id,
                    function: function_id,
                    type_args,
                    args,
                });
            }

            PtbCommand::SplitCoins { coin, amounts } => {
                let coin_arg = convert_arg(coin);
                let amount_args: Vec<Argument> = amounts.iter().map(&convert_arg).collect();
                commands.push(Command::SplitCoins {
                    coin: coin_arg,
                    amounts: amount_args,
                });
            }

            PtbCommand::MergeCoins {
                destination,
                sources,
            } => {
                let dest_arg = convert_arg(destination);
                let source_args: Vec<Argument> = sources.iter().map(&convert_arg).collect();
                commands.push(Command::MergeCoins {
                    destination: dest_arg,
                    sources: source_args,
                });
            }

            PtbCommand::TransferObjects { objects, address } => {
                let obj_args: Vec<Argument> = objects.iter().map(&convert_arg).collect();
                let addr_arg = convert_arg(address);
                commands.push(Command::TransferObjects {
                    objects: obj_args,
                    address: addr_arg,
                });
            }

            PtbCommand::MakeMoveVec { type_arg, elements } => {
                let type_tag = type_arg.as_ref().and_then(|s| parse_type_tag(s).ok());
                let elem_args: Vec<Argument> = elements.iter().map(&convert_arg).collect();
                commands.push(Command::MakeMoveVec {
                    type_tag,
                    elements: elem_args,
                });
            }

            PtbCommand::Publish { .. } | PtbCommand::Upgrade { .. } => {
                // Skip publish/upgrade for now
            }
        }
    }

    Ok((inputs, commands))
}

/// Internal method that converts to PTB commands with explicit version information.
///
/// The `object_versions` map allows overriding versions from TransactionInput with
/// more accurate historical versions (e.g., from gRPC response).
fn to_ptb_commands_internal_with_versions(
    tx: &FetchedTransaction,
    gas_balance: u64,
    cached_objects: &std::collections::HashMap<String, String>,
    object_versions: &std::collections::HashMap<String, u64>,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    let mut inputs = Vec::new();
    let mut commands = Vec::new();

    // Helper to parse object ID with proper error handling
    let parse_object_id = |object_id: &str| -> Result<AccountAddress> {
        AccountAddress::from_hex_literal(object_id).map_err(|e| {
            anyhow::anyhow!("invalid object ID '{}': {}", object_id, e)
        })
    };

    // Helper to get object bytes from cache
    let get_object_bytes = |object_id: &str| -> Result<Vec<u8>> {
        cached_objects
            .get(object_id)
            .ok_or_else(|| anyhow::anyhow!("object '{}' not found in cache", object_id))
            .and_then(|b64| {
                base64::engine::general_purpose::STANDARD.decode(b64)
                    .map_err(|e| anyhow::anyhow!("failed to decode object '{}': {}", object_id, e))
            })
    };

    // Helper to get version for an object - prefers external map, falls back to input version
    let get_version = |object_id: &str, input_version: u64| -> u64 {
        object_versions.get(object_id).copied().unwrap_or(input_version)
    };

    // Check if any command uses GasCoin
    let uses_gas_coin = tx.commands.iter().any(|cmd| match cmd {
        PtbCommand::SplitCoins { coin, .. } => matches!(coin, PtbArgument::GasCoin),
        PtbCommand::MergeCoins { destination, sources } => {
            matches!(destination, PtbArgument::GasCoin)
                || sources.iter().any(|s| matches!(s, PtbArgument::GasCoin))
        }
        PtbCommand::TransferObjects { objects, .. } => {
            objects.iter().any(|o| matches!(o, PtbArgument::GasCoin))
        }
        _ => false,
    });

    let input_offset: u16 = if uses_gas_coin { 1 } else { 0 };

    if uses_gas_coin {
        let mut gas_coin_bytes = vec![0u8; 32];
        gas_coin_bytes.extend_from_slice(&gas_balance.to_le_bytes());
        inputs.push(InputValue::Object(ObjectInput::Owned {
            id: AccountAddress::ZERO,
            bytes: gas_coin_bytes,
            type_tag: None,
            version: None, // Synthetic gas coin has no real version
        }));
    }

    // Convert inputs with version information
    for input in &tx.inputs {
        match input {
            TransactionInput::Pure { bytes } => {
                inputs.push(InputValue::Pure(bytes.clone()));
            }
            TransactionInput::Object {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                let ver = get_version(object_id, *version);
                inputs.push(InputValue::Object(ObjectInput::Owned {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(ver),
                }));
            }
            TransactionInput::SharedObject {
                object_id,
                initial_shared_version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                let ver = get_version(object_id, *initial_shared_version);
                inputs.push(InputValue::Object(ObjectInput::Shared {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(ver),
                }));
            }
            TransactionInput::ImmutableObject {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                let ver = get_version(object_id, *version);
                inputs.push(InputValue::Object(ObjectInput::ImmRef {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(ver),
                }));
            }
            TransactionInput::Receiving {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                let ver = get_version(object_id, *version);
                inputs.push(InputValue::Object(ObjectInput::Receiving {
                    id,
                    bytes,
                    type_tag: None,
                    parent_id: None,
                    version: Some(ver),
                }));
            }
        }
    }

    // Convert arguments helper
    let convert_arg = |arg: &PtbArgument| -> Argument {
        match arg {
            PtbArgument::Input { index } => Argument::Input(index + input_offset),
            PtbArgument::Result { index } => Argument::Result(*index),
            PtbArgument::NestedResult { index, result_index } => {
                Argument::NestedResult(*index, *result_index)
            }
            PtbArgument::GasCoin => Argument::Input(0),
        }
    };

    // Convert commands
    for cmd in &tx.commands {
        match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => {
                let package_addr = AccountAddress::from_hex_literal(package)
                    .map_err(|e| anyhow::anyhow!("invalid package address: {}", e))?;
                let module_id = Identifier::new(module.clone())?;
                let function_id = Identifier::new(function.clone())?;
                let type_args: Vec<TypeTag> = type_arguments
                    .iter()
                    .filter_map(|s| parse_type_tag(s).ok())
                    .collect();
                let args: Vec<Argument> = arguments.iter().map(&convert_arg).collect();

                commands.push(Command::MoveCall {
                    package: package_addr,
                    module: module_id,
                    function: function_id,
                    type_args,
                    args,
                });
            }
            PtbCommand::SplitCoins { coin, amounts } => {
                commands.push(Command::SplitCoins {
                    coin: convert_arg(coin),
                    amounts: amounts.iter().map(&convert_arg).collect(),
                });
            }
            PtbCommand::MergeCoins { destination, sources } => {
                commands.push(Command::MergeCoins {
                    destination: convert_arg(destination),
                    sources: sources.iter().map(&convert_arg).collect(),
                });
            }
            PtbCommand::TransferObjects { objects, address } => {
                commands.push(Command::TransferObjects {
                    objects: objects.iter().map(&convert_arg).collect(),
                    address: convert_arg(address),
                });
            }
            PtbCommand::MakeMoveVec { type_arg, elements } => {
                commands.push(Command::MakeMoveVec {
                    type_tag: type_arg.as_ref().and_then(|s| parse_type_tag(s).ok()),
                    elements: elements.iter().map(&convert_arg).collect(),
                });
            }
            PtbCommand::Publish { .. } | PtbCommand::Upgrade { .. } => {
                // Skip publish/upgrade for now
            }
        }
    }

    Ok((inputs, commands))
}

/// Internal method with address aliasing support for package upgrades.
fn to_ptb_commands_internal_with_aliases(
    tx: &FetchedTransaction,
    gas_balance: u64,
    cached_objects: &std::collections::HashMap<String, String>,
    address_aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    let mut inputs = Vec::new();
    let mut commands = Vec::new();

    // Helper to parse object ID with proper error handling
    let parse_object_id = |object_id: &str| -> Result<AccountAddress> {
        AccountAddress::from_hex_literal(object_id).map_err(|e| {
            anyhow::anyhow!("invalid object ID '{}': {}", object_id, e)
        })
    };

    // Helper to get object bytes from cache.
    // Returns an error if the object is not found - missing objects should be
    // fetched before replay, not silently replaced with placeholders.
    let get_object_bytes = |object_id: &str| -> Result<Vec<u8>> {
        cached_objects
            .get(object_id)
            .ok_or_else(|| anyhow::anyhow!("object '{}' not found in cache - ensure all input objects are fetched before replay", object_id))
            .and_then(|b64| {
                base64::engine::general_purpose::STANDARD.decode(b64)
                    .map_err(|e| anyhow::anyhow!("failed to decode object '{}' from base64: {}", object_id, e))
            })
    };

    // Helper to rewrite address if aliased
    let rewrite_addr = |addr: AccountAddress| -> AccountAddress {
        address_aliases.get(&addr).copied().unwrap_or(addr)
    };

    // Helper to rewrite addresses in type tags
    fn rewrite_type_tag(
        tag: TypeTag,
        aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
    ) -> TypeTag {
        match tag {
            TypeTag::Struct(s) => {
                let mut s = *s;
                s.address = aliases.get(&s.address).copied().unwrap_or(s.address);
                s.type_params = s
                    .type_params
                    .into_iter()
                    .map(|t| rewrite_type_tag(t, aliases))
                    .collect();
                TypeTag::Struct(Box::new(s))
            }
            TypeTag::Vector(inner) => TypeTag::Vector(Box::new(rewrite_type_tag(*inner, aliases))),
            other => other,
        }
    }

    // Check if any command uses GasCoin
    let uses_gas_coin = tx.commands.iter().any(|cmd| match cmd {
        PtbCommand::SplitCoins { coin, .. } => matches!(coin, PtbArgument::GasCoin),
        PtbCommand::MergeCoins {
            destination,
            sources,
        } => {
            matches!(destination, PtbArgument::GasCoin)
                || sources.iter().any(|s| matches!(s, PtbArgument::GasCoin))
        }
        PtbCommand::TransferObjects { objects, .. } => {
            objects.iter().any(|o| matches!(o, PtbArgument::GasCoin))
        }
        _ => false,
    });

    let input_offset: u16 = if uses_gas_coin { 1 } else { 0 };

    if uses_gas_coin {
        let mut gas_coin_bytes = vec![0u8; 32];
        gas_coin_bytes.extend_from_slice(&gas_balance.to_le_bytes());
        inputs.push(InputValue::Object(ObjectInput::Owned {
            id: AccountAddress::ZERO,
            bytes: gas_coin_bytes,
            type_tag: None,
            version: None,
        }));
    }

    // Convert inputs
    for input in &tx.inputs {
        match input {
            TransactionInput::Pure { bytes } => {
                inputs.push(InputValue::Pure(bytes.clone()));
            }
            TransactionInput::Object {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                inputs.push(InputValue::Object(ObjectInput::Owned {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(*version),
                }));
            }
            TransactionInput::SharedObject {
                object_id,
                initial_shared_version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                inputs.push(InputValue::Object(ObjectInput::Shared {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(*initial_shared_version),
                }));
            }
            TransactionInput::ImmutableObject {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                inputs.push(InputValue::Object(ObjectInput::ImmRef {
                    id,
                    bytes,
                    type_tag: None,
                    version: Some(*version),
                }));
            }
            TransactionInput::Receiving {
                object_id,
                version,
                ..
            } => {
                let id = parse_object_id(object_id)?;
                let bytes = get_object_bytes(object_id)?;
                // Use the Receiving variant for proper semantics.
                // Parent_id is not available from TransactionInput data alone.
                inputs.push(InputValue::Object(ObjectInput::Receiving {
                    id,
                    bytes,
                    type_tag: None,
                    parent_id: None,
                    version: Some(*version),
                }));
            }
        }
    }

    let convert_arg = |arg: &PtbArgument| -> Argument {
        match arg {
            PtbArgument::Input { index } => Argument::Input(index + input_offset),
            PtbArgument::Result { index } => Argument::Result(*index),
            PtbArgument::NestedResult {
                index,
                result_index,
            } => Argument::NestedResult(*index, *result_index),
            PtbArgument::GasCoin => Argument::Input(0),
        }
    };

    // Convert commands with address rewriting
    for cmd in &tx.commands {
        match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => {
                let package_addr = AccountAddress::from_hex_literal(package)
                    .map_err(|e| anyhow!("Invalid package address: {}", e))?;
                // Rewrite package address to bytecode self-address
                let rewritten_package = rewrite_addr(package_addr);
                let module_id = Identifier::new(module.clone())
                    .map_err(|e| anyhow!("Invalid module name: {}", e))?;
                let function_id = Identifier::new(function.clone())
                    .map_err(|e| anyhow!("Invalid function name: {}", e))?;

                // Parse and rewrite type arguments
                let type_args: Vec<TypeTag> = type_arguments
                    .iter()
                    .filter_map(|s| parse_type_tag(s).ok())
                    .map(|t| rewrite_type_tag(t, address_aliases))
                    .collect();

                let args: Vec<Argument> = arguments.iter().map(&convert_arg).collect();

                commands.push(Command::MoveCall {
                    package: rewritten_package,
                    module: module_id,
                    function: function_id,
                    type_args,
                    args,
                });
            }

            PtbCommand::SplitCoins { coin, amounts } => {
                commands.push(Command::SplitCoins {
                    coin: convert_arg(coin),
                    amounts: amounts.iter().map(&convert_arg).collect(),
                });
            }

            PtbCommand::MergeCoins {
                destination,
                sources,
            } => {
                commands.push(Command::MergeCoins {
                    destination: convert_arg(destination),
                    sources: sources.iter().map(&convert_arg).collect(),
                });
            }

            PtbCommand::TransferObjects { objects, address } => {
                commands.push(Command::TransferObjects {
                    objects: objects.iter().map(&convert_arg).collect(),
                    address: convert_arg(address),
                });
            }

            PtbCommand::MakeMoveVec { type_arg, elements } => {
                let type_tag = type_arg
                    .as_ref()
                    .and_then(|s| parse_type_tag(s).ok())
                    .map(|t| rewrite_type_tag(t, address_aliases));
                commands.push(Command::MakeMoveVec {
                    type_tag,
                    elements: elements.iter().map(&convert_arg).collect(),
                });
            }

            PtbCommand::Publish { .. } | PtbCommand::Upgrade { .. } => {
                // Skip publish/upgrade
            }
        }
    }

    Ok((inputs, commands))
}

/// Replay a transaction in the local VM.
///
/// This method executes the transaction commands using PTBExecutor and compares
/// the results with on-chain effects.
pub fn replay(tx: &FetchedTransaction, harness: &mut VMHarness) -> Result<ReplayResult> {
    replay_with_objects(tx, harness, &std::collections::HashMap::new())
}

/// Replay a transaction using cached object data.
pub fn replay_with_objects(
    tx: &FetchedTransaction,
    harness: &mut VMHarness,
    cached_objects: &std::collections::HashMap<String, String>,
) -> Result<ReplayResult> {
    replay_with_objects_and_aliases(
        tx,
        harness,
        cached_objects,
        &std::collections::HashMap::new(),
    )
}

/// Replay a transaction using cached object data and address aliases.
/// The aliases map on-chain package addresses to bytecode self-addresses.
pub fn replay_with_objects_and_aliases(
    tx: &FetchedTransaction,
    harness: &mut VMHarness,
    cached_objects: &std::collections::HashMap<String, String>,
    address_aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
) -> Result<ReplayResult> {
    replay_with_version_tracking(tx, harness, cached_objects, address_aliases, None)
}

/// Replay a transaction with full version tracking enabled.
///
/// This function enables version tracking in the PTBExecutor and uses the provided
/// `object_versions` map to populate input object versions. The resulting
/// `TransactionEffects` will contain `object_versions` with version change information.
///
/// # Arguments
/// * `tx` - The fetched transaction to replay
/// * `harness` - The VM harness for execution
/// * `cached_objects` - Map of object IDs to base64-encoded BCS bytes
/// * `address_aliases` - Map of on-chain addresses to bytecode self-addresses
/// * `object_versions` - Optional map of object IDs to their historical versions
///
/// # Returns
/// A `ReplayResult` containing execution results and version tracking information
/// in `effects.object_versions`.
pub fn replay_with_version_tracking(
    tx: &FetchedTransaction,
    harness: &mut VMHarness,
    cached_objects: &std::collections::HashMap<String, String>,
    address_aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
    object_versions: Option<&std::collections::HashMap<String, u64>>,
) -> Result<ReplayResult> {
    use crate::ptb::PTBExecutor;

    // Always use alias-aware conversion to handle package upgrades correctly.
    // If versions are provided, we'll add them to the inputs after conversion.
    let (mut inputs, commands) =
        to_ptb_commands_with_objects_and_aliases(tx, cached_objects, address_aliases)?;

    // If version tracking is enabled, add versions to the inputs
    if let Some(versions) = object_versions {
        for input in &mut inputs {
            if let InputValue::Object(obj_input) = input {
                let obj_id_hex = obj_input.id().to_hex_literal();
                if let Some(&ver) = versions.get(&obj_id_hex) {
                    obj_input.set_version(Some(ver));
                }
            }
        }
    }

    let commands_count = commands.len();

    // Execute using PTBExecutor
    let mut executor = PTBExecutor::new(harness);

    // Enable version tracking if versions are provided
    if let Some(versions) = object_versions {
        executor.set_track_versions(true);
        // Compute lamport timestamp from max version + 1
        let max_version = versions.values().copied().max().unwrap_or(0);
        executor.set_lamport_timestamp(max_version + 1);
    }

    // Add inputs to executor
    for input in &inputs {
        executor.add_input(input.clone());
    }

    // Execute commands
    let effects = match executor.execute_commands(&commands) {
        Ok(effects) => effects,
        Err(e) => {
            return Ok(ReplayResult {
                digest: tx.digest.clone(),
                local_success: false,
                local_error: Some(e.to_string()),
                comparison: None,
                commands_executed: 0,
                commands_failed: commands_count,
                objects_tracked: 0,
                lamport_timestamp: None,
                version_summary: None,
                gas_used: 0,
            });
        }
    };

    // Build version info for comparison if available
    let local_versions: Option<HashMap<String, LocalVersionInfo>> =
        effects.object_versions.as_ref().map(|vers| {
            vers.iter()
                .map(|(id, info)| {
                    (
                        id.to_hex_literal(),
                        LocalVersionInfo {
                            input_version: info.input_version,
                            output_version: info.output_version,
                        },
                    )
                })
                .collect()
        });

    // Compare with on-chain effects using version-aware comparison if versions provided
    let comparison = tx.effects.as_ref().map(|on_chain| {
        if object_versions.is_some() && local_versions.is_some() {
            EffectsComparison::compare_with_versions(
                on_chain,
                effects.success,
                effects.created.len(),
                effects.mutated.len(),
                effects.deleted.len(),
                local_versions.as_ref(),
                object_versions,
            )
        } else {
            EffectsComparison::compare(
                on_chain,
                effects.success,
                effects.created.len(),
                effects.mutated.len(),
                effects.deleted.len(),
            )
        }
    });

    // Build version summary
    let version_summary = effects.object_versions.as_ref().map(|vers| {
        use crate::ptb::VersionChangeType;
        let mut summary = VersionSummary::default();
        for info in vers.values() {
            match info.change_type {
                VersionChangeType::Created => summary.created += 1,
                VersionChangeType::Mutated => summary.mutated += 1,
                VersionChangeType::Deleted => summary.deleted += 1,
                VersionChangeType::Wrapped => summary.wrapped += 1,
            }
        }
        summary
    });

    Ok(ReplayResult {
        digest: tx.digest.clone(),
        local_success: effects.success,
        local_error: effects.error,
        comparison,
        commands_executed: if effects.success { commands_count } else { 0 },
        commands_failed: if effects.success { 0 } else { commands_count },
        objects_tracked: effects.object_versions.as_ref().map(|v| v.len()).unwrap_or(0),
        lamport_timestamp: effects.lamport_timestamp,
        version_summary,
        gas_used: effects.gas_used,
    })
}

/// Check if a transaction uses only framework packages (0x1, 0x2, 0x3).
pub fn uses_only_framework(tx: &FetchedTransaction) -> bool {
    let framework_addrs = [
        "0x0000000000000000000000000000000000000000000000000000000000000001",
        "0x0000000000000000000000000000000000000000000000000000000000000002",
        "0x0000000000000000000000000000000000000000000000000000000000000003",
        "0x1",
        "0x2",
        "0x3",
    ];

    for cmd in &tx.commands {
        if let PtbCommand::MoveCall { package, .. } = cmd {
            let is_framework = framework_addrs
                .iter()
                .any(|addr| package == *addr || package.to_lowercase() == addr.to_lowercase());
            if !is_framework {
                return false;
            }
        }
    }
    true
}

/// Get a list of third-party packages used by a transaction.
pub fn third_party_packages(tx: &FetchedTransaction) -> Vec<String> {
    let framework_addrs = [
        "0x0000000000000000000000000000000000000000000000000000000000000001",
        "0x0000000000000000000000000000000000000000000000000000000000000002",
        "0x0000000000000000000000000000000000000000000000000000000000000003",
        "0x1",
        "0x2",
        "0x3",
    ];

    let mut packages = std::collections::BTreeSet::new();
    for cmd in &tx.commands {
        if let PtbCommand::MoveCall { package, .. } = cmd {
            let is_framework = framework_addrs
                .iter()
                .any(|addr| package == *addr || package.to_lowercase() == addr.to_lowercase());
            if !is_framework {
                packages.insert(package.clone());
            }
        }
    }
    packages.into_iter().collect()
}

/// Get a summary of a transaction for display.
pub fn summary(tx: &FetchedTransaction) -> String {
    let status = tx
        .effects
        .as_ref()
        .map(|e| match &e.status {
            TransactionStatus::Success => "success".to_string(),
            TransactionStatus::Failure { error } => format!("failed: {}", error),
        })
        .unwrap_or_else(|| "unknown".to_string());

    format!(
        "Transaction {} from {} ({} commands, status: {})",
        tx.digest.0,
        tx.sender.to_hex_literal(),
        tx.commands.len(),
        status
    )
}

/// Convert CachedTransaction to PTB commands using cached object data.
pub fn cached_to_ptb_commands(
    cached: &CachedTransaction,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    to_ptb_commands_with_objects(&cached.transaction, &cached.objects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_digest() {
        let digest = TransactionDigest::new("abc123");
        assert_eq!(digest.0, "abc123");
    }

    /// Convert a PtbArgument to an Argument (test helper).
    fn convert_ptb_argument(arg: &PtbArgument) -> Argument {
        match arg {
            PtbArgument::Input { index } => Argument::Input(*index),
            PtbArgument::Result { index } => Argument::Result(*index),
            PtbArgument::NestedResult {
                index,
                result_index,
            } => Argument::NestedResult(*index, *result_index),
            PtbArgument::GasCoin => Argument::Input(0), // Gas coin is typically input 0
        }
    }

    #[test]
    fn test_ptb_argument_conversion() {
        let input = PtbArgument::Input { index: 5 };
        let arg = convert_ptb_argument(&input);
        assert!(matches!(arg, Argument::Input(5)));

        let result = PtbArgument::Result { index: 3 };
        let arg = convert_ptb_argument(&result);
        assert!(matches!(arg, Argument::Result(3)));

        let nested = PtbArgument::NestedResult {
            index: 2,
            result_index: 1,
        };
        let arg = convert_ptb_argument(&nested);
        assert!(matches!(arg, Argument::NestedResult(2, 1)));
    }

    #[test]
    fn test_transaction_status_serialization() {
        let success = TransactionStatus::Success;
        let json = serde_json::to_string(&success).unwrap();
        assert_eq!(json, "\"Success\"");

        let failure = TransactionStatus::Failure {
            error: "test error".to_string(),
        };
        let json = serde_json::to_string(&failure).unwrap();
        assert!(json.contains("test error"));
    }

    #[test]
    fn test_gas_summary_default() {
        let gas = GasSummary::default();
        assert_eq!(gas.computation_cost, 0);
        assert_eq!(gas.storage_cost, 0);
    }

    #[test]
    fn test_derive_dynamic_field_id() {
        // Test case from Cetus Pool's skip_list:
        // Parent UID: 0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3
        // Key type: u64
        // Key value: 481316
        // Expected object ID: 0x01aff7f7c58ba303e1d23df4ef9ccc1562d9bdcee7aeed813a3edb3a7f2b3704

        let parent = AccountAddress::from_hex_literal(
            "0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3",
        )
        .unwrap();

        let key: u64 = 481316;

        let derived = super::derive_dynamic_field_id_u64(parent, key).unwrap();

        let expected = AccountAddress::from_hex_literal(
            "0x01aff7f7c58ba303e1d23df4ef9ccc1562d9bdcee7aeed813a3edb3a7f2b3704",
        )
        .unwrap();

        assert_eq!(
            derived,
            expected,
            "Derived ID mismatch:\n  got:      {}\n  expected: {}",
            derived.to_hex_literal(),
            expected.to_hex_literal()
        );

        // Test another key value (key=0 for historical skip_list head)
        let key_0_derived = super::derive_dynamic_field_id_u64(parent, 0).unwrap();
        let key_0_expected = AccountAddress::from_hex_literal(
            "0x364f5bc3735b4aadfe4ff299163c407c8058ab7f014308ec62550a5306a1fb7f",
        )
        .unwrap();

        assert_eq!(
            key_0_derived,
            key_0_expected,
            "Derived ID for key=0 mismatch:\n  got:      {}\n  expected: {}",
            key_0_derived.to_hex_literal(),
            key_0_expected.to_hex_literal()
        );
    }
}

// ============================================================================
// GraphQL to FetchedTransaction Conversion
// ============================================================================

/// Convert a GraphQL transaction to the internal FetchedTransaction format.
///
/// This enables using transactions fetched via DataFetcher with the CachedTransaction
/// and replay infrastructure.
pub fn graphql_to_fetched_transaction(
    tx: &sui_transport::graphql::GraphQLTransaction,
) -> Result<FetchedTransaction> {
    use move_core_types::account_address::AccountAddress;
    use sui_transport::graphql::GraphQLTransactionInput;

    // Parse sender address
    let sender_hex = tx.sender.strip_prefix("0x").unwrap_or(&tx.sender);
    let sender = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))
        .map_err(|e| anyhow::anyhow!("Invalid sender address: {}", e))?;

    // Convert inputs
    let inputs: Vec<TransactionInput> = tx
        .inputs
        .iter()
        .map(|input| match input {
            GraphQLTransactionInput::Pure { bytes_base64 } => {
                // Decode base64 to bytes for Pure input
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(bytes_base64)
                    .unwrap_or_default();
                TransactionInput::Pure { bytes }
            }
            GraphQLTransactionInput::OwnedObject {
                address,
                version,
                digest,
            } => TransactionInput::Object {
                object_id: address.clone(),
                version: *version,
                digest: digest.clone(),
            },
            GraphQLTransactionInput::SharedObject {
                address,
                initial_shared_version,
                mutable,
            } => TransactionInput::SharedObject {
                object_id: address.clone(),
                initial_shared_version: *initial_shared_version,
                mutable: *mutable,
            },
            GraphQLTransactionInput::Receiving {
                address,
                version,
                digest,
            } => TransactionInput::Receiving {
                object_id: address.clone(),
                version: *version,
                digest: digest.clone(),
            },
        })
        .collect();

    // Convert commands
    let commands: Vec<PtbCommand> = tx
        .commands
        .iter()
        .filter_map(convert_graphql_command)
        .collect();

    // Convert effects
    let effects = tx.effects.as_ref().map(convert_graphql_effects);

    Ok(FetchedTransaction {
        digest: TransactionDigest(tx.digest.clone()),
        sender,
        gas_budget: tx.gas_budget.unwrap_or(0),
        gas_price: tx.gas_price.unwrap_or(0),
        commands,
        inputs,
        effects,
        timestamp_ms: tx.timestamp_ms,
        checkpoint: tx.checkpoint,
    })
}

/// Convert a GraphQL command to PtbCommand
fn convert_graphql_command(cmd: &sui_transport::graphql::GraphQLCommand) -> Option<PtbCommand> {
    use sui_transport::graphql::GraphQLCommand;

    match cmd {
        GraphQLCommand::MoveCall {
            package,
            module,
            function,
            type_arguments,
            arguments,
        } => Some(PtbCommand::MoveCall {
            package: package.clone(),
            module: module.clone(),
            function: function.clone(),
            type_arguments: type_arguments.clone(),
            arguments: arguments.iter().map(convert_graphql_argument).collect(),
        }),
        GraphQLCommand::TransferObjects { objects, address } => Some(PtbCommand::TransferObjects {
            objects: objects.iter().map(convert_graphql_argument).collect(),
            address: convert_graphql_argument(address),
        }),
        GraphQLCommand::SplitCoins { coin, amounts } => Some(PtbCommand::SplitCoins {
            coin: convert_graphql_argument(coin),
            amounts: amounts.iter().map(convert_graphql_argument).collect(),
        }),
        GraphQLCommand::MergeCoins {
            destination,
            sources,
        } => Some(PtbCommand::MergeCoins {
            destination: convert_graphql_argument(destination),
            sources: sources.iter().map(convert_graphql_argument).collect(),
        }),
        GraphQLCommand::MakeMoveVec { type_arg, elements } => Some(PtbCommand::MakeMoveVec {
            type_arg: type_arg.clone(),
            elements: elements.iter().map(convert_graphql_argument).collect(),
        }),
        GraphQLCommand::Publish {
            modules,
            dependencies,
        } => Some(PtbCommand::Publish {
            modules: modules.clone(),
            dependencies: dependencies.clone(),
        }),
        GraphQLCommand::Upgrade {
            package, ticket, ..
        } => Some(PtbCommand::Upgrade {
            modules: Vec::new(), // Upgrade modules not available in GraphQL response
            package: package.clone(),
            ticket: convert_graphql_argument(ticket),
        }),
        GraphQLCommand::Other { .. } => None, // Skip unknown command types
    }
}

/// Convert a GraphQL argument to PtbArgument
fn convert_graphql_argument(arg: &sui_transport::graphql::GraphQLArgument) -> PtbArgument {
    use sui_transport::graphql::GraphQLArgument;

    match arg {
        GraphQLArgument::GasCoin => PtbArgument::GasCoin,
        GraphQLArgument::Input(index) => PtbArgument::Input { index: *index },
        GraphQLArgument::Result(index) => PtbArgument::Result { index: *index },
        GraphQLArgument::NestedResult(index, result_idx) => PtbArgument::NestedResult {
            index: *index,
            result_index: *result_idx,
        },
    }
}

/// Convert GraphQL effects to TransactionEffectsSummary
fn convert_graphql_effects(
    effects: &sui_transport::graphql::GraphQLEffects,
) -> TransactionEffectsSummary {
    let status = if effects.status == "SUCCESS" {
        TransactionStatus::Success
    } else {
        TransactionStatus::Failure {
            error: effects.status.clone(),
        }
    };

    TransactionEffectsSummary {
        status,
        created: effects.created.iter().map(|o| o.address.clone()).collect(),
        mutated: effects.mutated.iter().map(|o| o.address.clone()).collect(),
        deleted: effects.deleted.clone(),
        wrapped: Vec::new(),
        unwrapped: Vec::new(),
        gas_used: GasSummary::default(),
        events_count: 0,
        shared_object_versions: std::collections::HashMap::new(),
    }
}

// ============================================================================
// gRPC to FetchedTransaction Conversion (re-exported from sui-prefetch)
// ============================================================================

pub use sui_prefetch::grpc_to_fetched_transaction;

// ============================================================================
// Auto-Fetch and Cache Functionality
// ============================================================================

// Note: extract_dependencies_from_bytecode has been moved to utilities::type_utils
// Use crate::utilities::extract_dependencies_from_bytecode instead

/// Extract all unique dependency addresses from a set of packages.
/// packages is HashMap<String, Vec<(module_name, bytecode_base64)>>
#[allow(dead_code)]
fn extract_all_dependencies(
    packages: &std::collections::HashMap<String, Vec<(String, String)>>,
) -> std::collections::BTreeSet<String> {
    use crate::utilities::extract_dependencies_from_bytecode;
    use std::collections::BTreeSet;

    let mut all_deps: BTreeSet<String> = BTreeSet::new();

    for modules in packages.values() {
        for (_name, bytecode_base64) in modules {
            if let Ok(bytecode) = base64::engine::general_purpose::STANDARD.decode(bytecode_base64)
            {
                for dep_addr in extract_dependencies_from_bytecode(&bytecode) {
                    all_deps.insert(dep_addr);
                }
            }
        }
    }

    all_deps
}

// Note: fetch_and_cache_transaction and load_or_fetch_transaction functions
// that depend on DataFetcher have been moved to the main crate's tx_replay module
// since they require network access via DataFetcher which is not available here.
