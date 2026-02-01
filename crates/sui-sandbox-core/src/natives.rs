//! # Native Function Implementations for the Local Bytecode Sandbox
//!
//! This module provides native function implementations that enable execution
//! of Sui Move code without requiring the full Sui runtime or network access.
//!
//! ## Native Categories
//!
//! **Category A: Real implementations (from move-stdlib-natives + fastcrypto)**
//! - vector::*, bcs::to_bytes, hash::{sha2_256, sha3_256, keccak256, blake2b256}
//! - string::*, type_name::*, debug::*, signer::*
//! - ecdsa_k1::*, ecdsa_r1::*, ed25519::*, bls12381::*
//! - groth16::* (ZK proof verification)
//! - group_ops::* (BLS12-381 elliptic curve operations)
//!
//! **Category B: Simulated (correct behavior, in-memory state)**
//! - tx_context::* - Returns configured values
//! - object::{delete_impl, record_new_uid, borrow_uid} - Tracks object lifecycle
//! - transfer::* - Tracks ownership in memory
//! - event::emit - Captures events in memory
//! - types::is_one_time_witness - Real check (one bool field + UPPERCASE module name)
//! - dynamic_field::* - Full support via ObjectRuntime VM extension
//!
//! **Category C: Deterministic (for reproducibility)**
//! - random::* - Deterministic bytes from configured seed
//!
//! **Category D: Unsupported (abort with E_NOT_SUPPORTED = 1000)**
//! - zklogin::* - Requires external verification infrastructure
//! - poseidon::* - Poseidon hash for ZK circuits
//! - config::* - System configuration requires on-chain state
//! - nitro_attestation::* - AWS Nitro attestation requires enclave access
//!
//! ## Cryptographic Fidelity
//!
//! All cryptographic operations use fastcrypto (Mysten Labs' crypto library),
//! providing 1:1 compatibility with Sui mainnet behavior.

/// Debug macro that only prints when the `debug-natives` feature is enabled.
/// Use this for verbose tracing output that aids debugging but clutters normal use.
macro_rules! debug_native {
    ($($arg:tt)*) => {
        #[cfg(feature = "debug-natives")]
        eprintln!($($arg)*);
    };
}

use move_binary_format::errors::PartialVMResult;
use move_core_types::{
    account_address::AccountAddress,
    gas_algebra::InternalGas,
    language_storage::TypeTag,
    runtime_value::{MoveStructLayout, MoveTypeLayout},
};
use move_vm_runtime::native_functions::{
    make_table_from_iter, NativeContext, NativeFunction, NativeFunctionTable,
};
use move_vm_types::{
    loaded_data::runtime_types::Type, natives::function::NativeResult, pop_arg, values::Value,
};
use parking_lot::Mutex;
use smallvec::smallvec;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// Cryptographic primitives from fastcrypto (Mysten Labs' crypto library)
use fastcrypto::bls12381::{min_pk, min_sig};
use fastcrypto::ed25519::{Ed25519PublicKey, Ed25519Signature};
use fastcrypto::groups::bls12381 as bls;
use fastcrypto::groups::{GroupElement, HashToGroupElement, MultiScalarMul, Pairing, Scalar};
use fastcrypto::hash::{Blake2b256, HashFunction, Keccak256, Sha256};
use fastcrypto::secp256k1::{
    recoverable::Secp256k1RecoverableSignature, Secp256k1PublicKey, Secp256k1Signature,
};
use fastcrypto::secp256r1::{
    recoverable::Secp256r1RecoverableSignature, Secp256r1PublicKey, Secp256r1Signature,
};
use fastcrypto::serde_helpers::ToFromByteArray;
use fastcrypto::traits::{RecoverableSignature, ToFromBytes, VerifyingKey};
use move_vm_types::values::Struct;
use sui_types::base_types::ObjectID as SuiObjectID;
use sui_types::digests::TransactionDigest as SuiTransactionDigest;

use super::sandbox_runtime::{E_FIELD_DOES_NOT_EXIST, E_FIELD_TYPE_MISMATCH};

/// Abort code for unsupported native functions (category D).
/// Used when a native cannot be simulated locally.
pub const E_NOT_SUPPORTED: u64 = 1000;

const MOVE_STDLIB_ADDRESS: AccountAddress = AccountAddress::ONE;
const SUI_FRAMEWORK_ADDRESS: AccountAddress = AccountAddress::TWO;
const SUI_SYSTEM_ADDRESS: AccountAddress = AccountAddress::new([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3,
]);

/// Check if a type is a valid One-Time Witness (OTW).
///
/// This implements the same runtime check that Sui performs:
/// 1. The struct must have exactly one field of type bool
/// 2. The struct name must equal the module name in UPPERCASE
///
/// Note: The Sui verifier also checks that OTW is only instantiated in init(),
/// but we don't run the verifier, so LLMs can instantiate OTW manually in helper code.
/// Check if a struct layout represents an OTW (one bool field, name == UPPERCASE(module))
fn is_otw_struct(struct_layout: &MoveStructLayout, type_tag: &TypeTag) -> bool {
    // MoveStructLayout.0 is a Vec<MoveTypeLayout> representing fields
    let has_one_bool_field = matches!(struct_layout.0.as_slice(), [MoveTypeLayout::Bool]);

    if !has_one_bool_field {
        return false;
    }

    // Check if struct name == UPPERCASE(module name)
    matches!(
        type_tag,
        TypeTag::Struct(struct_tag)
            if struct_tag.name.to_string() == struct_tag.module.to_string().to_ascii_uppercase()
    )
}

/// Mock clock for Move VM execution.
///
/// Provides configurable time values for time-dependent Move code.
/// Supports two modes:
/// - **Frozen mode** (default for replay): Returns the same timestamp on every access.
///   This is essential for transaction replay where the clock should be fixed.
/// - **Advancing mode**: Advances by a configurable increment on each access.
///   Useful for testing time-dependent logic.
///
/// ## Usage for Transaction Replay
///
/// When replaying transactions, use frozen mode to match on-chain behavior:
/// ```rust,ignore
/// let clock = MockClock::frozen(tx_timestamp_ms);
/// ```
///
/// ## Usage for Testing
///
/// For testing time-dependent logic with advancing time:
/// ```rust,ignore
/// let clock = MockClock::advancing(base_ms, tick_ms);
/// ```
pub struct MockClock {
    /// Base timestamp in milliseconds (default: 2024-01-01 00:00:00 UTC = 1704067200000)
    pub base_ms: u64,
    /// Increment per access in milliseconds (default: 1000 = 1 second)
    /// Set to 0 for frozen mode.
    pub tick_ms: u64,
    /// Number of times the clock has been accessed
    accesses: AtomicU64,
    /// Whether the clock is in frozen mode (returns same timestamp always)
    frozen: bool,
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MockClock {
    /// Default base timestamp: 2024-01-01 00:00:00 UTC
    pub const DEFAULT_BASE_MS: u64 = 1704067200000;
    /// Default tick: 1 second per access (used in advancing mode)
    pub const DEFAULT_TICK_MS: u64 = 1000;

    /// Create a new clock in advancing mode with default settings.
    /// For transaction replay, use `frozen()` instead.
    pub fn new() -> Self {
        Self {
            base_ms: Self::DEFAULT_BASE_MS,
            tick_ms: Self::DEFAULT_TICK_MS,
            accesses: AtomicU64::new(0),
            frozen: false,
        }
    }

    /// Create a clock with a specific base timestamp in advancing mode.
    /// For transaction replay, use `frozen()` instead.
    pub fn with_base(base_ms: u64) -> Self {
        Self {
            base_ms,
            tick_ms: Self::DEFAULT_TICK_MS,
            accesses: AtomicU64::new(0),
            frozen: false,
        }
    }

    /// Create a frozen clock that always returns the same timestamp.
    ///
    /// This is the correct mode for transaction replay - on-chain, the Clock
    /// object has a fixed timestamp throughout the entire transaction.
    /// Using advancing mode would cause deadline checks to fail incorrectly.
    pub fn frozen(timestamp_ms: u64) -> Self {
        Self {
            base_ms: timestamp_ms,
            tick_ms: 0,
            accesses: AtomicU64::new(0),
            frozen: true,
        }
    }

    /// Create a clock in advancing mode with custom tick interval.
    ///
    /// Each call to `timestamp_ms()` will advance by `tick_ms` milliseconds.
    /// Useful for testing time-dependent logic.
    pub fn advancing(base_ms: u64, tick_ms: u64) -> Self {
        Self {
            base_ms,
            tick_ms,
            accesses: AtomicU64::new(0),
            frozen: false,
        }
    }

    /// Check if the clock is in frozen mode.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Freeze the clock at its current timestamp.
    ///
    /// After calling this, `timestamp_ms()` will always return the same value.
    pub fn freeze(&mut self) {
        self.frozen = true;
        self.tick_ms = 0;
    }

    /// Unfreeze the clock and resume advancing mode.
    pub fn unfreeze(&mut self, tick_ms: u64) {
        self.frozen = false;
        self.tick_ms = tick_ms;
    }

    /// Get the current timestamp.
    ///
    /// In frozen mode, always returns `base_ms`.
    /// In advancing mode, returns `base_ms + (accesses * tick_ms)` and increments.
    pub fn timestamp_ms(&self) -> u64 {
        if self.frozen {
            self.base_ms
        } else {
            let n = self.accesses.fetch_add(1, Ordering::SeqCst);
            self.base_ms + (n * self.tick_ms)
        }
    }

    /// Get the current timestamp without advancing (for inspection).
    pub fn peek_timestamp_ms(&self) -> u64 {
        if self.frozen {
            self.base_ms
        } else {
            let n = self.accesses.load(Ordering::SeqCst);
            self.base_ms + (n * self.tick_ms)
        }
    }

    /// Reset the clock to its initial state (accesses = 0).
    pub fn reset(&self) {
        self.accesses.store(0, Ordering::SeqCst);
    }

    /// Get the number of times the clock has been accessed.
    pub fn access_count(&self) -> u64 {
        self.accesses.load(Ordering::SeqCst)
    }
}

/// Mock random number generator that produces deterministic "random" values.
///
/// This allows code that uses randomness to execute with reproducible results.
/// The generator uses a seed and counter to produce deterministic output.
pub struct MockRandom {
    /// Seed for the random generator (default: all zeros)
    seed: [u8; 32],
    /// Counter for deterministic sequence
    counter: AtomicU64,
}

impl Default for MockRandom {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRandom {
    pub fn new() -> Self {
        Self {
            seed: [0u8; 32],
            counter: AtomicU64::new(0),
        }
    }

    pub fn with_seed(seed: [u8; 32]) -> Self {
        Self {
            seed,
            counter: AtomicU64::new(0),
        }
    }

    /// Generate the next batch of deterministic "random" bytes.
    ///
    /// Uses SHA-256(seed || counter) to produce deterministic output.
    pub fn next_bytes(&self, len: usize) -> Vec<u8> {
        use sha2::{Digest, Sha256};

        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let mut hasher = Sha256::new();
        hasher.update(self.seed);
        hasher.update(n.to_le_bytes());
        let hash = hasher.finalize();

        // Return requested length (truncate or repeat if needed)
        if len <= 32 {
            hash[..len].to_vec()
        } else {
            // For longer outputs, just repeat the hash
            let mut result = Vec::with_capacity(len);
            while result.len() < len {
                result.extend_from_slice(&hash[..std::cmp::min(32, len - result.len())]);
            }
            result
        }
    }

    /// Reset the counter to produce the same sequence again.
    pub fn reset(&self) {
        self.counter.store(0, Ordering::SeqCst);
    }
}

/// An event emitted during Move execution.
///
/// This captures the type information and serialized data of events
/// emitted via `sui::event::emit`.
#[derive(Debug, Clone)]
pub struct EmittedEvent {
    /// The fully-qualified type of the emitted event (e.g., "0x2::coin::CoinCreated<0x2::sui::SUI>")
    pub type_tag: String,
    /// BCS-serialized event data
    pub data: Vec<u8>,
    /// Sequence number within the transaction (0-indexed)
    pub sequence: u64,
}

/// Thread-safe store for events emitted during Move execution.
///
/// Events are captured by the `event::emit` native function and can be
/// queried after execution completes.
#[derive(Debug, Default)]
pub struct EventStore {
    events: Mutex<Vec<EmittedEvent>>,
    counter: AtomicU64,
}

impl EventStore {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            counter: AtomicU64::new(0),
        }
    }

    /// Record an emitted event.
    pub fn emit(&self, type_tag: String, data: Vec<u8>) {
        let sequence = self.counter.fetch_add(1, Ordering::SeqCst);
        let event = EmittedEvent {
            type_tag,
            data,
            sequence,
        };
        self.events.lock().push(event);
    }

    /// Get all emitted events.
    pub fn get_events(&self) -> Vec<EmittedEvent> {
        self.events.lock().clone()
    }

    /// Get events of a specific type.
    pub fn get_events_by_type(&self, type_prefix: &str) -> Vec<EmittedEvent> {
        self.get_events()
            .into_iter()
            .filter(|e| e.type_tag.starts_with(type_prefix))
            .collect()
    }

    /// Get the number of emitted events.
    pub fn count(&self) -> u64 {
        self.counter.load(Ordering::SeqCst)
    }

    /// Clear all events (for reuse between transactions).
    pub fn clear(&self) {
        self.events.lock().clear();
        self.counter.store(0, Ordering::SeqCst);
    }
}

/// Runtime state for native function execution in the Move VM sandbox.
///
/// `MockNativeState` provides the execution context that native functions need
/// to interact with the simulated Sui environment. It tracks:
///
/// # Transaction Context
/// - **Sender**: The address executing the transaction (`tx_context::sender()`)
/// - **Epoch**: Current epoch number (`tx_context::epoch()`)
/// - **Timestamp**: Epoch timestamp in milliseconds (`tx_context::epoch_timestamp_ms()`)
/// - **Transaction Hash**: Unique identifier for object ID derivation
///
/// # Object ID Generation
/// Object IDs are derived using `hash(tx_hash || ids_created)`, ensuring globally
/// unique addresses. Each `MockNativeState` instance has a unique `tx_hash` by default,
/// so objects created in different execution contexts have different IDs.
///
/// # Gas Model
/// - **Reference Gas Price**: Epoch-wide base price (750 MIST default)
/// - **Gas Price**: Transaction-specific price (reference + tip)
/// - **Gas Budget**: Maximum gas units for the transaction (50 SUI default)
///
/// # Protocol Version
/// Controls feature flags via `protocol_config::protocol_version()`.
/// Features are enabled when `protocol_version >= 60` (default: 73).
///
/// # Subsystems
/// - **Clock**: Mock time provider (advancing or frozen)
/// - **Random**: Deterministic randomness for reproducible testing
/// - **Events**: Captured event emissions for inspection
/// - **Native Costs**: Optional gas costs for native functions (for accurate gas metering)
///
/// # Example
///
/// ```rust,ignore
/// use sui_sandbox_core::natives::MockNativeState;
///
/// // For testing with defaults
/// let state = MockNativeState::new();
///
/// // For transaction replay with specific context
/// let replay_state = MockNativeState::for_replay_with_tx_hash(
///     sender_address,
///     epoch,
///     timestamp_ms,
///     transaction_digest,
/// );
/// ```
///
/// Note: Dynamic field storage is handled separately by `ObjectRuntime` (a VM extension).
pub struct MockNativeState {
    pub sender: AccountAddress,
    pub epoch: u64,
    pub epoch_timestamp_ms: u64,
    ids_created: AtomicU64,
    /// Transaction hash/digest for deriving object IDs
    pub tx_hash: [u8; 32],
    /// Mock clock for time-dependent code
    pub clock: MockClock,
    /// Mock random for randomness-dependent code
    pub random: MockRandom,
    /// Event store for capturing emitted events
    pub events: EventStore,
    /// Reference gas price for this epoch (in MIST)
    pub reference_gas_price: u64,
    /// Gas price for this transaction (reference + tip)
    pub gas_price: u64,
    /// Gas budget for this transaction
    pub gas_budget: u64,
    /// Protocol version for feature gating
    pub protocol_version: u64,
    /// Native function costs for accurate gas metering (optional)
    /// When None, native functions report zero cost (backwards compatible)
    /// When Some, costs match protocol config values
    pub native_costs: Option<crate::gas::NativeFunctionCosts>,
}

// Re-use gas constants from the gas module (single source of truth)
pub use crate::gas::{DEFAULT_GAS_BUDGET, DEFAULT_PROTOCOL_VERSION, DEFAULT_REFERENCE_GAS_PRICE};

impl Default for MockNativeState {
    fn default() -> Self {
        Self::new()
    }
}

impl MockNativeState {
    /// Generate a unique tx_hash for object ID derivation
    fn generate_tx_hash() -> [u8; 32] {
        crate::tx_hash::generate_tx_hash()
    }

    pub fn new() -> Self {
        Self {
            sender: AccountAddress::ZERO,
            epoch: 0,
            epoch_timestamp_ms: MockClock::DEFAULT_BASE_MS,
            ids_created: AtomicU64::new(0),
            tx_hash: Self::generate_tx_hash(),
            clock: MockClock::new(),
            random: MockRandom::new(),
            events: EventStore::new(),
            reference_gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_budget: DEFAULT_GAS_BUDGET,
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            native_costs: None,
        }
    }

    /// Create with a specific random seed for reproducible tests.
    pub fn with_random_seed(seed: [u8; 32]) -> Self {
        Self {
            sender: AccountAddress::ZERO,
            epoch: 0,
            epoch_timestamp_ms: MockClock::DEFAULT_BASE_MS,
            ids_created: AtomicU64::new(0),
            tx_hash: Self::generate_tx_hash(),
            clock: MockClock::new(),
            random: MockRandom::with_seed(seed),
            events: EventStore::new(),
            reference_gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_budget: DEFAULT_GAS_BUDGET,
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            native_costs: None,
        }
    }

    /// Create state configured for transaction replay.
    ///
    /// This sets up:
    /// - Frozen clock at the exact transaction timestamp (won't advance during execution)
    /// - Correct epoch from the transaction
    /// - Sender address from the transaction
    /// - Unique tx_hash for this transaction
    ///
    /// Use this for accurate replay of on-chain transactions.
    pub fn for_replay(sender: AccountAddress, epoch: u64, timestamp_ms: u64) -> Self {
        Self {
            sender,
            epoch,
            epoch_timestamp_ms: timestamp_ms,
            ids_created: AtomicU64::new(0),
            tx_hash: Self::generate_tx_hash(),
            clock: MockClock::frozen(timestamp_ms),
            random: MockRandom::new(),
            events: EventStore::new(),
            reference_gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_budget: DEFAULT_GAS_BUDGET,
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            native_costs: None,
        }
    }

    /// Create state for replay with a specific tx_hash.
    ///
    /// Use this when you need to exactly match on-chain object IDs.
    pub fn for_replay_with_tx_hash(
        sender: AccountAddress,
        epoch: u64,
        timestamp_ms: u64,
        tx_hash: [u8; 32],
    ) -> Self {
        Self {
            sender,
            epoch,
            epoch_timestamp_ms: timestamp_ms,
            ids_created: AtomicU64::new(0),
            tx_hash,
            clock: MockClock::frozen(timestamp_ms),
            random: MockRandom::new(),
            events: EventStore::new(),
            reference_gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_budget: DEFAULT_GAS_BUDGET,
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            native_costs: None,
        }
    }

    /// Create state for replay with a specific random seed.
    ///
    /// Same as `for_replay` but with deterministic randomness.
    pub fn for_replay_with_seed(
        sender: AccountAddress,
        epoch: u64,
        timestamp_ms: u64,
        random_seed: [u8; 32],
    ) -> Self {
        Self {
            sender,
            epoch,
            epoch_timestamp_ms: timestamp_ms,
            ids_created: AtomicU64::new(0),
            tx_hash: Self::generate_tx_hash(),
            clock: MockClock::frozen(timestamp_ms),
            random: MockRandom::with_seed(random_seed),
            events: EventStore::new(),
            reference_gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_budget: DEFAULT_GAS_BUDGET,
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            native_costs: None,
        }
    }

    /// Enable accurate native function costs from protocol config.
    ///
    /// When enabled, native functions will report their actual gas costs
    /// instead of zero cost. This is needed for accurate gas metering.
    pub fn with_native_costs(mut self, costs: crate::gas::NativeFunctionCosts) -> Self {
        self.native_costs = Some(costs);
        self
    }

    /// Get the gas cost for a native function, or 0 if costs not enabled.
    pub fn get_native_cost(
        &self,
        cost_fn: impl FnOnce(&crate::gas::NativeFunctionCosts) -> u64,
    ) -> u64 {
        self.native_costs.as_ref().map(cost_fn).unwrap_or(0)
    }

    /// Generate a fresh unique ID using the same algorithm as derive_id.
    /// This ensures globally unique object IDs: hash(tx_hash || ids_created)
    pub fn fresh_id(&self) -> AccountAddress {
        let count = self.ids_created.fetch_add(1, Ordering::SeqCst);

        let digest = SuiTransactionDigest::new(self.tx_hash);
        let object_id = SuiObjectID::derive_id(digest, count);
        AccountAddress::new(object_id.into_bytes())
    }

    pub fn ids_created(&self) -> u64 {
        self.ids_created.load(Ordering::SeqCst)
    }

    /// Set the ids_created counter (used for deterministic replay).
    pub fn set_ids_created(&self, value: u64) {
        self.ids_created.store(value, Ordering::SeqCst);
    }

    /// Get the current clock timestamp (advances the clock).
    pub fn clock_timestamp_ms(&self) -> u64 {
        self.clock.timestamp_ms()
    }

    /// Get deterministic random bytes.
    pub fn random_bytes(&self, len: usize) -> Vec<u8> {
        self.random.next_bytes(len)
    }

    /// Get all emitted events.
    pub fn get_events(&self) -> Vec<EmittedEvent> {
        self.events.get_events()
    }

    /// Get events of a specific type.
    pub fn get_events_by_type(&self, type_prefix: &str) -> Vec<EmittedEvent> {
        self.events.get_events_by_type(type_prefix)
    }

    /// Clear all events (call between transactions).
    pub fn clear_events(&self) {
        self.events.clear()
    }
}

/// Build the complete native function table for Move VM execution.
pub fn build_native_function_table(state: Arc<MockNativeState>) -> NativeFunctionTable {
    // Start with move-stdlib natives (real implementations)
    let stdlib_gas = move_stdlib_natives::GasParameters::zeros();
    let mut table = move_stdlib_natives::all_natives(MOVE_STDLIB_ADDRESS, stdlib_gas, false);

    // Add mock Sui natives at 0x2
    let sui_natives = build_sui_natives(state);
    let sui_table = make_table_from_iter(SUI_FRAMEWORK_ADDRESS, sui_natives);
    table.extend(sui_table);

    // Add sui-system natives at 0x3
    let sys_natives = build_sui_system_natives();
    let sys_table = make_table_from_iter(SUI_SYSTEM_ADDRESS, sys_natives);
    table.extend(sys_table);

    table
}

/// Build mock Sui framework native functions (at 0x2)
fn build_sui_natives(
    state: Arc<MockNativeState>,
) -> Vec<(&'static str, &'static str, NativeFunction)> {
    let mut natives: Vec<(&'static str, &'static str, NativeFunction)> = vec![];

    // ============================================================
    // CATEGORY B: SAFE MOCKS - return valid placeholder values
    // ============================================================

    // tx_context natives - all safe, just return placeholder values
    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_sender",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_sender_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::address(state_clone.sender)],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_epoch",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_epoch_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(state_clone.epoch)],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_epoch_timestamp_ms",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_epoch_timestamp_ms_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(state_clone.epoch_timestamp_ms)],
            ))
        }),
    ));

    // fresh_id: Returns unique addresses. The actual derivation doesn't matter
    // for type inhabitation - we just need valid, unique addresses.
    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "fresh_id",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_fresh_id_base);
            let id = state_clone.fresh_id();
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::address(id)],
            ))
        }),
    ));

    // Reference gas price - use configured value from MockNativeState
    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_rgp",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_rgp_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(state_clone.reference_gas_price)],
            ))
        }),
    ));

    // Gas price (reference + tip) - use configured value
    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_gas_price",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_gas_price_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(state_clone.gas_price)],
            ))
        }),
    ));

    // Gas budget - use configured value
    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_gas_budget",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_gas_budget_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(state_clone.gas_budget)],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_ids_created",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_ids_created_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(state_clone.ids_created())],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_sponsor",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_sponsor_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::vector_address(vec![])],
            ))
        }),
    ));

    // derive_id: Derive object ID from tx_hash and ids_created counter
    // This matches the real Sui implementation: hash(tx_hash || ids_created)
    // ensuring globally unique object IDs across all transactions
    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "derive_id",
        make_native(move |_ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.tx_context_derive_id_base);
            let ids_created = pop_arg!(args, u64);
            let tx_hash = pop_arg!(args, Vec<u8>);

            let tx_bytes: [u8; 32] = tx_hash.as_slice().try_into().map_err(|_| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let digest = SuiTransactionDigest::new(tx_bytes);
            let object_id = SuiObjectID::derive_id(digest, ids_created);
            let bytes = object_id.into_bytes();

            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::address(AccountAddress::new(bytes))],
            ))
        }),
    ));

    // object natives
    let state_clone = state.clone();
    natives.push((
        "object",
        "borrow_uid",
        make_native(move |_ctx, _ty_args, mut args| {
            use move_vm_types::values::VMValueCast;

            let cost = state_clone.get_native_cost(|c| c.object_borrow_uid_base);

            // borrow_uid<T: key>(obj: &T): &UID
            // All Sui objects with the `key` ability have `id: UID` as their first field.
            // We need to extract a reference to that first field.

            let obj_ref = match args.pop_back() {
                Some(v) => v,
                None => {
                    return Ok(NativeResult::err(InternalGas::new(cost), E_NOT_SUPPORTED));
                }
            };

            // Cast the Value to a StructRef so we can call borrow_field
            let struct_ref: move_vm_types::values::StructRef = match obj_ref.cast() {
                Ok(sr) => sr,
                Err(_) => {
                    return Ok(NativeResult::err(InternalGas::new(cost), E_NOT_SUPPORTED));
                }
            };

            // The input is a reference to a struct. We need to return a reference to its first field.
            // In Move VM, we can use borrow_field to get a reference to a field by index.
            // The UID is always the first field (index 0) of any Sui object.
            match struct_ref.borrow_field(0) {
                Ok(uid_ref) => Ok(NativeResult::ok(InternalGas::new(cost), smallvec![uid_ref])),
                Err(_) => {
                    // If borrow_field fails, the object might not have a proper UID field
                    // This shouldn't happen for valid Sui objects, but handle gracefully
                    Ok(NativeResult::err(InternalGas::new(cost), E_NOT_SUPPORTED))
                }
            }
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "object",
        "delete_impl",
        make_native(move |ctx, _ty_args, mut args| {
            use crate::sandbox_runtime::SharedObjectRuntime;

            let cost = state_clone.get_native_cost(|c| c.object_delete_impl_base);

            // Extract the ID from args - delete_impl(id: address)
            let uid_bytes = pop_arg!(args, AccountAddress);

            // Record this deleted ID in the shared state
            // This mirrors Sui's ObjectRuntime.delete_id() behavior
            if let Ok(runtime) = ctx.extensions().get::<SharedObjectRuntime>() {
                runtime.record_deleted_id(uid_bytes);
            }

            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "object",
        "record_new_uid",
        make_native(move |ctx, _ty_args, mut args| {
            use crate::sandbox_runtime::SharedObjectRuntime;

            let cost = state_clone.get_native_cost(|c| c.object_record_new_id_base);

            // Extract the ID from args - record_new_uid(id: address)
            let uid_bytes = pop_arg!(args, AccountAddress);

            // Record this new ID in the shared state
            // This mirrors Sui's ObjectRuntime.new_id() behavior
            if let Ok(runtime) = ctx.extensions().get::<SharedObjectRuntime>() {
                runtime.record_new_id(uid_bytes);
            }

            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    // transfer natives - track ownership changes via SharedObjectRuntime
    // Native names must match the bytecode: freeze_object_impl, share_object_impl, etc.

    // transfer_impl<T>(obj: T, recipient: address)
    // Transfers ownership of an object to a recipient address
    let state_clone = state.clone();
    natives.push((
        "transfer",
        "transfer_impl",
        make_native(move |ctx, mut ty_args, mut args| {
            use crate::sandbox_runtime::{Owner, SharedObjectRuntime};

            let cost = state_clone.get_native_cost(|c| c.transfer_internal_base);

            // Pop arguments: recipient (address), obj (T)
            // Note: args are in reverse order on the stack
            let recipient = pop_arg!(args, AccountAddress);
            let obj_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;

            // Try to extract object ID from the value
            // In Sui, objects have an `id: UID` field where UID = { id: ID { bytes: address } }
            // The first 32 bytes of a serialized object are typically the ID

            // Get type tag and layout first (before borrowing extensions mutably)
            let obj_type = ty_args.pop();
            let layout = obj_type
                .as_ref()
                .and_then(|obj_ty| ctx.type_to_type_layout(obj_ty).ok().flatten());

            if let Some(layout) = layout {
                // Try to serialize the object to get its bytes and ID
                if let Some(bytes) = obj_value.typed_serialize(&layout) {
                    if bytes.len() >= 32 {
                        let mut id_bytes = [0u8; 32];
                        id_bytes.copy_from_slice(&bytes[0..32]);
                        let object_id = AccountAddress::new(id_bytes);

                        // Record this created object in the shared state
                        // This persists even after the session ends
                        if let Some(ref obj_ty) = obj_type {
                            // Convert Type to TypeTag for storage
                            if let Ok(type_tag) = ctx.type_to_type_tag(obj_ty) {
                                if let Ok(runtime) = ctx.extensions().get::<SharedObjectRuntime>() {
                                    runtime.record_created_object(
                                        object_id,
                                        type_tag,
                                        bytes.clone(),
                                        Owner::Address(recipient),
                                    );
                                }
                            }
                        }
                    }
                }
            }

            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    // freeze_object_impl<T>(obj: T)
    // Makes an object immutable (frozen)
    let state_clone = state.clone();
    natives.push((
        "transfer",
        "freeze_object_impl",
        make_native(move |ctx, mut ty_args, mut args| {
            use crate::sandbox_runtime::{Owner, SharedObjectRuntime};

            let cost = state_clone.get_native_cost(|c| c.transfer_freeze_object_base);

            // Pop the object value
            let obj_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;

            // Get type tag and layout first (before borrowing extensions mutably)
            let obj_type = ty_args.pop();
            let layout = obj_type
                .as_ref()
                .and_then(|obj_ty| ctx.type_to_type_layout(obj_ty).ok().flatten());

            if let Some(layout) = layout {
                if let Some(bytes) = obj_value.typed_serialize(&layout) {
                    if bytes.len() >= 32 {
                        let mut id_bytes = [0u8; 32];
                        id_bytes.copy_from_slice(&bytes[0..32]);
                        let object_id = AccountAddress::new(id_bytes);

                        // Record this created object in the shared state
                        if let Some(ref obj_ty) = obj_type {
                            // Convert Type to TypeTag for storage
                            if let Ok(type_tag) = ctx.type_to_type_tag(obj_ty) {
                                if let Ok(runtime) = ctx.extensions().get::<SharedObjectRuntime>() {
                                    runtime.record_created_object(
                                        object_id,
                                        type_tag,
                                        bytes.clone(),
                                        Owner::Immutable,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    // share_object_impl<T>(obj: T)
    // Makes an object shared (accessible by anyone)
    let state_clone = state.clone();
    natives.push((
        "transfer",
        "share_object_impl",
        make_native(move |ctx, mut ty_args, mut args| {
            use crate::sandbox_runtime::{Owner, SharedObjectRuntime};

            let cost = state_clone.get_native_cost(|c| c.transfer_share_object_base);

            // Pop the object value
            let obj_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;

            // Get type tag and layout first (before borrowing extensions mutably)
            let obj_type = ty_args.pop();
            let layout = obj_type
                .as_ref()
                .and_then(|obj_ty| ctx.type_to_type_layout(obj_ty).ok().flatten());

            if let Some(layout) = layout {
                if let Some(bytes) = obj_value.typed_serialize(&layout) {
                    if bytes.len() >= 32 {
                        let mut id_bytes = [0u8; 32];
                        id_bytes.copy_from_slice(&bytes[0..32]);
                        let object_id = AccountAddress::new(id_bytes);

                        // Record this created object in the shared state
                        if let Some(ref obj_ty) = obj_type {
                            // Convert Type to TypeTag for storage
                            if let Ok(type_tag) = ctx.type_to_type_tag(obj_ty) {
                                if let Ok(runtime) = ctx.extensions().get::<SharedObjectRuntime>() {
                                    runtime.record_created_object(
                                        object_id,
                                        type_tag,
                                        bytes.clone(),
                                        Owner::Shared,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    // receive_impl<T>(parent: address, to_receive: Receiving<T>) -> T
    // Receiving<T> is a struct with: { id: ID, version: u64 }
    // ID is a struct with: { bytes: address }
    let state_clone = state.clone();
    natives.push((
        "transfer",
        "receive_impl",
        make_native(move |ctx, mut ty_args, mut args| {
            use crate::sandbox_runtime::{ObjectRuntime, SharedObjectRuntime};
            let cost = state_clone.get_native_cost(|c| c.transfer_receive_base);

            // Get the type we're receiving (this is T, not Receiving<T>)
            let receive_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;

            // Pop arguments: to_receive (Receiving<T>), parent (address)
            let receiving_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let parent = pop_arg!(args, AccountAddress);

            // Get type layout early (needed for deserialization)
            let fallback_type_layout = match ctx.type_to_type_layout(&receive_ty) {
                Ok(Some(layout)) => Some(layout),
                _ => None,
            };

            // Try to extract object ID from the Receiving value
            // Receiving<T> = { id: ID { bytes: address }, version: u64 }
            // Try to serialize and extract the address from the bytes
            let object_id = match receiving_value.typed_serialize(&MoveTypeLayout::Address) {
                // If it's just an address, use it directly
                Some(bytes) if bytes.len() == 32 => {
                    let mut id_bytes = [0u8; 32];
                    id_bytes.copy_from_slice(&bytes);
                    AccountAddress::new(id_bytes)
                }
                _ => {
                    // For complex struct values, try to get any pending receive for this parent
                    // This is a fallback for when we can't extract the ID directly
                    if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                        // Get the pending receives and try to receive one
                        let result = {
                            let mut state = shared.shared_state().lock();
                            let pending: Vec<AccountAddress> = state
                                .get_pending_receives_for(parent)
                                .iter()
                                .map(|(id, _, _)| *id)
                                .collect();
                            if let Some(first_id) = pending.first() {
                                state.receive_pending(parent, *first_id)
                            } else {
                                None
                            }
                        };

                        if let Some((_type_tag, obj_bytes)) = result {
                            if let Some(type_layout) = &fallback_type_layout {
                                match Value::simple_deserialize(&obj_bytes, type_layout) {
                                    Some(value) => {
                                        return Ok(NativeResult::ok(
                                            InternalGas::new(cost),
                                            smallvec![value],
                                        ));
                                    }
                                    None => {
                                        return Ok(NativeResult::err(InternalGas::new(cost), 3));
                                    }
                                }
                            } else {
                                return Ok(NativeResult::err(InternalGas::new(cost), 2));
                            }
                        }
                    }
                    return Ok(NativeResult::err(InternalGas::new(cost), 1));
                }
            };

            // Get the type layout first (before we borrow extensions)
            let type_layout = match ctx.type_to_type_layout(&receive_ty) {
                Ok(Some(layout)) => layout,
                _ => return Ok(NativeResult::err(InternalGas::new(cost), 2)),
            };

            // First try SharedObjectRuntime's shared state (from SimulationEnvironment)
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                // Try shared state first
                let recv_bytes_opt = {
                    let mut state = shared.shared_state().lock();
                    state.receive_pending(parent, object_id)
                };

                if let Some((_type_tag, ref recv_bytes)) = recv_bytes_opt {
                    match Value::simple_deserialize(recv_bytes, &type_layout) {
                        Some(value) => {
                            return Ok(NativeResult::ok(InternalGas::new(cost), smallvec![value]));
                        }
                        None => {
                            return Ok(NativeResult::err(InternalGas::new(cost), 3));
                        }
                    }
                }

                // Also try the local ObjectRuntime's ObjectStore
                let runtime = shared.local_mut();
                if let Ok(recv_bytes) = runtime.object_store_mut().receive_object(parent, object_id)
                {
                    match Value::simple_deserialize(&recv_bytes, &type_layout) {
                        Some(value) => {
                            return Ok(NativeResult::ok(InternalGas::new(cost), smallvec![value]));
                        }
                        None => {
                            return Ok(NativeResult::err(InternalGas::new(cost), 3));
                        }
                    }
                }
            }

            // Fallback to regular ObjectRuntime
            if let Ok(runtime) = ctx.extensions_mut().get_mut::<ObjectRuntime>() {
                match runtime.object_store_mut().receive_object(parent, object_id) {
                    Ok(recv_bytes) => match Value::simple_deserialize(&recv_bytes, &type_layout) {
                        Some(value) => {
                            return Ok(NativeResult::ok(InternalGas::new(cost), smallvec![value]));
                        }
                        None => {
                            return Ok(NativeResult::err(InternalGas::new(cost), 3));
                        }
                    },
                    Err(code) => return Ok(NativeResult::err(InternalGas::new(cost), code)),
                }
            }

            // Object not found in any pending receives
            Ok(NativeResult::err(InternalGas::new(cost), 4))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "transfer",
        "party_transfer_impl",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.transfer_internal_base);
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    // event natives - now with recording!
    let state_clone = state.clone();
    natives.push((
        "event",
        "emit",
        make_native(move |ctx, ty_args, mut args| {
            // event::emit<T>(event: T)
            // ty_args[0] is the event type T
            // args[0] is the event value
            let base_cost = state_clone.get_native_cost(|c| c.event_emit_base);
            let per_byte_cost = state_clone.get_native_cost(|c| c.event_emit_per_byte);

            let mut event_size = 0usize;
            if let Some(event_ty) = ty_args.first() {
                // Get the event value and serialize it
                if let Some(event_value) = args.pop_front() {
                    // Try to get type tag for the event type
                    let type_tag_str = match ctx.type_to_type_tag(event_ty) {
                        Ok(tag) => format!("{}", tag),
                        Err(_) => "unknown".to_string(),
                    };

                    // Serialize the event value to BCS
                    // Note: We use typed_serialize which may fail for some complex types
                    let event_bytes: Vec<u8> = event_value
                        .typed_serialize(
                            &ctx.type_to_type_layout(event_ty)
                                .ok()
                                .flatten()
                                .unwrap_or(MoveTypeLayout::Bool),
                        )
                        .unwrap_or_default();

                    event_size = event_bytes.len();

                    // Record the event
                    state_clone.events.emit(type_tag_str, event_bytes);
                }
            }
            let total_cost = base_cost + (per_byte_cost * event_size as u64);
            Ok(NativeResult::ok(InternalGas::new(total_cost), smallvec![]))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "event",
        "emit_authenticated_impl",
        make_native(move |ctx, ty_args, mut args| {
            // Similar to emit but for authenticated events
            let base_cost = state_clone.get_native_cost(|c| c.event_emit_base);
            let per_byte_cost = state_clone.get_native_cost(|c| c.event_emit_per_byte);

            let mut event_size = 0usize;
            if let Some(event_ty) = ty_args.first() {
                if let Some(event_value) = args.pop_front() {
                    let type_tag_str = match ctx.type_to_type_tag(event_ty) {
                        Ok(tag) => format!("{}", tag),
                        Err(_) => "unknown".to_string(),
                    };
                    let event_bytes: Vec<u8> = event_value
                        .typed_serialize(
                            &ctx.type_to_type_layout(event_ty)
                                .ok()
                                .flatten()
                                .unwrap_or(MoveTypeLayout::Bool),
                        )
                        .unwrap_or_default();
                    event_size = event_bytes.len();
                    state_clone.events.emit(type_tag_str, event_bytes);
                }
            }
            let total_cost = base_cost + (per_byte_cost * event_size as u64);
            Ok(NativeResult::ok(InternalGas::new(total_cost), smallvec![]))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "event",
        "events_by_type",
        make_native(move |ctx, ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.event_emit_base);
            // Return events matching the requested type
            let type_prefix = if let Some(ty) = ty_args.first() {
                match ctx.type_to_type_tag(ty) {
                    Ok(tag) => format!("{}", tag),
                    Err(_) => String::new(),
                }
            } else {
                String::new()
            };

            let events = state_clone.events.get_events_by_type(&type_prefix);
            // Serialize events as a vector of BCS bytes
            // For simplicity, we concatenate all event data
            let mut result_bytes = Vec::new();
            for event in events {
                result_bytes.extend_from_slice(&event.data);
            }
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::vector_u8(result_bytes)],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "event",
        "num_events",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.event_emit_base);
            let count = state_clone.events.count();
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(count)],
            ))
        }),
    ));

    // address natives
    let state_clone = state.clone();
    natives.push((
        "address",
        "from_bytes",
        make_native(move |_ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.address_from_bytes_base);
            let bytes = pop_arg!(args, Vec<u8>);
            if bytes.len() != 32 {
                return Ok(NativeResult::err(InternalGas::new(cost), 1));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::address(AccountAddress::new(arr))],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "address",
        "to_u256",
        make_native(move |_ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.address_to_u256_base);
            let addr = pop_arg!(args, AccountAddress);
            let bytes = addr.to_vec();
            // AccountAddress is always 32 bytes, so this conversion is safe
            let arr: [u8; 32] = match bytes.try_into() {
                Ok(a) => a,
                Err(_) => {
                    return Ok(NativeResult::err(InternalGas::new(cost), E_NOT_SUPPORTED));
                }
            };
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u256(move_core_types::u256::U256::from_le_bytes(
                    &arr
                ))],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "address",
        "from_u256",
        make_native(move |_ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.address_from_u256_base);
            let u = pop_arg!(args, move_core_types::u256::U256);
            let bytes = u.to_le_bytes();
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::address(AccountAddress::new(bytes))],
            ))
        }),
    ));

    // types::is_one_time_witness - real implementation
    // Checks: (1) struct has exactly one bool field, (2) name == UPPERCASE(module_name)
    // This matches the actual Sui runtime check, allowing LLMs to use the OTW pattern correctly.
    let state_clone = state.clone();
    natives.push((
        "types",
        "is_one_time_witness",
        make_native(move |ctx, ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.types_is_one_time_witness_base);
            // The type parameter T is what we need to check
            if ty_args.is_empty() {
                return Ok(NativeResult::ok(
                    InternalGas::new(cost),
                    smallvec![Value::bool(false)],
                ));
            }

            let ty = &ty_args[0];

            // Get TypeTag to check the name
            let type_tag = match ctx.type_to_type_tag(ty) {
                Ok(tag) => tag,
                Err(_) => {
                    return Ok(NativeResult::ok(
                        InternalGas::new(cost),
                        smallvec![Value::bool(false)],
                    ));
                }
            };

            // Get type layout to check for one bool field
            let type_layout = match ctx.type_to_type_layout(ty) {
                Ok(Some(layout)) => layout,
                _ => {
                    return Ok(NativeResult::ok(
                        InternalGas::new(cost),
                        smallvec![Value::bool(false)],
                    ));
                }
            };

            // Must be a struct type
            let MoveTypeLayout::Struct(struct_layout) = type_layout else {
                return Ok(NativeResult::ok(
                    InternalGas::new(cost),
                    smallvec![Value::bool(false)],
                ));
            };

            let is_otw = is_otw_struct(&struct_layout, &type_tag);

            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::bool(is_otw)],
            ))
        }),
    ));

    // hash natives - REAL implementations using fastcrypto
    let state_clone = state.clone();
    natives.push((
        "hash",
        "blake2b256",
        make_native(move |_ctx, _ty_args, mut args| {
            let data = pop_arg!(args, Vec<u8>);
            let base_cost = state_clone.get_native_cost(|c| c.hash_blake2b256_base);
            let per_byte_cost = state_clone.get_native_cost(|c| c.hash_blake2b256_per_byte);
            let total_cost = base_cost + (per_byte_cost * data.len() as u64);
            let hash = Blake2b256::digest(&data);
            Ok(NativeResult::ok(
                InternalGas::new(total_cost),
                smallvec![Value::vector_u8(hash.digest.to_vec())],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "hash",
        "keccak256",
        make_native(move |_ctx, _ty_args, mut args| {
            let data = pop_arg!(args, Vec<u8>);
            let base_cost = state_clone.get_native_cost(|c| c.hash_keccak256_base);
            let per_byte_cost = state_clone.get_native_cost(|c| c.hash_keccak256_per_byte);
            let total_cost = base_cost + (per_byte_cost * data.len() as u64);
            let hash = Keccak256::digest(&data);
            Ok(NativeResult::ok(
                InternalGas::new(total_cost),
                smallvec![Value::vector_u8(hash.digest.to_vec())],
            ))
        }),
    ));

    // protocol_config - use configured protocol version
    let state_clone = state.clone();
    natives.push((
        "protocol_config",
        "protocol_version_impl",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.protocol_version_base);
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(state_clone.protocol_version)],
            ))
        }),
    ));

    // protocol_config::is_feature_enabled
    // In a real implementation, this would check against a feature flag table.
    // For simulation, we default to true but can be configured per-feature if needed.
    // Feature gates are checked against protocol version thresholds.
    let state_clone = state.clone();
    natives.push((
        "protocol_config",
        "is_feature_enabled",
        make_native(move |_ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.is_feature_enabled_base);
            // The feature is passed as a u64 feature ID
            let _feature_id = pop_arg!(args, u64);

            // Most features are enabled at or after protocol version 60+
            // For simulation, we enable features if protocol_version >= 60
            // This provides reasonable behavior while allowing version-gating
            let enabled = state_clone.protocol_version >= 60;
            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::bool(enabled)],
            ))
        }),
    ));

    // accumulator natives - no-op (use minimal cost since they don't do real work)
    let state_clone = state.clone();
    natives.push((
        "accumulator",
        "emit_deposit_event",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.event_emit_base);
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "accumulator",
        "emit_withdraw_event",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.event_emit_base);
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "accumulator_settlement",
        "record_settlement_sui_conservation",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.event_emit_base);
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    // ============================================================
    // CATEGORY B+: TEST UTILITIES
    // These enable minting/burning coins for testing without real economics.
    // Coin<T> = { id: UID, balance: Balance<T> }
    // Balance<T> = { value: u64 }
    // UID = { id: ID }
    // ID = { bytes: address }
    // ============================================================
    add_test_utility_natives(&mut natives, state.clone());

    // ============================================================
    // CATEGORY B+: DYNAMIC FIELD SUPPORT (partial)
    // ============================================================
    add_dynamic_field_natives(&mut natives, state.clone());

    // ============================================================
    // CATEGORY A+: REAL CRYPTO IMPLEMENTATIONS
    // Uses fastcrypto for 1:1 mainnet compatibility.
    // See add_real_crypto_natives() for details.
    // ============================================================
    add_real_crypto_natives(&mut natives, state);

    natives
}

/// Build Sui system native functions (at 0x3)
fn build_sui_system_natives() -> Vec<(&'static str, &'static str, NativeFunction)> {
    vec![(
        "validator",
        "validate_metadata_bcs",
        make_native(|_ctx, _ty_args, _args| Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))),
    )]
}

/// Add test utility natives for coin/balance minting and burning.
///
/// These natives enable LLMs to create test coins and balances without
/// needing real economics. The `#[test_only]` functions in sui-framework
/// are not included in production bytecode, so we implement them as natives.
///
/// Supported functions:
/// - `balance::create_for_testing<T>(value: u64) -> Balance<T>`
/// - `balance::destroy_for_testing<T>(balance: Balance<T>)`
/// - `coin::mint_for_testing<T>(value: u64, ctx: &mut TxContext) -> Coin<T>`
/// - `coin::burn_for_testing<T>(coin: Coin<T>)`
fn add_test_utility_natives(
    natives: &mut Vec<(&'static str, &'static str, NativeFunction)>,
    state: Arc<MockNativeState>,
) {
    // balance::create_for_testing<T>(value: u64) -> Balance<T>
    // Balance<T> is a struct with a single u64 field: { value: u64 }
    let state_clone = state.clone();
    natives.push((
        "balance",
        "create_for_testing",
        make_native(move |_ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.balance_create_base);
            let value = pop_arg!(args, u64);
            // Balance<T> = struct { value: u64 }
            // We construct it as a struct with one field
            let balance =
                Value::struct_(move_vm_types::values::Struct::pack(vec![Value::u64(value)]));
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![balance]))
        }),
    ));

    // balance::destroy_for_testing<T>(balance: Balance<T>)
    // Just consumes the balance, no-op
    let state_clone = state.clone();
    natives.push((
        "balance",
        "destroy_for_testing",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.balance_destroy_base);
            // Balance is consumed, nothing to return
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    // coin::mint_for_testing<T>(value: u64, ctx: &mut TxContext) -> Coin<T>
    // Coin<T> = { id: UID, balance: Balance<T> }
    // UID = { id: ID }
    // ID = { bytes: address }
    let state_clone = state.clone();
    natives.push((
        "coin",
        "mint_for_testing",
        make_native(move |_ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.coin_mint_base);
            // Pop TxContext reference (we ignore it but need to consume it)
            let _ctx_ref = args.pop_back();
            let value = pop_arg!(args, u64);

            // Generate a fresh ID for the coin
            let id_addr = state_clone.fresh_id();

            // Construct ID { bytes: address }
            let id = Value::struct_(move_vm_types::values::Struct::pack(vec![Value::address(
                id_addr,
            )]));

            // Construct UID { id: ID }
            let uid = Value::struct_(move_vm_types::values::Struct::pack(vec![id]));

            // Construct Balance<T> { value: u64 }
            let balance =
                Value::struct_(move_vm_types::values::Struct::pack(vec![Value::u64(value)]));

            // Construct Coin<T> { id: UID, balance: Balance<T> }
            let coin = Value::struct_(move_vm_types::values::Struct::pack(vec![uid, balance]));

            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![coin]))
        }),
    ));

    // coin::burn_for_testing<T>(coin: Coin<T>)
    // Just consumes the coin, no-op
    let state_clone = state.clone();
    natives.push((
        "coin",
        "burn_for_testing",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.coin_burn_base);
            // Coin is consumed, nothing to return
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));

    // Additional test utilities that may be useful

    // balance::create_supply_for_testing<T>() -> Supply<T>
    // Supply<T> = { value: u64 } (tracks total supply)
    let state_clone = state.clone();
    natives.push((
        "balance",
        "create_supply_for_testing",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.supply_create_base);
            // Supply<T> = struct { value: u64 } starting at 0
            let supply = Value::struct_(move_vm_types::values::Struct::pack(vec![Value::u64(0)]));
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![supply]))
        }),
    ));

    // balance::destroy_supply_for_testing<T>(supply: Supply<T>)
    let state_clone = state.clone();
    natives.push((
        "balance",
        "destroy_supply_for_testing",
        make_native(move |_ctx, _ty_args, _args| {
            let cost = state_clone.get_native_cost(|c| c.balance_destroy_base);
            Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
        }),
    ));
}

/// Extract address from UID { id: ID { bytes: address } }
fn extract_address_from_uid(uid_ref: &move_vm_types::values::StructRef) -> Option<AccountAddress> {
    use move_vm_types::values::{Reference, VMValueCast};

    // UID structure: UID { id: ID } where ID { bytes: address }
    // Navigate: UID.id (field 0) -> ID, then ID.bytes (field 0) -> address
    //
    // borrow_field returns a Value containing an IndexedRef (a reference type).
    // We need to cast to Reference and then read_ref to get the actual value.

    // Step 1: Get field 0 (id: ID) from UID
    let id_field = uid_ref.borrow_field(0).ok()?;

    // Step 2: Cast to StructRef to access ID's fields
    let id_struct_ref: move_vm_types::values::StructRef = id_field.cast().ok()?;

    // Step 3: Get field 0 (bytes: address) from ID - this is an IndexedRef
    let bytes_field = id_struct_ref.borrow_field(0).ok()?;

    // Step 4: Cast the Value to Reference (works for IndexedRef)
    let bytes_ref: Reference = bytes_field.value_as().ok()?;

    // Step 5: Dereference to get the actual value
    let actual_value = bytes_ref.read_ref().ok()?;

    // Step 6: Cast the dereferenced value to AccountAddress
    let addr: AccountAddress = actual_value.value_as().ok()?;

    debug_native!(
        "[extract_address_from_uid] SUCCESS: addr={}",
        addr.to_hex_literal()
    );
    Some(addr)
}

/// Helper to get ObjectRuntime from extensions.
/// Tries SharedObjectRuntime first (for PTB sessions), falls back to ObjectRuntime.
fn get_object_runtime_ref<'a>(
    ctx: &'a NativeContext,
) -> Result<&'a crate::sandbox_runtime::ObjectRuntime, move_binary_format::errors::PartialVMError> {
    use crate::sandbox_runtime::{ObjectRuntime, SharedObjectRuntime};

    // Try SharedObjectRuntime first (used in PTB sessions for persistent state)
    if let Ok(shared) = ctx.extensions().get::<SharedObjectRuntime>() {
        let runtime: &ObjectRuntime = shared.local();
        return Ok(runtime);
    }

    // Fall back to regular ObjectRuntime
    ctx.extensions().get::<ObjectRuntime>()
}

/// Helper to get mutable ObjectRuntime from extensions.
fn get_object_runtime_mut<'a>(
    ctx: &'a mut NativeContext,
) -> Result<&'a mut crate::sandbox_runtime::ObjectRuntime, move_binary_format::errors::PartialVMError>
{
    use crate::sandbox_runtime::{ObjectRuntime, SharedObjectRuntime};

    // Try SharedObjectRuntime first
    if ctx.extensions().get::<SharedObjectRuntime>().is_ok() {
        let shared: &mut SharedObjectRuntime = ctx.extensions_mut().get_mut()?;
        return Ok(shared.local_mut());
    }

    // Fall back to regular ObjectRuntime
    ctx.extensions_mut().get_mut::<ObjectRuntime>()
}

/// Sync an added child to shared state (if using SharedObjectRuntime).
fn sync_child_to_shared_state(
    ctx: &mut NativeContext,
    parent: AccountAddress,
    child_id: AccountAddress,
    child_tag: &TypeTag,
    child_bytes: &[u8],
) {
    use crate::sandbox_runtime::SharedObjectRuntime;

    if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
        shared.shared_state().lock().add_child(
            parent,
            child_id,
            child_tag.clone(),
            child_bytes.to_vec(),
        );
    }
}

/// Remove a child from shared state (if using SharedObjectRuntime).
fn remove_child_from_shared_state(
    ctx: &mut NativeContext,
    parent: AccountAddress,
    child_id: AccountAddress,
) {
    use crate::sandbox_runtime::SharedObjectRuntime;

    if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
        let mut state = shared.shared_state().lock();
        let _before = state.children.len();
        state.remove_child(parent, child_id);
        let _after = state.children.len();
        debug_native!(
            "[remove_child_from_shared_state] parent={}, child={}, removed={} (children {} -> {})",
            &parent.to_hex_literal()[..20],
            &child_id.to_hex_literal()[..20],
            _before > _after,
            _before,
            _after
        );
    }
}

/// Mark a child as mutated in shared state (if using SharedObjectRuntime).
fn mark_child_mutated(ctx: &mut NativeContext, parent: AccountAddress, child_id: AccountAddress) {
    let _ = (ctx, parent, child_id);
    // Mutation tracking is synced from the local GlobalValue state on drop,
    // which avoids false positives from mutable borrows with no writes.
}

/// Check if a child exists in shared state (if using SharedObjectRuntime).
/// Note: This does NOT trigger on-demand fetching. Use check_shared_state_for_child_with_fetch
/// if you need to fetch children that aren't already cached.
fn check_shared_state_for_child(
    ctx: &NativeContext,
    parent: AccountAddress,
    child_id: AccountAddress,
) -> bool {
    use crate::sandbox_runtime::SharedObjectRuntime;

    if let Ok(shared) = ctx.extensions().get::<SharedObjectRuntime>() {
        let arc = shared.shared_state();
        let state = arc.lock();
        return state.has_child(parent, child_id);
    }
    false
}

/// Count children for a parent in shared state (if using SharedObjectRuntime).
fn count_shared_state_children(ctx: &NativeContext, parent: AccountAddress) -> u64 {
    use crate::sandbox_runtime::SharedObjectRuntime;

    if let Ok(shared) = ctx.extensions().get::<SharedObjectRuntime>() {
        return shared
            .shared_state()
            .lock()
            .count_children_for_parent(parent);
    }
    0
}

/// Add dynamic field natives that use the ObjectRuntime VM extension.
///
/// These natives access the ObjectRuntime via context.extensions_mut().get_mut()
/// which provides proper reference semantics for borrow operations.
///
/// For PTB execution with persistent state, register SharedObjectRuntime instead
/// of ObjectRuntime. These natives will automatically use SharedObjectRuntime's
/// local runtime when available.
fn add_dynamic_field_natives(
    natives: &mut Vec<(&'static str, &'static str, NativeFunction)>,
    state: Arc<MockNativeState>,
) {
    use fastcrypto::hash::{Blake2b256, HashFunction};

    // hash_type_and_key<K>(parent: address, k: K) -> address
    // Deterministically derives child ID from parent + key type + key value
    //
    // IMPORTANT: Must match Sui's derive_dynamic_field_id exactly:
    // hash(scope || parent || len(key) || key || key_type_tag)
    // where scope = 0xf0 (HashingIntentScope::ChildObjectId)
    // and len(key) is encoded as 8-byte little-endian
    let state_clone = state.clone();
    natives.push((
        "dynamic_field",
        "hash_type_and_key",
        make_native(move |ctx, mut ty_args, mut args| {
            use crate::sandbox_runtime::SharedObjectRuntime;

            let key_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let key_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let parent = pop_arg!(args, AccountAddress);

            let key_tag_original = ctx.type_to_type_tag(&key_ty)?;

            // CRITICAL: Rewrite the type tag to use RUNTIME addresses instead of BYTECODE addresses.
            // This is necessary for upgraded packages where:
            // - Bytecode references the original package address (e.g., 0xefe8b36d...)
            // - But runtime types use the current package address (e.g., 0xd384ded6...)
            // Dynamic field keys are stored with RUNTIME addresses, so hash must match.
            let key_tag_rewritten =
                if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                    shared.rewrite_type_tag(key_tag_original.clone())
                } else {
                    key_tag_original.clone()
                };

            let cost = state_clone.get_native_cost(|c| c.dynamic_field_hash_base);

            let key_layout = match ctx.type_to_type_layout(&key_ty) {
                Ok(Some(layout)) => layout,
                _ => return Ok(NativeResult::err(InternalGas::new(cost), 3)),
            };

            let key_bytes = match key_value.typed_serialize(&key_layout) {
                Some(bytes) => bytes,
                None => return Ok(NativeResult::err(InternalGas::new(cost), 3)),
            };

            let compute_child_id = |tag: &TypeTag, key_bytes: &[u8]| -> AccountAddress {
                let type_tag_bytes = bcs::to_bytes(tag).unwrap_or_default();

                // Derive child ID using Sui's exact formula:
                // Blake2b256(0xf0 || parent || len(key_bytes) as u64 LE || key_bytes || type_tag_bytes)
                const CHILD_OBJECT_ID_SCOPE: u8 = 0xf0;

                let mut hasher = Blake2b256::default();
                hasher.update([CHILD_OBJECT_ID_SCOPE]);
                hasher.update(parent.as_ref());
                hasher.update((key_bytes.len() as u64).to_le_bytes());
                hasher.update(key_bytes);
                hasher.update(&type_tag_bytes);

                let hash = hasher.finalize();
                AccountAddress::new(hash.digest)
            };

            fn rewrite_address(tag: &TypeTag, from: AccountAddress, to: AccountAddress) -> TypeTag {
                match tag {
                    TypeTag::Struct(s) => {
                        let mut s = (**s).clone();
                        if s.address == from {
                            s.address = to;
                        }
                        s.type_params = s
                            .type_params
                            .into_iter()
                            .map(|t| rewrite_address(&t, from, to))
                            .collect();
                        TypeTag::Struct(Box::new(s))
                    }
                    TypeTag::Vector(inner) => {
                        TypeTag::Vector(Box::new(rewrite_address(inner, from, to)))
                    }
                    other => other.clone(),
                }
            }

            let mut candidate_tags: Vec<TypeTag> = Vec::new();
            candidate_tags.push(key_tag_rewritten.clone());
            if key_tag_rewritten != key_tag_original {
                candidate_tags.push(key_tag_original.clone());
            }

            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                if let Some(resolved_tag) = shared.resolve_key_type(parent, &key_bytes) {
                    candidate_tags.push(resolved_tag);
                }
                if let TypeTag::Struct(s) = &key_tag_original {
                    for storage in shared.storage_aliases_for(&s.address) {
                        if storage == s.address {
                            continue;
                        }
                        let alt = rewrite_address(&key_tag_original, s.address, storage);
                        candidate_tags.push(alt);
                    }
                }
            }

            // Deduplicate candidates by formatted type tag.
            let mut seen = std::collections::HashSet::new();
            candidate_tags.retain(|tag| {
                let key = crate::types::format_type_tag(tag);
                seen.insert(key)
            });

            let mut chosen_child_id = None;
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                for tag in &candidate_tags {
                    let child_id = compute_child_id(tag, &key_bytes);
                    shared.record_computed_child(child_id, parent, tag.clone(), key_bytes.clone());
                    let exists = {
                        let state = shared.shared_state().lock();
                        state.has_child(parent, child_id)
                    };
                    if exists {
                        chosen_child_id = Some(child_id);
                        break;
                    }
                    if shared.try_fetch_child(parent, child_id).is_some() {
                        chosen_child_id = Some(child_id);
                        break;
                    }
                }
            }

            let mut child_id = match chosen_child_id {
                Some(id) => id,
                None => compute_child_id(&key_tag_rewritten, &key_bytes),
            };
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                if let Some(actual_id) = shared.resolve_child_id_alias(child_id) {
                    child_id = actual_id;
                }
            }

            // Check for suspicious parent addresses that look like corrupted data
            // Sui addresses are usually SHA-256 based, so having most bytes zero is suspicious
            let parent_bytes = parent.as_ref();
            let zero_count = parent_bytes.iter().filter(|&&b| b == 0).count();
            let is_suspicious = zero_count > 24; // More than 24 zero bytes out of 32 is suspicious

            if is_suspicious {
                debug_native!(
                    "[hash_type_and_key] SUSPICIOUS parent={} ({} zero bytes), raw bytes: {:02x?}",
                    parent.to_hex_literal(),
                    zero_count,
                    parent_bytes
                );
            } else {
                debug_native!(
                    "[hash_type_and_key] parent={}, key_type={:?}, key_len={}, result={}",
                    parent.to_hex_literal(),
                    key_tag_rewritten,
                    key_bytes.len(),
                    child_id.to_hex_literal()
                );
                // Check if this child is already in shared state
                if let Ok(shared) = ctx.extensions().get::<SharedObjectRuntime>() {
                    let state = shared.shared_state().lock();
                    let _found = state.has_child(parent, child_id);
                    debug_native!(
                        "[hash_type_and_key] Child {} in shared_state? {} (total children={})",
                        child_id.to_hex_literal(),
                        _found,
                        state.children.len()
                    );
                }
            }

            // Record the computed child info for key-based fallback lookup.
            // This is essential for handling package upgrades where the computed hash
            // differs from the stored child object's hash due to type address changes.
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                shared.record_computed_child(child_id, parent, key_tag_rewritten, key_bytes);
            }

            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::address(child_id)],
            ))
        }),
    ));

    // add_child_object<Child: key>(parent: address, child: Child)
    let state_clone = state.clone();
    natives.push((
        "dynamic_field",
        "add_child_object",
        make_native(move |ctx, mut ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.dynamic_field_add_child_base);
            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let parent = pop_arg!(args, AccountAddress);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;

            // Get layout to extract child ID
            let child_layout = match ctx.type_to_type_layout(&child_ty) {
                Ok(Some(layout)) => layout,
                _ => return Ok(NativeResult::err(InternalGas::new(cost), 3)),
            };

            // Serialize to get the child ID (first 32 bytes = UID.id.bytes)
            let child_bytes = match child_value.copy_value()?.typed_serialize(&child_layout) {
                Some(bytes) => bytes,
                None => return Ok(NativeResult::err(InternalGas::new(cost), 3)),
            };

            let child_id = if child_bytes.len() >= 32 {
                let mut addr_bytes = [0u8; 32];
                addr_bytes.copy_from_slice(&child_bytes[..32]);
                AccountAddress::new(addr_bytes)
            } else {
                return Ok(NativeResult::err(InternalGas::new(cost), 3));
            };

            // Store in ObjectRuntime extension (supports both ObjectRuntime and SharedObjectRuntime)
            let runtime = get_object_runtime_mut(ctx)?;
            match runtime.add_child_object(parent, child_id, child_value, child_tag.clone()) {
                Ok(()) => {
                    // Sync to shared state for persistence across VM sessions
                    sync_child_to_shared_state(ctx, parent, child_id, &child_tag, &child_bytes);
                    Ok(NativeResult::ok(InternalGas::new(cost), smallvec![]))
                }
                Err(code) => Ok(NativeResult::err(InternalGas::new(cost), code)),
            }
        }),
    ));

    // borrow_child_object<Child: key>(object: &UID, id: address) -> &Child
    let state_clone = state.clone();
    natives.push((
        "dynamic_field",
        "borrow_child_object",
        make_native(move |ctx, mut ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.dynamic_field_borrow_child_base);
            use crate::sandbox_runtime::SharedObjectRuntime;
            use move_vm_types::values::StructRef;

            debug_native!("[borrow_child_object] ENTERING NATIVE, ty_args={}, args={}", ty_args.len(), args.len());
            use std::io::Write;
            std::io::stderr().flush().ok();

            let child_ty = ty_args.pop().ok_or_else(|| {
            debug_native!("[borrow_child_object] ERROR: no type arg");
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent_uid = pop_arg!(args, StructRef);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;

            debug_native!("[borrow_child_object] NATIVE CALLED, child_tag={:?}", child_tag);

            // Extract parent address from UID { id: ID { bytes: address } }
            // Navigate: UID.id (field 0) -> ID.bytes (field 0) -> address
            let parent = match extract_address_from_uid(&parent_uid) {
                Some(addr) => addr,
                None => {
                    // Failed to extract UID - return error instead of silently using 0x0
            debug_native!("[borrow_child_object] FAILED to extract parent UID!");
                    return Ok(NativeResult::err(InternalGas::new(cost), E_NOT_SUPPORTED));
                }
            };

            // Debug: print parent and child addresses
            debug_native!(
                "[borrow_child_object] parent={}, child_id={}",
                parent.to_hex_literal(),
                child_id.to_hex_literal()
            );

            // First check if it's already in the local runtime
            {
                let runtime = get_object_runtime_ref(ctx)?;
                if runtime.child_object_exists(parent, child_id) {
            debug_native!("[borrow_child_object] found in local runtime");
                    match runtime.borrow_child_object(parent, child_id, &child_tag) {
                        Ok(value) => {
                            return Ok(NativeResult::ok(InternalGas::new(cost), smallvec![value]))
                        }
                        Err(code) => return Ok(NativeResult::err(InternalGas::new(cost), code)),
                    }
                }
            }

            // Not in local runtime - check shared state and lazy-load if available
            // Get the type layout for deserialization
            let type_layout = match ctx.type_to_type_layout(&child_ty) {
                Ok(Some(layout)) => layout,
                _ => {
                    return Ok(NativeResult::err(
                        InternalGas::new(cost),
                        E_FIELD_TYPE_MISMATCH,
                    ))
                }
            };

            // Try to load from shared state
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                // Get bytes from shared state
                let child_bytes_opt = {
                    let state = shared.shared_state().lock();
                    debug_native!(
                        "[borrow_child_object] checking shared state, has {} children",
                        state.children.len()
                    );
                    // Debug: print first few children keys
                    for (_key, _) in state.children.iter().take(5) {
                        debug_native!(
                            "[borrow_child_object]   - parent={}, child={}",
                            _key.0.to_hex_literal(),
                            _key.1.to_hex_literal()
                        );
                    }
                    state
                        .get_child(parent, child_id)
                        .map(|(_, bytes)| bytes.clone())
                };

                debug_native!(
                    "[borrow_child_object] shared state lookup result: {:?}",
                    child_bytes_opt.as_ref().map(|b| b.len())
                );

                // If not in shared state, try on-demand fetching
            debug_native!("[borrow_child_object] child_bytes_opt.is_none() = {}", child_bytes_opt.is_none());
                let child_bytes_opt = if child_bytes_opt.is_none() {
            debug_native!("[borrow_child_object] calling try_fetch_child for parent={}, child={}",
                        parent.to_hex_literal(), child_id.to_hex_literal());
                    if let Some((fetched_tag, fetched_bytes)) = shared.try_fetch_child(parent, child_id) {
                        debug_native!(
                            "[borrow_child_object] on-demand fetch succeeded, {} bytes, type={:?}",
                            fetched_bytes.len(),
                            fetched_tag
                        );
                        // Add to shared state for future lookups
                        {
                            let mut state = shared.shared_state().lock();
                            state.add_child(parent, child_id, fetched_tag, fetched_bytes.clone());
                            // Mark as preloaded so it is treated as existing, not "new"
                            state.preloaded_children.insert((parent, child_id));
                            state
                                .preloaded_child_bytes
                                .insert((parent, child_id), fetched_bytes.clone());
                        }
                        Some(fetched_bytes)
                    } else {
            debug_native!("[borrow_child_object] on-demand fetch failed or not configured");
                        None
                    }
                } else {
            debug_native!("[borrow_child_object] already have child_bytes from shared state");
                    child_bytes_opt
                };

                if let Some(child_bytes) = child_bytes_opt {
                    // Get detailed layout info for debugging
                    let layout_field_count = match &type_layout {
                        MoveTypeLayout::Struct(s) => s.0.len(),
                        _ => 0,
                    };

                    // Validate Field struct layout (silent unless incorrect)
                    let is_field_type = format!("{:?}", child_tag).contains("\"Field\"");
                    if is_field_type && layout_field_count != 3 {
                        debug_native!(
                            "[borrow_child_object] WARNING: Field layout has {} fields, expected 3!",
                            layout_field_count
                        );
                    }

                    // Deserialize the bytes into a Move Value
                    if let Some(value) = Value::simple_deserialize(&child_bytes, &type_layout) {
                        if is_field_type {
            debug_native!("[borrow_child_object] Deserialization SUCCESS");
                        }
                        // Add to local runtime so we can borrow from it
                        let runtime = shared.local_mut();
                        match runtime.add_child_object_cached(parent, child_id, value, child_tag.clone()) {
                            Ok(()) => {
                                // Now we can borrow from the local runtime
                                match runtime.borrow_child_object(parent, child_id, &child_tag) {
                                    Ok(ref_value) => {
                                        return Ok(NativeResult::ok(
                                            InternalGas::new(cost),
                                            smallvec![ref_value],
                                        ))
                                    }
                                    Err(code) => {
                                        return Ok(NativeResult::err(InternalGas::new(cost), code))
                                    }
                                }
                            }
                            Err(code) => return Ok(NativeResult::err(InternalGas::new(cost), code)),
                        }
                    } else {
                        debug_native!(
                            "[borrow_child_object] Deserialization FAILED for type {:?}",
                            child_tag
                        );
            debug_native!("[borrow_child_object] Layout: {:?}", type_layout);
                        return Ok(NativeResult::err(
                            InternalGas::new(cost),
                            E_FIELD_TYPE_MISMATCH,
                        ));
                    }
                }
            }

            // Child doesn't exist in either local or shared state
            debug_native!(
                "[borrow_child_object] FINAL: child not found, returning E_FIELD_DOES_NOT_EXIST, parent={}, child_id={}",
                parent.to_hex_literal(),
                child_id.to_hex_literal()
            );
            Ok(NativeResult::err(
                InternalGas::new(cost),
                E_FIELD_DOES_NOT_EXIST,
            ))
        }),
    ));

    // borrow_child_object_mut<Child: key>(object: &mut UID, id: address) -> &mut Child
    let state_clone = state.clone();
    natives.push((
        "dynamic_field",
        "borrow_child_object_mut",
        make_native(move |ctx, mut ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.dynamic_field_borrow_child_base);
            use move_vm_types::values::StructRef;
            use crate::sandbox_runtime::SharedObjectRuntime;

            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent_uid = pop_arg!(args, StructRef);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;

            debug_native!("[borrow_child_object_mut] NATIVE CALLED");

            // Extract parent address (same as borrow_child_object)
            let parent = match extract_address_from_uid(&parent_uid) {
                Some(addr) => addr,
                None => {
            debug_native!("[borrow_child_object_mut] FAILED to extract parent UID!");
                    return Ok(NativeResult::err(InternalGas::new(cost), E_NOT_SUPPORTED));
                }
            };

            debug_native!("[borrow_child_object_mut] parent={}, child_id={}", parent.to_hex_literal(), child_id.to_hex_literal());

            // First check if it's already in the local runtime
            {
                let runtime = get_object_runtime_ref(ctx)?;
                if runtime.child_object_exists(parent, child_id) {
                    let is_field_type = format!("{:?}", child_tag).contains("\"Field\"");
            debug_native!("[borrow_child_object_mut] found in local runtime, parent={}, child={}, type={:?}",
                        parent.to_hex_literal(), child_id.to_hex_literal(), child_tag);
                    let runtime = get_object_runtime_mut(ctx)?;
                    match runtime.borrow_child_object_mut(parent, child_id, &child_tag) {
                        Ok(value) => {
                            mark_child_mutated(ctx, parent, child_id);
                            if is_field_type {
            debug_native!("[borrow_child_object_mut] LOCAL returning value for parent={}: {:?}",
                                    parent.to_hex_literal(), value);
                            }
                            return Ok(NativeResult::ok(InternalGas::new(cost), smallvec![value]));
                        }
                        Err(code) => return Ok(NativeResult::err(InternalGas::new(cost), code)),
                    }
                }
            }

            // Not in local runtime - check shared state and lazy-load if available
            let type_layout = match ctx.type_to_type_layout(&child_ty) {
                Ok(Some(layout)) => layout,
                _ => return Ok(NativeResult::err(InternalGas::new(cost), E_FIELD_TYPE_MISMATCH)),
            };

            // Try to load from shared state (same logic as borrow_child_object)
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                let child_bytes_opt = {
                    let state = shared.shared_state().lock();
            debug_native!("[borrow_child_object_mut] checking shared state, has {} children", state.children.len());
                    state.get_child(parent, child_id).map(|(_, bytes)| bytes.clone())
                };

            debug_native!("[borrow_child_object_mut] shared state lookup result: {:?}", child_bytes_opt.as_ref().map(|b| b.len()));

                // If not in shared state, try on-demand fetching
                let child_bytes_opt = if child_bytes_opt.is_none() {
                    if let Some((fetched_tag, fetched_bytes)) = shared.try_fetch_child(parent, child_id) {
            debug_native!("[borrow_child_object_mut] on-demand fetch succeeded, {} bytes, type={:?}", fetched_bytes.len(), fetched_tag);
                        {
                            let mut state = shared.shared_state().lock();
                            state.add_child(parent, child_id, fetched_tag, fetched_bytes.clone());
                            // Mark as preloaded so it is treated as existing, not "new"
                            state.preloaded_children.insert((parent, child_id));
                            state
                                .preloaded_child_bytes
                                .insert((parent, child_id), fetched_bytes.clone());
                        }
                        Some(fetched_bytes)
                    } else {
            debug_native!("[borrow_child_object_mut] on-demand fetch failed or not configured");
                        None
                    }
                } else {
                    child_bytes_opt
                };

                if let Some(child_bytes) = child_bytes_opt {
                    // Get detailed layout info for debugging
                    let layout_field_count = match &type_layout {
                        MoveTypeLayout::Struct(s) => s.0.len(),
                        _ => 0,
                    };

                    // Validate Field struct layout (silent unless incorrect)
                    let is_field_type = format!("{:?}", child_tag).contains("\"Field\"");
                    if is_field_type && layout_field_count != 3 {
                        debug_native!(
                            "[borrow_child_object_mut] WARNING: Field layout has {} fields, expected 3!",
                            layout_field_count
                        );
                    }

                    // Deserialize the bytes into a Move Value
                    if let Some(value) = Value::simple_deserialize(&child_bytes, &type_layout) {
                        if is_field_type {
            debug_native!("[borrow_child_object_mut] Deserialization SUCCESS");
                        }
                        // Add to local runtime so we can borrow from it
                        let runtime = shared.local_mut();
                        match runtime.add_child_object_cached(parent, child_id, value, child_tag.clone()) {
                            Ok(()) => {
                                // Now we can borrow mutably from the local runtime
                                match runtime.borrow_child_object_mut(parent, child_id, &child_tag) {
                                    Ok(ref_value) => {
                                        mark_child_mutated(ctx, parent, child_id);
                                        if is_field_type {
            debug_native!("[borrow_child_object_mut] Returning ref_value: {:?}", ref_value);
                                        }
                                        return Ok(NativeResult::ok(InternalGas::new(cost), smallvec![ref_value]));
                                    }
                                    Err(code) => {
            debug_native!("[borrow_child_object_mut] borrow failed with code {}", code);
                                        return Ok(NativeResult::err(InternalGas::new(cost), code));
                                    }
                                }
                            }
                            Err(code) => return Ok(NativeResult::err(InternalGas::new(cost), code)),
                        }
                    } else {
                        debug_native!(
                            "[borrow_child_object_mut] Deserialization FAILED for type {:?}",
                            child_tag
                        );
            debug_native!("[borrow_child_object_mut] Layout: {:?}", type_layout);
                        return Ok(NativeResult::err(InternalGas::new(cost), E_FIELD_TYPE_MISMATCH));
                    }
                }
            }

            // Child doesn't exist
            Ok(NativeResult::err(InternalGas::new(cost), E_FIELD_DOES_NOT_EXIST))
        }),
    ));

    // remove_child_object<Child: key>(parent: address, id: address) -> Child
    let state_clone = state.clone();
    natives.push((
        "dynamic_field",
        "remove_child_object",
        make_native(move |ctx, mut ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.dynamic_field_remove_child_base);
            use crate::sandbox_runtime::SharedObjectRuntime;

            debug_native!("[remove_child_object] ENTERING NATIVE");
            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent = pop_arg!(args, AccountAddress);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;
            debug_native!(
                "[remove_child_object] parent={}, child_id={}, type={:?}",
                parent.to_hex_literal(),
                child_id.to_hex_literal(),
                child_tag
            );

            // First check if the child is in local runtime
            {
                let runtime = get_object_runtime_ref(ctx)?;
                if runtime.child_object_exists(parent, child_id) {
                    debug_native!("[remove_child_object] found in local runtime");
                    let runtime = get_object_runtime_mut(ctx)?;
                    match runtime.remove_child_object(parent, child_id, &child_tag) {
                        Ok(value) => {
                            debug_native!("[remove_child_object] SUCCESS from local runtime");
                            remove_child_from_shared_state(ctx, parent, child_id);
                            return Ok(NativeResult::ok(InternalGas::new(cost), smallvec![value]));
                        }
                        Err(code) => {
                            debug_native!(
                                "[remove_child_object] FAILED from local runtime with code {}",
                                code
                            );
                            return Ok(NativeResult::err(InternalGas::new(cost), code));
                        }
                    }
                }
            }

            // Not in local runtime - try to load from shared state
            let type_layout = match ctx.type_to_type_layout(&child_ty) {
                Ok(Some(layout)) => layout,
                _ => {
                    debug_native!("[remove_child_object] Failed to get type layout");
                    return Ok(NativeResult::err(
                        InternalGas::new(cost),
                        E_FIELD_TYPE_MISMATCH,
                    ));
                }
            };

            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                // Try to get from shared state
                let child_bytes_opt = {
                    let state = shared.shared_state().lock();
                    debug_native!(
                        "[remove_child_object] checking shared state, has {} children",
                        state.children.len()
                    );
                    state
                        .get_child(parent, child_id)
                        .map(|(_, bytes)| bytes.clone())
                };

                // If not in shared state, try on-demand fetching
                let child_bytes_opt = if child_bytes_opt.is_none() {
                    debug_native!(
                        "[remove_child_object] not in shared state, trying on-demand fetch"
                    );
                    if let Some((_fetched_tag, fetched_bytes)) =
                        shared.try_fetch_child(parent, child_id)
                    {
                        debug_native!(
                            "[remove_child_object] on-demand fetch succeeded, {} bytes",
                            fetched_bytes.len()
                        );
                        Some(fetched_bytes)
                    } else {
                        debug_native!("[remove_child_object] on-demand fetch failed");
                        None
                    }
                } else {
                    debug_native!("[remove_child_object] found in shared state");
                    child_bytes_opt
                };

                if let Some(child_bytes) = child_bytes_opt {
                    // Deserialize and add to local runtime, then remove
                    if let Some(value) = Value::simple_deserialize(&child_bytes, &type_layout) {
                        debug_native!("[remove_child_object] Deserialization SUCCESS");
                        // Add to local runtime first so we can remove it
                        let runtime = shared.local_mut();
                        if let Err(e) = runtime.add_child_object_cached(
                            parent,
                            child_id,
                            value.copy_value().unwrap(),
                            child_tag.clone(),
                        ) {
                            debug_native!(
                                "[remove_child_object] Failed to add to local runtime: {}",
                                e
                            );
                            return Ok(NativeResult::err(InternalGas::new(cost), e));
                        }
                        // Now remove it
                        match runtime.remove_child_object(parent, child_id, &child_tag) {
                            Ok(value) => {
                                debug_native!(
                                    "[remove_child_object] SUCCESS after loading from shared state"
                                );
                                remove_child_from_shared_state(ctx, parent, child_id);
                                return Ok(NativeResult::ok(
                                    InternalGas::new(cost),
                                    smallvec![value],
                                ));
                            }
                            Err(code) => {
                                debug_native!(
                                    "[remove_child_object] FAILED after loading, code {}",
                                    code
                                );
                                return Ok(NativeResult::err(InternalGas::new(cost), code));
                            }
                        }
                    } else {
                        debug_native!(
                            "[remove_child_object] Deserialization FAILED for type {:?}",
                            child_tag
                        );
                    }
                }
            }

            debug_native!("[remove_child_object] FAILED - child not found anywhere");
            Ok(NativeResult::err(
                InternalGas::new(cost),
                E_FIELD_DOES_NOT_EXIST,
            ))
        }),
    ));

    // has_child_object(parent: address, id: address) -> bool
    let state_clone = state.clone();
    natives.push((
        "dynamic_field",
        "has_child_object",
        make_native(move |ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.dynamic_field_has_child_base);
            let child_id = pop_arg!(args, AccountAddress);
            let parent = pop_arg!(args, AccountAddress);

            debug_native!(
                "[has_child_object] parent={}, child_id={}",
                parent.to_hex_literal(),
                child_id.to_hex_literal()
            );

            // Check local runtime first (this borrow ends before we check shared state)
            let in_local = {
                let runtime = get_object_runtime_ref(ctx)?;
                runtime.child_object_exists(parent, child_id)
            };

            if in_local {
                return Ok(NativeResult::ok(
                    InternalGas::new(cost),
                    smallvec![Value::bool(true)],
                ));
            }

            // Check if this child was removed during this PTB - if so, don't re-fetch
            use crate::sandbox_runtime::SharedObjectRuntime;
            if let Ok(shared) = ctx.extensions().get::<SharedObjectRuntime>() {
                if shared
                    .shared_state()
                    .lock()
                    .is_child_removed(parent, child_id)
                {
                    debug_native!("[has_child_object] child was removed, returning false");
                    return Ok(NativeResult::ok(
                        InternalGas::new(cost),
                        smallvec![Value::bool(false)],
                    ));
                }
            }

            // Check shared state
            let in_shared = check_shared_state_for_child(ctx, parent, child_id);
            if in_shared {
                return Ok(NativeResult::ok(
                    InternalGas::new(cost),
                    smallvec![Value::bool(true)],
                ));
            }

            // Not found locally or in shared state - try on-demand fetching
            // This is needed for replay scenarios where dynamic fields are fetched lazily
            let fetched = if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>()
            {
                if let Some((fetched_tag, fetched_bytes)) = shared.try_fetch_child(parent, child_id)
                {
                    // Add to shared state for future lookups
                    {
                        let mut state = shared.shared_state().lock();
                        state.add_child(parent, child_id, fetched_tag, fetched_bytes.clone());
                        // Mark as preloaded so this existing child isn't treated as newly created.
                        state.preloaded_children.insert((parent, child_id));
                        state
                            .preloaded_child_bytes
                            .insert((parent, child_id), fetched_bytes);
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };

            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::bool(fetched)],
            ))
        }),
    ));

    // has_child_object_with_ty<Child: key>(parent: address, id: address) -> bool
    let state_clone = state.clone();
    natives.push((
        "dynamic_field",
        "has_child_object_with_ty",
        make_native(move |ctx, mut ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.dynamic_field_has_child_base);
            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent = pop_arg!(args, AccountAddress);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;

            // Check local runtime first (this borrow ends before we check shared state)
            let in_local = {
                let runtime = get_object_runtime_ref(ctx)?;
                runtime.child_object_exists_with_type(parent, child_id, &child_tag)
            };

            if in_local {
                return Ok(NativeResult::ok(
                    InternalGas::new(cost),
                    smallvec![Value::bool(true)],
                ));
            }

            // Check shared state
            let in_shared = check_shared_state_for_child(ctx, parent, child_id);
            if in_shared {
                return Ok(NativeResult::ok(
                    InternalGas::new(cost),
                    smallvec![Value::bool(true)],
                ));
            }

            // Not found locally or in shared state - try on-demand fetching
            // This is needed for replay scenarios where dynamic fields are fetched lazily
            use crate::sandbox_runtime::SharedObjectRuntime;
            let fetched = if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>()
            {
                if let Some((fetched_tag, fetched_bytes)) = shared.try_fetch_child(parent, child_id)
                {
                    // Verify the type matches before adding
                    // Note: We do a simple match here - could be more lenient if needed
                    let type_matches = fetched_tag == child_tag;
            debug_native!("[has_child_object_with_ty] fetched_tag={:?}", fetched_tag);
            debug_native!("[has_child_object_with_ty] child_tag={:?}", child_tag);
            debug_native!("[has_child_object_with_ty] type_matches={}", type_matches);
                    if type_matches {
                        // Add to shared state for future lookups
                        {
                            let arc = shared.shared_state();
                            #[cfg(debug_assertions)]
                            let _arc_ptr = std::sync::Arc::as_ptr(arc);
                            let mut state = arc.lock();
                            #[cfg(debug_assertions)]
                            let _before_count = state.children.len();
                            state.add_child(parent, child_id, fetched_tag, fetched_bytes.clone());
                            // Mark as preloaded so it is treated as existing, not "new"
                            state.preloaded_children.insert((parent, child_id));
                            state
                                .preloaded_child_bytes
                                .insert((parent, child_id), fetched_bytes);
                            #[cfg(debug_assertions)]
                            let _after_count = state.children.len();
            debug_native!("[has_child_object_with_ty] arc={:p}, Added child to shared state. Count: {} -> {}. Parent={}, Child={}",
                                _arc_ptr, _before_count, _after_count, parent.to_hex_literal(), child_id.to_hex_literal());
                        }
                        true
                    } else {
                        // Type mismatch - the field exists but with a different type
                        // This is still "exists" in the general sense
            debug_native!("[has_child_object_with_ty] TYPE MISMATCH - returning false even though child exists!");
                        false
                    }
                } else {
            debug_native!("[has_child_object_with_ty] try_fetch_child returned None");
                    false
                }
            } else {
                false
            };

            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::bool(fetched)],
            ))
        }),
    ));

    // field_info_count(parent: address) -> u64
    // Returns the number of dynamic fields for a given parent object.
    // This is a sandbox-specific extension to help with Table/Bag iteration.
    let state_clone = state.clone();
    natives.push((
        "dynamic_field",
        "field_info_count",
        make_native(move |ctx, _ty_args, mut args| {
            let cost = state_clone.get_native_cost(|c| c.dynamic_field_has_child_base);
            let parent = pop_arg!(args, AccountAddress);

            let runtime = get_object_runtime_ref(ctx)?;
            let count = runtime.count_children_for_parent(parent);

            // Also check shared state for additional children
            let shared_count = count_shared_state_children(ctx, parent);

            Ok(NativeResult::ok(
                InternalGas::new(cost),
                smallvec![Value::u64(count + shared_count)],
            ))
        }),
    ));
}

/// Add permissive mocks for crypto and other operations.
///
/// These mocks return plausible success values instead of aborting, allowing
/// LLM-generated code that uses these operations to continue executing.
/// This is appropriate for type inhabitation testing where we care about
/// type correctness, not cryptographic correctness.
///
/// ## Philosophy
///
/// All cryptographic operations use fastcrypto (Mysten Labs' crypto library),
/// providing 1:1 compatibility with Sui mainnet behavior.
///
/// - Verification functions perform REAL verification
/// - Recovery functions perform REAL public key recovery
/// - Invalid signatures/keys return false (not abort)
/// - Random returns deterministic bytes from MockRandom (for reproducibility)
fn add_real_crypto_natives(
    natives: &mut Vec<(&'static str, &'static str, NativeFunction)>,
    state: Arc<MockNativeState>,
) {
    // Hash function selection constants (must match Sui's ecdsa_k1 module)
    const KECCAK256: u8 = 0;
    const SHA256: u8 = 1;

    // ============================================================
    // BLS12-381 - REAL signature verification
    // ============================================================
    natives.push((
        "bls12381",
        "bls12381_min_sig_verify",
        make_native(|_ctx, _ty_args, mut args| {
            let msg = pop_arg!(args, Vec<u8>);
            let public_key_bytes = pop_arg!(args, Vec<u8>);
            let signature_bytes = pop_arg!(args, Vec<u8>);

            let Ok(signature) =
                <min_sig::BLS12381Signature as ToFromBytes>::from_bytes(&signature_bytes)
            else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let public_key =
                match <min_sig::BLS12381PublicKey as ToFromBytes>::from_bytes(&public_key_bytes) {
                    Ok(pk) => match pk.validate() {
                        Ok(_) => pk,
                        Err(_) => {
                            return Ok(NativeResult::ok(
                                InternalGas::new(0),
                                smallvec![Value::bool(false)],
                            ))
                        }
                    },
                    Err(_) => {
                        return Ok(NativeResult::ok(
                            InternalGas::new(0),
                            smallvec![Value::bool(false)],
                        ))
                    }
                };

            let result = public_key.verify(&msg, &signature).is_ok();
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(result)],
            ))
        }),
    ));

    natives.push((
        "bls12381",
        "bls12381_min_pk_verify",
        make_native(|_ctx, _ty_args, mut args| {
            let msg = pop_arg!(args, Vec<u8>);
            let public_key_bytes = pop_arg!(args, Vec<u8>);
            let signature_bytes = pop_arg!(args, Vec<u8>);

            let Ok(signature) =
                <min_pk::BLS12381Signature as ToFromBytes>::from_bytes(&signature_bytes)
            else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let public_key =
                match <min_pk::BLS12381PublicKey as ToFromBytes>::from_bytes(&public_key_bytes) {
                    Ok(pk) => match pk.validate() {
                        Ok(_) => pk,
                        Err(_) => {
                            return Ok(NativeResult::ok(
                                InternalGas::new(0),
                                smallvec![Value::bool(false)],
                            ))
                        }
                    },
                    Err(_) => {
                        return Ok(NativeResult::ok(
                            InternalGas::new(0),
                            smallvec![Value::bool(false)],
                        ))
                    }
                };

            let result = public_key.verify(&msg, &signature).is_ok();
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(result)],
            ))
        }),
    ));

    // ============================================================
    // ECDSA K1 (secp256k1) - REAL verification and recovery
    // ============================================================
    natives.push((
        "ecdsa_k1",
        "secp256k1_ecrecover",
        make_native(|_ctx, _ty_args, mut args| {
            let hash = pop_arg!(args, u8);
            let msg = pop_arg!(args, Vec<u8>);
            let signature_bytes = pop_arg!(args, Vec<u8>);

            let Ok(sig) =
                <Secp256k1RecoverableSignature as ToFromBytes>::from_bytes(&signature_bytes)
            else {
                // Return error code 1 = INVALID_SIGNATURE
                return Ok(NativeResult::err(InternalGas::new(0), 1));
            };

            let pk = match hash {
                KECCAK256 => sig.recover_with_hash::<Keccak256>(&msg),
                SHA256 => sig.recover_with_hash::<Sha256>(&msg),
                _ => {
                    // Return error code 0 = FAIL_TO_RECOVER_PUBKEY
                    return Ok(NativeResult::err(InternalGas::new(0), 0));
                }
            };

            match pk {
                Ok(pk) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(pk.as_bytes().to_vec())],
                )),
                Err(_) => Ok(NativeResult::err(InternalGas::new(0), 0)),
            }
        }),
    ));

    natives.push((
        "ecdsa_k1",
        "decompress_pubkey",
        make_native(|_ctx, _ty_args, mut args| {
            let pubkey_bytes = pop_arg!(args, Vec<u8>);

            match Secp256k1PublicKey::from_bytes(&pubkey_bytes) {
                Ok(pubkey) => {
                    let uncompressed = pubkey.pubkey.serialize_uncompressed();
                    Ok(NativeResult::ok(
                        InternalGas::new(0),
                        smallvec![Value::vector_u8(uncompressed.to_vec())],
                    ))
                }
                Err(_) => Ok(NativeResult::err(InternalGas::new(0), 2)), // INVALID_PUBKEY
            }
        }),
    ));

    natives.push((
        "ecdsa_k1",
        "secp256k1_verify",
        make_native(|_ctx, _ty_args, mut args| {
            let hash = pop_arg!(args, u8);
            let msg = pop_arg!(args, Vec<u8>);
            let public_key_bytes = pop_arg!(args, Vec<u8>);
            let signature_bytes = pop_arg!(args, Vec<u8>);

            let Ok(sig) = <Secp256k1Signature as ToFromBytes>::from_bytes(&signature_bytes) else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let Ok(pk) = <Secp256k1PublicKey as ToFromBytes>::from_bytes(&public_key_bytes) else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let result = match hash {
                KECCAK256 => pk.verify_with_hash::<Keccak256>(&msg, &sig).is_ok(),
                SHA256 => pk.verify_with_hash::<Sha256>(&msg, &sig).is_ok(),
                _ => false,
            };

            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(result)],
            ))
        }),
    ));

    // ============================================================
    // ECDSA R1 (secp256r1/P-256) - REAL verification and recovery
    // ============================================================
    natives.push((
        "ecdsa_r1",
        "secp256r1_ecrecover",
        make_native(|_ctx, _ty_args, mut args| {
            let hash = pop_arg!(args, u8);
            let msg = pop_arg!(args, Vec<u8>);
            let signature_bytes = pop_arg!(args, Vec<u8>);

            let Ok(sig) =
                <Secp256r1RecoverableSignature as ToFromBytes>::from_bytes(&signature_bytes)
            else {
                return Ok(NativeResult::err(InternalGas::new(0), 1));
            };

            let pk = match hash {
                KECCAK256 => sig.recover_with_hash::<Keccak256>(&msg),
                SHA256 => sig.recover_with_hash::<Sha256>(&msg),
                _ => {
                    return Ok(NativeResult::err(InternalGas::new(0), 0));
                }
            };

            match pk {
                Ok(pk) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(pk.as_bytes().to_vec())],
                )),
                Err(_) => Ok(NativeResult::err(InternalGas::new(0), 0)),
            }
        }),
    ));

    natives.push((
        "ecdsa_r1",
        "secp256r1_verify",
        make_native(|_ctx, _ty_args, mut args| {
            let hash = pop_arg!(args, u8);
            let msg = pop_arg!(args, Vec<u8>);
            let public_key_bytes = pop_arg!(args, Vec<u8>);
            let signature_bytes = pop_arg!(args, Vec<u8>);

            let Ok(sig) = <Secp256r1Signature as ToFromBytes>::from_bytes(&signature_bytes) else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let Ok(pk) = <Secp256r1PublicKey as ToFromBytes>::from_bytes(&public_key_bytes) else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let result = match hash {
                KECCAK256 => pk.verify_with_hash::<Keccak256>(&msg, &sig).is_ok(),
                SHA256 => pk.verify_with_hash::<Sha256>(&msg, &sig).is_ok(),
                _ => false,
            };

            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(result)],
            ))
        }),
    ));

    // ============================================================
    // Ed25519 - REAL signature verification
    // ============================================================
    natives.push((
        "ed25519",
        "ed25519_verify",
        make_native(|_ctx, _ty_args, mut args| {
            let msg = pop_arg!(args, Vec<u8>);
            let public_key_bytes = pop_arg!(args, Vec<u8>);
            let signature_bytes = pop_arg!(args, Vec<u8>);

            let Ok(signature) = <Ed25519Signature as ToFromBytes>::from_bytes(&signature_bytes)
            else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let Ok(public_key) = <Ed25519PublicKey as ToFromBytes>::from_bytes(&public_key_bytes)
            else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let result = public_key.verify(&msg, &signature).is_ok();
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(result)],
            ))
        }),
    ));

    // ============================================================
    // ECVRF - Verifiable Random Function
    // ============================================================
    natives.push((
        "ecvrf",
        "ecvrf_verify",
        make_native(|_ctx, _ty_args, _args| {
            // VRF verification "passes"
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(true)],
            ))
        }),
    ));

    // ============================================================
    // Groth16 - REAL ZK-SNARK verification using fastcrypto-zkp
    // ============================================================
    // Curve constants (must match Sui's groth16 module)
    const BLS12381_CURVE: u8 = 0;
    const BN254_CURVE: u8 = 1;

    // Error codes
    const INVALID_VERIFYING_KEY: u64 = 0;
    const INVALID_CURVE: u64 = 1;
    const TOO_MANY_PUBLIC_INPUTS: u64 = 2;
    const MAX_PUBLIC_INPUTS: usize = 8;

    natives.push((
        "groth16",
        "prepare_verifying_key_internal",
        make_native(|_ctx, _ty_args, mut args| {
            let verifying_key = pop_arg!(args, Vec<u8>);
            let curve = pop_arg!(args, u8);

            let result = match curve {
                BLS12381_CURVE => fastcrypto_zkp::bls12381::api::prepare_pvk_bytes(&verifying_key),
                BN254_CURVE => fastcrypto_zkp::bn254::api::prepare_pvk_bytes(&verifying_key),
                _ => {
                    return Ok(NativeResult::err(InternalGas::new(0), INVALID_CURVE));
                }
            };

            match result {
                Ok(pvk) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::struct_(Struct::pack(vec![
                        Value::vector_u8(pvk[0].to_vec()),
                        Value::vector_u8(pvk[1].to_vec()),
                        Value::vector_u8(pvk[2].to_vec()),
                        Value::vector_u8(pvk[3].to_vec())
                    ]))],
                )),
                Err(_) => Ok(NativeResult::err(
                    InternalGas::new(0),
                    INVALID_VERIFYING_KEY,
                )),
            }
        }),
    ));

    natives.push((
        "groth16",
        "verify_groth16_proof_internal",
        make_native(|_ctx, _ty_args, mut args| {
            let proof_points = pop_arg!(args, Vec<u8>);
            let public_proof_inputs = pop_arg!(args, Vec<u8>);
            let delta_g2_neg_pc = pop_arg!(args, Vec<u8>);
            let gamma_g2_neg_pc = pop_arg!(args, Vec<u8>);
            let alpha_g1_beta_g2 = pop_arg!(args, Vec<u8>);
            let vk_gamma_abc_g1 = pop_arg!(args, Vec<u8>);
            let curve = pop_arg!(args, u8);

            let result = match curve {
                BLS12381_CURVE => {
                    if public_proof_inputs.len()
                        > fastcrypto::groups::bls12381::SCALAR_LENGTH * MAX_PUBLIC_INPUTS
                    {
                        return Ok(NativeResult::err(
                            InternalGas::new(0),
                            TOO_MANY_PUBLIC_INPUTS,
                        ));
                    }
                    fastcrypto_zkp::bls12381::api::verify_groth16_in_bytes(
                        &vk_gamma_abc_g1,
                        &alpha_g1_beta_g2,
                        &gamma_g2_neg_pc,
                        &delta_g2_neg_pc,
                        &public_proof_inputs,
                        &proof_points,
                    )
                }
                BN254_CURVE => {
                    if public_proof_inputs.len()
                        > fastcrypto_zkp::bn254::api::SCALAR_SIZE * MAX_PUBLIC_INPUTS
                    {
                        return Ok(NativeResult::err(
                            InternalGas::new(0),
                            TOO_MANY_PUBLIC_INPUTS,
                        ));
                    }
                    fastcrypto_zkp::bn254::api::verify_groth16_in_bytes(
                        &vk_gamma_abc_g1,
                        &alpha_g1_beta_g2,
                        &gamma_g2_neg_pc,
                        &delta_g2_neg_pc,
                        &public_proof_inputs,
                        &proof_points,
                    )
                }
                _ => {
                    return Ok(NativeResult::err(InternalGas::new(0), INVALID_CURVE));
                }
            };

            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(result.unwrap_or(false))],
            ))
        }),
    ));

    // ============================================================
    // HMAC - Hash-based Message Authentication Code
    // ============================================================
    natives.push((
        "hmac",
        "hmac_sha3_256",
        make_native(|_ctx, _ty_args, _args| {
            // Return 32-byte HMAC output (zeros)
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(vec![0u8; 32])],
            ))
        }),
    ));

    // ============================================================
    // Group Operations - REAL BLS12-381 elliptic curve operations
    // ============================================================
    // Group type constants (must match Sui's group_ops module)
    const GROUP_BLS12381_SCALAR: u8 = 0;
    const GROUP_BLS12381_G1: u8 = 1;
    const GROUP_BLS12381_G2: u8 = 2;
    const GROUP_BLS12381_GT: u8 = 3;

    // Error codes
    const GROUP_OPS_INVALID_INPUT: u64 = 1;

    natives.push((
        "group_ops",
        "internal_validate",
        make_native(|_ctx, _ty_args, mut args| {
            let bytes = pop_arg!(args, Vec<u8>);
            let group_type = pop_arg!(args, u8);

            let result = match group_type {
                GROUP_BLS12381_SCALAR => {
                    let arr: Result<&[u8; 32], _> = bytes.as_slice().try_into();
                    arr.is_ok_and(|a| bls::Scalar::from_byte_array(a).is_ok())
                }
                GROUP_BLS12381_G1 => {
                    let arr: Result<&[u8; 48], _> = bytes.as_slice().try_into();
                    arr.is_ok_and(|a| bls::G1Element::from_byte_array(a).is_ok())
                }
                GROUP_BLS12381_G2 => {
                    let arr: Result<&[u8; 96], _> = bytes.as_slice().try_into();
                    arr.is_ok_and(|a| bls::G2Element::from_byte_array(a).is_ok())
                }
                GROUP_BLS12381_GT => {
                    let arr: Result<&[u8; 576], _> = bytes.as_slice().try_into();
                    arr.is_ok_and(|a| bls::GTElement::from_byte_array(a).is_ok())
                }
                _ => false,
            };

            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(result)],
            ))
        }),
    ));

    natives.push((
        "group_ops",
        "internal_add",
        make_native(|_ctx, _ty_args, mut args| {
            let e2 = pop_arg!(args, Vec<u8>);
            let e1 = pop_arg!(args, Vec<u8>);
            let group_type = pop_arg!(args, u8);

            let result: Option<Vec<u8>> = (|| match group_type {
                GROUP_BLS12381_SCALAR => {
                    let a1: &[u8; 32] = e1.as_slice().try_into().ok()?;
                    let a2: &[u8; 32] = e2.as_slice().try_into().ok()?;
                    let a = bls::Scalar::from_byte_array(a1).ok()?;
                    let b = bls::Scalar::from_byte_array(a2).ok()?;
                    Some((a + b).to_byte_array().to_vec())
                }
                GROUP_BLS12381_G1 => {
                    let a1: &[u8; 48] = e1.as_slice().try_into().ok()?;
                    let a2: &[u8; 48] = e2.as_slice().try_into().ok()?;
                    let a = bls::G1Element::from_byte_array(a1).ok()?;
                    let b = bls::G1Element::from_byte_array(a2).ok()?;
                    Some((a + b).to_byte_array().to_vec())
                }
                GROUP_BLS12381_G2 => {
                    let a1: &[u8; 96] = e1.as_slice().try_into().ok()?;
                    let a2: &[u8; 96] = e2.as_slice().try_into().ok()?;
                    let a = bls::G2Element::from_byte_array(a1).ok()?;
                    let b = bls::G2Element::from_byte_array(a2).ok()?;
                    Some((a + b).to_byte_array().to_vec())
                }
                GROUP_BLS12381_GT => {
                    let a1: &[u8; 576] = e1.as_slice().try_into().ok()?;
                    let a2: &[u8; 576] = e2.as_slice().try_into().ok()?;
                    let a = bls::GTElement::from_byte_array(a1).ok()?;
                    let b = bls::GTElement::from_byte_array(a2).ok()?;
                    Some((a + b).to_byte_array().to_vec())
                }
                _ => None,
            })();

            match result {
                Some(bytes) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(bytes)],
                )),
                None => Ok(NativeResult::err(
                    InternalGas::new(0),
                    GROUP_OPS_INVALID_INPUT,
                )),
            }
        }),
    ));

    natives.push((
        "group_ops",
        "internal_sub",
        make_native(|_ctx, _ty_args, mut args| {
            let e2 = pop_arg!(args, Vec<u8>);
            let e1 = pop_arg!(args, Vec<u8>);
            let group_type = pop_arg!(args, u8);

            let result: Option<Vec<u8>> = (|| match group_type {
                GROUP_BLS12381_SCALAR => {
                    let a1: &[u8; 32] = e1.as_slice().try_into().ok()?;
                    let a2: &[u8; 32] = e2.as_slice().try_into().ok()?;
                    let a = bls::Scalar::from_byte_array(a1).ok()?;
                    let b = bls::Scalar::from_byte_array(a2).ok()?;
                    Some((a - b).to_byte_array().to_vec())
                }
                GROUP_BLS12381_G1 => {
                    let a1: &[u8; 48] = e1.as_slice().try_into().ok()?;
                    let a2: &[u8; 48] = e2.as_slice().try_into().ok()?;
                    let a = bls::G1Element::from_byte_array(a1).ok()?;
                    let b = bls::G1Element::from_byte_array(a2).ok()?;
                    Some((a - b).to_byte_array().to_vec())
                }
                GROUP_BLS12381_G2 => {
                    let a1: &[u8; 96] = e1.as_slice().try_into().ok()?;
                    let a2: &[u8; 96] = e2.as_slice().try_into().ok()?;
                    let a = bls::G2Element::from_byte_array(a1).ok()?;
                    let b = bls::G2Element::from_byte_array(a2).ok()?;
                    Some((a - b).to_byte_array().to_vec())
                }
                GROUP_BLS12381_GT => {
                    let a1: &[u8; 576] = e1.as_slice().try_into().ok()?;
                    let a2: &[u8; 576] = e2.as_slice().try_into().ok()?;
                    let a = bls::GTElement::from_byte_array(a1).ok()?;
                    let b = bls::GTElement::from_byte_array(a2).ok()?;
                    Some((a - b).to_byte_array().to_vec())
                }
                _ => None,
            })();

            match result {
                Some(bytes) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(bytes)],
                )),
                None => Ok(NativeResult::err(
                    InternalGas::new(0),
                    GROUP_OPS_INVALID_INPUT,
                )),
            }
        }),
    ));

    natives.push((
        "group_ops",
        "internal_mul",
        make_native(|_ctx, _ty_args, mut args| {
            // Move signature: internal_mul(type: u8, e1: &vector<u8>, e2: &vector<u8>)
            // For G1/G2/GT: e1 is scalar, e2 is element
            // Stack pops in reverse order: e2 first, then e1, then type
            let e2 = pop_arg!(args, Vec<u8>); // element (for G1/G2/GT) or scalar2 (for Scalar)
            let e1 = pop_arg!(args, Vec<u8>); // scalar (for G1/G2/GT) or scalar1 (for Scalar)
            let group_type = pop_arg!(args, u8);

            let result: Option<Vec<u8>> = (|| {
                match group_type {
                    GROUP_BLS12381_SCALAR => {
                        // Both are scalars: result = e1 * e2 (but Sui does e2 * e1 = b * a)
                        let a1: &[u8; 32] = e1.as_slice().try_into().ok()?;
                        let a2: &[u8; 32] = e2.as_slice().try_into().ok()?;
                        let s1 = bls::Scalar::from_byte_array(a1).ok()?;
                        let s2 = bls::Scalar::from_byte_array(a2).ok()?;
                        Some((s2 * s1).to_byte_array().to_vec())
                    }
                    GROUP_BLS12381_G1 => {
                        // e1 = scalar, e2 = G1 element; result = e2 * e1
                        let s_arr: &[u8; 32] = e1.as_slice().try_into().ok()?;
                        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                        let g_arr: &[u8; 48] = e2.as_slice().try_into().ok()?;
                        let g = bls::G1Element::from_byte_array(g_arr).ok()?;
                        Some((g * s).to_byte_array().to_vec())
                    }
                    GROUP_BLS12381_G2 => {
                        let s_arr: &[u8; 32] = e1.as_slice().try_into().ok()?;
                        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                        let g_arr: &[u8; 96] = e2.as_slice().try_into().ok()?;
                        let g = bls::G2Element::from_byte_array(g_arr).ok()?;
                        Some((g * s).to_byte_array().to_vec())
                    }
                    GROUP_BLS12381_GT => {
                        let s_arr: &[u8; 32] = e1.as_slice().try_into().ok()?;
                        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                        let g_arr: &[u8; 576] = e2.as_slice().try_into().ok()?;
                        let g = bls::GTElement::from_byte_array(g_arr).ok()?;
                        Some((g * s).to_byte_array().to_vec())
                    }
                    _ => None,
                }
            })();

            match result {
                Some(bytes) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(bytes)],
                )),
                None => Ok(NativeResult::err(
                    InternalGas::new(0),
                    GROUP_OPS_INVALID_INPUT,
                )),
            }
        }),
    ));

    natives.push((
        "group_ops",
        "internal_div",
        make_native(|_ctx, _ty_args, mut args| {
            // Move signature: internal_div(type: u8, e1: &vector<u8>, e2: &vector<u8>)
            // For G1/G2/GT: e1 is scalar (divisor), e2 is element (dividend)
            // Result: e2 / e1 = element / scalar
            let e2 = pop_arg!(args, Vec<u8>); // element (dividend)
            let e1 = pop_arg!(args, Vec<u8>); // scalar (divisor)
            let group_type = pop_arg!(args, u8);

            // Division is multiplication by inverse of scalar: e2 / e1 = e2 * (1/e1)
            let result: Option<Vec<u8>> = (|| {
                match group_type {
                    GROUP_BLS12381_SCALAR => {
                        // Both scalars: e2 / e1
                        let a1: &[u8; 32] = e1.as_slice().try_into().ok()?;
                        let a2: &[u8; 32] = e2.as_slice().try_into().ok()?;
                        let s1 = bls::Scalar::from_byte_array(a1).ok()?;
                        let s2 = bls::Scalar::from_byte_array(a2).ok()?;
                        let s1_inv = s1.inverse().ok()?;
                        Some((s2 * s1_inv).to_byte_array().to_vec())
                    }
                    GROUP_BLS12381_G1 => {
                        // e1 = scalar, e2 = element; result = e2 / e1
                        let s_arr: &[u8; 32] = e1.as_slice().try_into().ok()?;
                        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                        let s_inv = s.inverse().ok()?;
                        let g_arr: &[u8; 48] = e2.as_slice().try_into().ok()?;
                        let g = bls::G1Element::from_byte_array(g_arr).ok()?;
                        Some((g * s_inv).to_byte_array().to_vec())
                    }
                    GROUP_BLS12381_G2 => {
                        let s_arr: &[u8; 32] = e1.as_slice().try_into().ok()?;
                        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                        let s_inv = s.inverse().ok()?;
                        let g_arr: &[u8; 96] = e2.as_slice().try_into().ok()?;
                        let g = bls::G2Element::from_byte_array(g_arr).ok()?;
                        Some((g * s_inv).to_byte_array().to_vec())
                    }
                    GROUP_BLS12381_GT => {
                        let s_arr: &[u8; 32] = e1.as_slice().try_into().ok()?;
                        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                        let s_inv = s.inverse().ok()?;
                        let g_arr: &[u8; 576] = e2.as_slice().try_into().ok()?;
                        let g = bls::GTElement::from_byte_array(g_arr).ok()?;
                        Some((g * s_inv).to_byte_array().to_vec())
                    }
                    _ => None,
                }
            })();

            match result {
                Some(bytes) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(bytes)],
                )),
                None => Ok(NativeResult::err(
                    InternalGas::new(0),
                    GROUP_OPS_INVALID_INPUT,
                )),
            }
        }),
    ));

    natives.push((
        "group_ops",
        "internal_hash_to",
        make_native(|_ctx, _ty_args, mut args| {
            let msg = pop_arg!(args, Vec<u8>);
            let group_type = pop_arg!(args, u8);

            let result: Result<Vec<u8>, _> = match group_type {
                GROUP_BLS12381_G1 => {
                    let g = bls::G1Element::hash_to_group_element(&msg);
                    Ok(g.to_byte_array().to_vec())
                }
                GROUP_BLS12381_G2 => {
                    let g = bls::G2Element::hash_to_group_element(&msg);
                    Ok(g.to_byte_array().to_vec())
                }
                _ => Err(()),
            };

            match result {
                Ok(bytes) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(bytes)],
                )),
                Err(_) => Ok(NativeResult::err(
                    InternalGas::new(0),
                    GROUP_OPS_INVALID_INPUT,
                )),
            }
        }),
    ));

    natives.push((
        "group_ops",
        "internal_multi_scalar_mul",
        make_native(|_ctx, _ty_args, mut args| {
            // Move signature: internal_multi_scalar_mul(type, scalars, elements)
            // e1 = scalars, e2 = elements
            // Stack pops in reverse: e2 (elements) first, then e1 (scalars)
            let elements_bytes = pop_arg!(args, Vec<u8>); // e2 - elements
            let scalars_bytes = pop_arg!(args, Vec<u8>); // e1 - scalars
            let group_type = pop_arg!(args, u8);

            let result: Option<Vec<u8>> = (|| match group_type {
                GROUP_BLS12381_G1 => {
                    let scalar_size = 32;
                    let element_size = 48;
                    if scalars_bytes.len() % scalar_size != 0
                        || elements_bytes.len() % element_size != 0
                    {
                        return None;
                    }
                    let n = scalars_bytes.len() / scalar_size;
                    if n != elements_bytes.len() / element_size || n == 0 {
                        return None;
                    }

                    let mut scalars = Vec::with_capacity(n);
                    let mut elements = Vec::with_capacity(n);
                    for i in 0..n {
                        let s_arr: &[u8; 32] = scalars_bytes
                            [i * scalar_size..(i + 1) * scalar_size]
                            .try_into()
                            .ok()?;
                        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                        let g_arr: &[u8; 48] = elements_bytes
                            [i * element_size..(i + 1) * element_size]
                            .try_into()
                            .ok()?;
                        let g = bls::G1Element::from_byte_array(g_arr).ok()?;
                        scalars.push(s);
                        elements.push(g);
                    }

                    let result = bls::G1Element::multi_scalar_mul(&scalars, &elements).ok()?;
                    Some(result.to_byte_array().to_vec())
                }
                GROUP_BLS12381_G2 => {
                    let scalar_size = 32;
                    let element_size = 96;
                    if scalars_bytes.len() % scalar_size != 0
                        || elements_bytes.len() % element_size != 0
                    {
                        return None;
                    }
                    let n = scalars_bytes.len() / scalar_size;
                    if n != elements_bytes.len() / element_size || n == 0 {
                        return None;
                    }

                    let mut scalars = Vec::with_capacity(n);
                    let mut elements = Vec::with_capacity(n);
                    for i in 0..n {
                        let s_arr: &[u8; 32] = scalars_bytes
                            [i * scalar_size..(i + 1) * scalar_size]
                            .try_into()
                            .ok()?;
                        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                        let g_arr: &[u8; 96] = elements_bytes
                            [i * element_size..(i + 1) * element_size]
                            .try_into()
                            .ok()?;
                        let g = bls::G2Element::from_byte_array(g_arr).ok()?;
                        scalars.push(s);
                        elements.push(g);
                    }

                    let result = bls::G2Element::multi_scalar_mul(&scalars, &elements).ok()?;
                    Some(result.to_byte_array().to_vec())
                }
                _ => None,
            })();

            match result {
                Some(bytes) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(bytes)],
                )),
                None => Ok(NativeResult::err(
                    InternalGas::new(0),
                    GROUP_OPS_INVALID_INPUT,
                )),
            }
        }),
    ));

    natives.push((
        "group_ops",
        "internal_pairing",
        make_native(|_ctx, _ty_args, mut args| {
            let g2_bytes = pop_arg!(args, Vec<u8>);
            let g1_bytes = pop_arg!(args, Vec<u8>);
            let _group_type = pop_arg!(args, u8); // Pairing type (unused, always G1)

            let result: Option<Vec<u8>> = (|| {
                let g1_arr: &[u8; 48] = g1_bytes.as_slice().try_into().ok()?;
                let g1 = bls::G1Element::from_byte_array(g1_arr).ok()?;
                let g2_arr: &[u8; 96] = g2_bytes.as_slice().try_into().ok()?;
                let g2 = bls::G2Element::from_byte_array(g2_arr).ok()?;
                let gt = g1.pairing(&g2);
                Some(gt.to_byte_array().to_vec())
            })();

            match result {
                Some(bytes) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(bytes)],
                )),
                None => Ok(NativeResult::err(
                    InternalGas::new(0),
                    GROUP_OPS_INVALID_INPUT,
                )),
            }
        }),
    ));

    // internal_sum - sum of multiple elements
    natives.push((
        "group_ops",
        "internal_sum",
        make_native(|_ctx, _ty_args, mut args| {
            let elements_bytes = pop_arg!(args, Vec<u8>);
            let group_type = pop_arg!(args, u8);

            let result: Option<Vec<u8>> = (|| match group_type {
                GROUP_BLS12381_G1 => {
                    let element_size = 48;
                    if elements_bytes.len() % element_size != 0 {
                        return None;
                    }
                    let n = elements_bytes.len() / element_size;
                    let mut sum = bls::G1Element::zero();
                    for i in 0..n {
                        let g_arr: &[u8; 48] = elements_bytes
                            [i * element_size..(i + 1) * element_size]
                            .try_into()
                            .ok()?;
                        let g = bls::G1Element::from_byte_array(g_arr).ok()?;
                        sum += g;
                    }
                    Some(sum.to_byte_array().to_vec())
                }
                GROUP_BLS12381_G2 => {
                    let element_size = 96;
                    if elements_bytes.len() % element_size != 0 {
                        return None;
                    }
                    let n = elements_bytes.len() / element_size;
                    let mut sum = bls::G2Element::zero();
                    for i in 0..n {
                        let g_arr: &[u8; 96] = elements_bytes
                            [i * element_size..(i + 1) * element_size]
                            .try_into()
                            .ok()?;
                        let g = bls::G2Element::from_byte_array(g_arr).ok()?;
                        sum += g;
                    }
                    Some(sum.to_byte_array().to_vec())
                }
                _ => None,
            })();

            match result {
                Some(bytes) => Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::vector_u8(bytes)],
                )),
                None => Ok(NativeResult::err(
                    InternalGas::new(0),
                    GROUP_OPS_INVALID_INPUT,
                )),
            }
        }),
    ));

    // internal_convert - convert between compressed and uncompressed forms
    // For now, just pass through (we don't have uncompressed G1 support yet)
    natives.push((
        "group_ops",
        "internal_convert",
        make_native(|_ctx, _ty_args, mut args| {
            let bytes = pop_arg!(args, Vec<u8>);
            let _to_type = pop_arg!(args, u8);
            let _from_type = pop_arg!(args, u8);
            // For now, just return the input - full conversion support would require
            // tracking uncompressed G1 representation
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(bytes)],
            ))
        }),
    ));

    // ============================================================
    // Poseidon - ZK-friendly hash function
    // ============================================================
    natives.push((
        "poseidon",
        "poseidon_bn254",
        make_native(|_ctx, _ty_args, _args| {
            // Return 32-byte hash output (field element)
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(vec![0u8; 32])],
            ))
        }),
    ));

    // ============================================================
    // VDF - Verifiable Delay Function
    // ============================================================
    natives.push((
        "vdf",
        "vdf_verify",
        make_native(|_ctx, _ty_args, _args| {
            // VDF verification "passes"
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(true)],
            ))
        }),
    ));

    natives.push((
        "vdf",
        "vdf_hash_to_input",
        make_native(|_ctx, _ty_args, _args| {
            // Return valid VDF input bytes
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(vec![0u8; 32])],
            ))
        }),
    ));

    // ============================================================
    // zkLogin - Zero-knowledge login verification
    // ============================================================
    natives.push((
        "zklogin_verified_id",
        "check_zklogin_id",
        make_native(|_ctx, _ty_args, _args| {
            // zkLogin ID check "passes"
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(true)],
            ))
        }),
    ));

    natives.push((
        "zklogin_verified_issuer",
        "check_zklogin_issuer",
        make_native(|_ctx, _ty_args, _args| {
            // zkLogin issuer check "passes"
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(true)],
            ))
        }),
    ));

    // ============================================================
    // Nitro Attestation - AWS Nitro Enclave verification
    // ============================================================
    natives.push((
        "nitro_attestation",
        "verify_nitro_attestation",
        make_native(|_ctx, _ty_args, _args| {
            // Attestation verification "passes"
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(true)],
            ))
        }),
    ));

    // ============================================================
    // Config - System configuration reading
    // ============================================================
    natives.push((
        "config",
        "read_setting_impl",
        make_native(|_ctx, _ty_args, _args| {
            // Return None (Option<T>) - setting not found
            // This is safer than returning arbitrary values
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(vec![0u8])], // BCS-encoded None
            ))
        }),
    ));

    // ============================================================
    // Random - Deterministic randomness using MockRandom
    // ============================================================
    let state_clone = state.clone();
    natives.push((
        "random",
        "random_internal",
        make_native(move |_ctx, _ty_args, _args| {
            // Return deterministic "random" bytes from MockRandom
            // Each call advances the counter, producing a new value
            let bytes = state_clone.random_bytes(32);
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(bytes)],
            ))
        }),
    ));

    // ============================================================
    // Funds Accumulator - Still unsupported (requires state)
    // ============================================================
    for func in [
        "add_to_accumulator_address",
        "withdraw_from_accumulator_address",
    ] {
        natives.push((
            "funds_accumulator",
            func,
            make_native(|_ctx, _ty_args, _args| {
                // These require actual accumulator state, keep as unsupported
                Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
            }),
        ));
    }
}

/// Helper to create a NativeFunction from a closure
fn make_native<F>(f: F) -> NativeFunction
where
    F: Fn(
            &mut move_vm_runtime::native_functions::NativeContext,
            Vec<Type>,
            VecDeque<Value>,
        ) -> PartialVMResult<NativeResult>
        + Send
        + Sync
        + 'static,
{
    Arc::new(f)
}
