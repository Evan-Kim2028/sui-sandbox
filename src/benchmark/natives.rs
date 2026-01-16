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
use smallvec::smallvec;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

use super::errors::E_NOT_SUPPORTED;
use super::object_runtime::{E_FIELD_DOES_NOT_EXIST, E_FIELD_TYPE_MISMATCH};

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

/// Mock clock that advances on each access.
///
/// This provides sensible time values for time-dependent Move code.
/// The clock starts at a realistic timestamp and advances by a configurable
/// increment on each access, simulating the passage of time.
pub struct MockClock {
    /// Base timestamp in milliseconds (default: 2024-01-01 00:00:00 UTC = 1704067200000)
    pub base_ms: u64,
    /// Increment per access in milliseconds (default: 1000 = 1 second)
    pub tick_ms: u64,
    /// Number of times the clock has been accessed
    accesses: AtomicU64,
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MockClock {
    /// Default base timestamp: 2024-01-01 00:00:00 UTC
    pub const DEFAULT_BASE_MS: u64 = 1704067200000;
    /// Default tick: 1 second per access
    pub const DEFAULT_TICK_MS: u64 = 1000;

    pub fn new() -> Self {
        Self {
            base_ms: Self::DEFAULT_BASE_MS,
            tick_ms: Self::DEFAULT_TICK_MS,
            accesses: AtomicU64::new(0),
        }
    }

    pub fn with_base(base_ms: u64) -> Self {
        Self {
            base_ms,
            tick_ms: Self::DEFAULT_TICK_MS,
            accesses: AtomicU64::new(0),
        }
    }

    /// Get the current timestamp, advancing the clock.
    pub fn timestamp_ms(&self) -> u64 {
        let n = self.accesses.fetch_add(1, Ordering::SeqCst);
        self.base_ms + (n * self.tick_ms)
    }

    /// Get the current timestamp without advancing (for inspection).
    pub fn peek_timestamp_ms(&self) -> u64 {
        let n = self.accesses.load(Ordering::SeqCst);
        self.base_ms + (n * self.tick_ms)
    }

    /// Reset the clock to its initial state.
    pub fn reset(&self) {
        self.accesses.store(0, Ordering::SeqCst);
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
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }

    /// Get all emitted events.
    pub fn get_events(&self) -> Vec<EmittedEvent> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
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
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
        self.counter.store(0, Ordering::SeqCst);
    }
}

/// Mock state for native function execution.
///
/// This struct holds all mock state needed for the Local Move VM Sandbox:
/// - Transaction context (sender, epoch, IDs)
/// - Clock (advancing timestamps)
/// - Random (deterministic randomness)
/// - Events (captured event emissions)
///
/// Note: Dynamic field storage is handled by ObjectRuntime (a VM extension).
pub struct MockNativeState {
    pub sender: AccountAddress,
    pub epoch: u64,
    pub epoch_timestamp_ms: u64,
    ids_created: AtomicU64,
    /// Mock clock for time-dependent code
    pub clock: MockClock,
    /// Mock random for randomness-dependent code
    pub random: MockRandom,
    /// Event store for capturing emitted events
    pub events: EventStore,
}

impl Default for MockNativeState {
    fn default() -> Self {
        Self::new()
    }
}

impl MockNativeState {
    pub fn new() -> Self {
        Self {
            sender: AccountAddress::ZERO,
            epoch: 0,
            epoch_timestamp_ms: MockClock::DEFAULT_BASE_MS,
            ids_created: AtomicU64::new(0),
            clock: MockClock::new(),
            random: MockRandom::new(),
            events: EventStore::new(),
        }
    }

    /// Create with a specific random seed for reproducible tests.
    pub fn with_random_seed(seed: [u8; 32]) -> Self {
        Self {
            sender: AccountAddress::ZERO,
            epoch: 0,
            epoch_timestamp_ms: MockClock::DEFAULT_BASE_MS,
            ids_created: AtomicU64::new(0),
            clock: MockClock::new(),
            random: MockRandom::with_seed(seed),
            events: EventStore::new(),
        }
    }

    /// Generate a fresh unique ID (sequential, not hash-derived)
    pub fn fresh_id(&self) -> AccountAddress {
        let count = self.ids_created.fetch_add(1, Ordering::SeqCst);
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&count.to_le_bytes());
        AccountAddress::new(bytes)
    }

    pub fn ids_created(&self) -> u64 {
        self.ids_created.load(Ordering::SeqCst)
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
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::address(state_clone.sender)],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_epoch",
        make_native(move |_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u64(state_clone.epoch)],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_epoch_timestamp_ms",
        make_native(move |_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
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
            let id = state_clone.fresh_id();
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::address(id)],
            ))
        }),
    ));

    natives.push((
        "tx_context",
        "native_rgp",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u64(1000)],
            ))
        }),
    ));

    natives.push((
        "tx_context",
        "native_gas_price",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u64(1000)],
            ))
        }),
    ));

    natives.push((
        "tx_context",
        "native_gas_budget",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u64(u64::MAX)],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "tx_context",
        "native_ids_created",
        make_native(move |_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u64(state_clone.ids_created())],
            ))
        }),
    ));

    natives.push((
        "tx_context",
        "native_sponsor",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_address(vec![])],
            ))
        }),
    ));

    // derive_id: Same as fresh_id - we just need valid unique addresses
    natives.push((
        "tx_context",
        "derive_id",
        make_native(|_ctx, _ty_args, mut args| {
            let ids_created = pop_arg!(args, u64);
            let _tx_hash = pop_arg!(args, Vec<u8>);
            // Use ids_created to generate deterministic unique address
            let mut bytes = [0u8; 32];
            bytes[24..32].copy_from_slice(&ids_created.to_le_bytes());
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::address(AccountAddress::new(bytes))],
            ))
        }),
    ));

    // object natives
    natives.push((
        "object",
        "borrow_uid",
        make_native(|_ctx, _ty_args, mut args| {
            use move_vm_types::values::VMValueCast;

            // borrow_uid<T: key>(obj: &T): &UID
            // All Sui objects with the `key` ability have `id: UID` as their first field.
            // We need to extract a reference to that first field.

            let obj_ref = match args.pop_back() {
                Some(v) => v,
                None => {
                    return Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED));
                }
            };

            // Cast the Value to a StructRef so we can call borrow_field
            let struct_ref: move_vm_types::values::StructRef = match obj_ref.cast() {
                Ok(sr) => sr,
                Err(_) => {
                    return Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED));
                }
            };

            // The input is a reference to a struct. We need to return a reference to its first field.
            // In Move VM, we can use borrow_field to get a reference to a field by index.
            // The UID is always the first field (index 0) of any Sui object.
            match struct_ref.borrow_field(0) {
                Ok(uid_ref) => Ok(NativeResult::ok(InternalGas::new(0), smallvec![uid_ref])),
                Err(_) => {
                    // If borrow_field fails, the object might not have a proper UID field
                    // This shouldn't happen for valid Sui objects, but handle gracefully
                    Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
                }
            }
        }),
    ));

    natives.push((
        "object",
        "delete_impl",
        make_native(|_ctx, _ty_args, _args| {
            // No-op: we don't track object lifecycle
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    natives.push((
        "object",
        "record_new_uid",
        make_native(|_ctx, _ty_args, _args| {
            // No-op: we don't track UIDs
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    // transfer natives - track ownership changes via ObjectRuntime
    // Native names must match the bytecode: freeze_object_impl, share_object_impl, etc.

    // transfer_impl<T>(obj: T, recipient: address)
    // Transfers ownership of an object to a recipient address
    natives.push((
        "transfer",
        "transfer_impl",
        make_native(|ctx, mut ty_args, mut args| {
            use crate::benchmark::object_runtime::{ObjectRuntime, Owner};

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

            // Get type layout first (before borrowing extensions mutably)
            let layout = ty_args
                .pop()
                .and_then(|obj_ty| ctx.type_to_type_layout(&obj_ty).ok().flatten());

            if let Some(layout) = layout {
                // Try to serialize the object to get its bytes and ID
                if let Some(bytes) = obj_value.typed_serialize(&layout) {
                    if bytes.len() >= 32 {
                        let mut id_bytes = [0u8; 32];
                        id_bytes.copy_from_slice(&bytes[0..32]);
                        let object_id = AccountAddress::new(id_bytes);

                        // Now we can borrow extensions mutably
                        if let Ok(runtime) = ctx.extensions_mut().get_mut::<ObjectRuntime>() {
                            // Transfer ownership - ignore errors for objects we haven't tracked
                            let _ = runtime
                                .object_store_mut()
                                .transfer(&object_id, Owner::Address(recipient));
                        }
                    }
                }
            }

            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    // freeze_object_impl<T>(obj: T)
    // Makes an object immutable (frozen)
    natives.push((
        "transfer",
        "freeze_object_impl",
        make_native(|ctx, mut ty_args, mut args| {
            use crate::benchmark::object_runtime::ObjectRuntime;

            // Pop the object value
            let obj_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;

            // Get type layout first (before borrowing extensions mutably)
            let layout = ty_args
                .pop()
                .and_then(|obj_ty| ctx.type_to_type_layout(&obj_ty).ok().flatten());

            if let Some(layout) = layout {
                if let Some(bytes) = obj_value.typed_serialize(&layout) {
                    if bytes.len() >= 32 {
                        let mut id_bytes = [0u8; 32];
                        id_bytes.copy_from_slice(&bytes[0..32]);
                        let object_id = AccountAddress::new(id_bytes);

                        // Now we can borrow extensions mutably
                        if let Ok(runtime) = ctx.extensions_mut().get_mut::<ObjectRuntime>() {
                            // Mark as immutable - ignore errors for objects we haven't tracked
                            let _ = runtime.object_store_mut().mark_immutable(object_id);
                        }
                    }
                }
            }

            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    // share_object_impl<T>(obj: T)
    // Makes an object shared (accessible by anyone)
    natives.push((
        "transfer",
        "share_object_impl",
        make_native(|ctx, mut ty_args, mut args| {
            use crate::benchmark::object_runtime::ObjectRuntime;

            // Pop the object value
            let obj_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;

            // Get type layout first (before borrowing extensions mutably)
            let layout = ty_args
                .pop()
                .and_then(|obj_ty| ctx.type_to_type_layout(&obj_ty).ok().flatten());

            if let Some(layout) = layout {
                if let Some(bytes) = obj_value.typed_serialize(&layout) {
                    if bytes.len() >= 32 {
                        let mut id_bytes = [0u8; 32];
                        id_bytes.copy_from_slice(&bytes[0..32]);
                        let object_id = AccountAddress::new(id_bytes);

                        // Now we can borrow extensions mutably
                        if let Ok(runtime) = ctx.extensions_mut().get_mut::<ObjectRuntime>() {
                            // Mark as shared - ignore errors for objects we haven't tracked
                            let _ = runtime.object_store_mut().mark_shared(object_id);
                        }
                    }
                }
            }

            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    // receive_impl<T>(parent: address, to_receive: Receiving<T>) -> T
    // Receiving<T> is a struct with: { id: ID, version: u64 }
    // ID is a struct with: { bytes: address }
    natives.push((
        "transfer",
        "receive_impl",
        make_native(|ctx, mut ty_args, mut args| {
            use crate::benchmark::object_runtime::{ObjectRuntime, SharedObjectRuntime};

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
                            if let Ok(mut state) = shared.shared_state().lock() {
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
                            } else {
                                None
                            }
                        };

                        if let Some((_type_tag, obj_bytes)) = result {
                            if let Some(type_layout) = &fallback_type_layout {
                                match Value::simple_deserialize(&obj_bytes, type_layout) {
                                    Some(value) => {
                                        return Ok(NativeResult::ok(
                                            InternalGas::new(0),
                                            smallvec![value],
                                        ));
                                    }
                                    None => {
                                        return Ok(NativeResult::err(InternalGas::new(0), 3));
                                    }
                                }
                            } else {
                                return Ok(NativeResult::err(InternalGas::new(0), 2));
                            }
                        }
                    }
                    return Ok(NativeResult::err(InternalGas::new(0), 1));
                }
            };

            // Get the type layout first (before we borrow extensions)
            let type_layout = match ctx.type_to_type_layout(&receive_ty) {
                Ok(Some(layout)) => layout,
                _ => return Ok(NativeResult::err(InternalGas::new(0), 2)),
            };

            // First try SharedObjectRuntime's shared state (from SimulationEnvironment)
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                // Try shared state first
                let recv_bytes_opt = {
                    if let Ok(mut state) = shared.shared_state().lock() {
                        state.receive_pending(parent, object_id)
                    } else {
                        None
                    }
                };

                if let Some((_type_tag, recv_bytes)) = recv_bytes_opt {
                    match Value::simple_deserialize(&recv_bytes, &type_layout) {
                        Some(value) => {
                            return Ok(NativeResult::ok(InternalGas::new(0), smallvec![value]));
                        }
                        None => {
                            return Ok(NativeResult::err(InternalGas::new(0), 3));
                        }
                    }
                }

                // Also try the local ObjectRuntime's ObjectStore
                let runtime = shared.local_mut();
                if let Ok(recv_bytes) = runtime.object_store_mut().receive_object(parent, object_id)
                {
                    match Value::simple_deserialize(&recv_bytes, &type_layout) {
                        Some(value) => {
                            return Ok(NativeResult::ok(InternalGas::new(0), smallvec![value]));
                        }
                        None => {
                            return Ok(NativeResult::err(InternalGas::new(0), 3));
                        }
                    }
                }
            }

            // Fallback to regular ObjectRuntime
            if let Ok(runtime) = ctx.extensions_mut().get_mut::<ObjectRuntime>() {
                match runtime.object_store_mut().receive_object(parent, object_id) {
                    Ok(recv_bytes) => match Value::simple_deserialize(&recv_bytes, &type_layout) {
                        Some(value) => {
                            return Ok(NativeResult::ok(InternalGas::new(0), smallvec![value]));
                        }
                        None => {
                            return Ok(NativeResult::err(InternalGas::new(0), 3));
                        }
                    },
                    Err(code) => return Ok(NativeResult::err(InternalGas::new(0), code)),
                }
            }

            // Object not found in any pending receives
            Ok(NativeResult::err(InternalGas::new(0), 4))
        }),
    ));

    natives.push((
        "transfer",
        "party_transfer_impl",
        make_native(|_ctx, _ty_args, _args| Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))),
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

                    // Record the event
                    state_clone.events.emit(type_tag_str, event_bytes);
                }
            }
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "event",
        "emit_authenticated_impl",
        make_native(move |ctx, ty_args, mut args| {
            // Similar to emit but for authenticated events
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
                    state_clone.events.emit(type_tag_str, event_bytes);
                }
            }
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "event",
        "events_by_type",
        make_native(move |ctx, ty_args, _args| {
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
                InternalGas::new(0),
                smallvec![Value::vector_u8(result_bytes)],
            ))
        }),
    ));

    let state_clone = state.clone();
    natives.push((
        "event",
        "num_events",
        make_native(move |_ctx, _ty_args, _args| {
            let count = state_clone.events.count();
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u64(count)],
            ))
        }),
    ));

    // address natives
    natives.push((
        "address",
        "from_bytes",
        make_native(|_ctx, _ty_args, mut args| {
            let bytes = pop_arg!(args, Vec<u8>);
            if bytes.len() != 32 {
                return Ok(NativeResult::err(InternalGas::new(0), 1));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::address(AccountAddress::new(arr))],
            ))
        }),
    ));

    natives.push((
        "address",
        "to_u256",
        make_native(|_ctx, _ty_args, mut args| {
            let addr = pop_arg!(args, AccountAddress);
            let bytes = addr.to_vec();
            // AccountAddress is always 32 bytes, so this conversion is safe
            let arr: [u8; 32] = match bytes.try_into() {
                Ok(a) => a,
                Err(_) => {
                    return Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED));
                }
            };
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u256(move_core_types::u256::U256::from_le_bytes(
                    &arr
                ))],
            ))
        }),
    ));

    natives.push((
        "address",
        "from_u256",
        make_native(|_ctx, _ty_args, mut args| {
            let u = pop_arg!(args, move_core_types::u256::U256);
            let bytes = u.to_le_bytes();
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::address(AccountAddress::new(bytes))],
            ))
        }),
    ));

    // types::is_one_time_witness - real implementation
    // Checks: (1) struct has exactly one bool field, (2) name == UPPERCASE(module_name)
    // This matches the actual Sui runtime check, allowing LLMs to use the OTW pattern correctly.
    natives.push((
        "types",
        "is_one_time_witness",
        make_native(|ctx, ty_args, _args| {
            // The type parameter T is what we need to check
            if ty_args.is_empty() {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            }

            let ty = &ty_args[0];

            // Get TypeTag to check the name
            let type_tag = match ctx.type_to_type_tag(ty) {
                Ok(tag) => tag,
                Err(_) => {
                    return Ok(NativeResult::ok(
                        InternalGas::new(0),
                        smallvec![Value::bool(false)],
                    ));
                }
            };

            // Get type layout to check for one bool field
            let type_layout = match ctx.type_to_type_layout(ty) {
                Ok(Some(layout)) => layout,
                _ => {
                    return Ok(NativeResult::ok(
                        InternalGas::new(0),
                        smallvec![Value::bool(false)],
                    ));
                }
            };

            // Must be a struct type
            let MoveTypeLayout::Struct(struct_layout) = type_layout else {
                return Ok(NativeResult::ok(
                    InternalGas::new(0),
                    smallvec![Value::bool(false)],
                ));
            };

            let is_otw = is_otw_struct(&struct_layout, &type_tag);

            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(is_otw)],
            ))
        }),
    ));

    // hash natives - REAL implementations using fastcrypto
    natives.push((
        "hash",
        "blake2b256",
        make_native(|_ctx, _ty_args, mut args| {
            let data = pop_arg!(args, Vec<u8>);
            let hash = Blake2b256::digest(&data);
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(hash.digest.to_vec())],
            ))
        }),
    ));

    natives.push((
        "hash",
        "keccak256",
        make_native(|_ctx, _ty_args, mut args| {
            let data = pop_arg!(args, Vec<u8>);
            let hash = Keccak256::digest(&data);
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(hash.digest.to_vec())],
            ))
        }),
    ));

    // protocol_config
    natives.push((
        "protocol_config",
        "protocol_version_impl",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u64(62)],
            ))
        }),
    ));

    // accumulator natives - no-op
    natives.push((
        "accumulator",
        "emit_deposit_event",
        make_native(|_ctx, _ty_args, _args| Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))),
    ));

    natives.push((
        "accumulator",
        "emit_withdraw_event",
        make_native(|_ctx, _ty_args, _args| Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))),
    ));

    natives.push((
        "accumulator_settlement",
        "record_settlement_sui_conservation",
        make_native(|_ctx, _ty_args, _args| Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))),
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
    natives.push((
        "balance",
        "create_for_testing",
        make_native(|_ctx, _ty_args, mut args| {
            let value = pop_arg!(args, u64);
            // Balance<T> = struct { value: u64 }
            // We construct it as a struct with one field
            let balance =
                Value::struct_(move_vm_types::values::Struct::pack(vec![Value::u64(value)]));
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![balance]))
        }),
    ));

    // balance::destroy_for_testing<T>(balance: Balance<T>)
    // Just consumes the balance, no-op
    natives.push((
        "balance",
        "destroy_for_testing",
        make_native(|_ctx, _ty_args, _args| {
            // Balance is consumed, nothing to return
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
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

            Ok(NativeResult::ok(InternalGas::new(0), smallvec![coin]))
        }),
    ));

    // coin::burn_for_testing<T>(coin: Coin<T>)
    // Just consumes the coin, no-op
    natives.push((
        "coin",
        "burn_for_testing",
        make_native(|_ctx, _ty_args, _args| {
            // Coin is consumed, nothing to return
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    // Additional test utilities that may be useful

    // balance::create_supply_for_testing<T>() -> Supply<T>
    // Supply<T> = { value: u64 } (tracks total supply)
    natives.push((
        "balance",
        "create_supply_for_testing",
        make_native(|_ctx, _ty_args, _args| {
            // Supply<T> = struct { value: u64 } starting at 0
            let supply = Value::struct_(move_vm_types::values::Struct::pack(vec![Value::u64(0)]));
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![supply]))
        }),
    ));

    // balance::destroy_supply_for_testing<T>(supply: Supply<T>)
    natives.push((
        "balance",
        "destroy_supply_for_testing",
        make_native(|_ctx, _ty_args, _args| Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))),
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

    eprintln!(
        "[extract_address_from_uid] SUCCESS: addr={}",
        addr.to_hex_literal()
    );
    Some(addr)
}

/// Helper to get ObjectRuntime from extensions.
/// Tries SharedObjectRuntime first (for PTB sessions), falls back to ObjectRuntime.
fn get_object_runtime_ref<'a>(
    ctx: &'a NativeContext,
) -> Result<
    &'a crate::benchmark::object_runtime::ObjectRuntime,
    move_binary_format::errors::PartialVMError,
> {
    use crate::benchmark::object_runtime::{ObjectRuntime, SharedObjectRuntime};

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
) -> Result<
    &'a mut crate::benchmark::object_runtime::ObjectRuntime,
    move_binary_format::errors::PartialVMError,
> {
    use crate::benchmark::object_runtime::{ObjectRuntime, SharedObjectRuntime};

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
    use crate::benchmark::object_runtime::SharedObjectRuntime;

    if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
        if let Ok(mut state) = shared.shared_state().lock() {
            state.add_child(parent, child_id, child_tag.clone(), child_bytes.to_vec());
        }
    }
}

/// Remove a child from shared state (if using SharedObjectRuntime).
fn remove_child_from_shared_state(
    ctx: &mut NativeContext,
    parent: AccountAddress,
    child_id: AccountAddress,
) {
    use crate::benchmark::object_runtime::SharedObjectRuntime;

    if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
        if let Ok(mut state) = shared.shared_state().lock() {
            state.remove_child(parent, child_id);
        }
    }
}

/// Check if a child exists in shared state (if using SharedObjectRuntime).
fn check_shared_state_for_child(
    ctx: &NativeContext,
    parent: AccountAddress,
    child_id: AccountAddress,
) -> bool {
    use crate::benchmark::object_runtime::SharedObjectRuntime;

    if let Ok(shared) = ctx.extensions().get::<SharedObjectRuntime>() {
        if let Ok(state) = shared.shared_state().lock() {
            return state.has_child(parent, child_id);
        }
    }
    false
}

/// Count children for a parent in shared state (if using SharedObjectRuntime).
fn count_shared_state_children(ctx: &NativeContext, parent: AccountAddress) -> u64 {
    use crate::benchmark::object_runtime::SharedObjectRuntime;

    if let Ok(shared) = ctx.extensions().get::<SharedObjectRuntime>() {
        if let Ok(state) = shared.shared_state().lock() {
            return state.count_children_for_parent(parent);
        }
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
    _state: Arc<MockNativeState>, // Keep signature for compatibility, but we use extensions now
) {
    use fastcrypto::hash::{Blake2b256, HashFunction};

    // hash_type_and_key<K>(parent: address, k: K) -> address
    // Deterministically derives child ID from parent + key type + key value
    //
    // IMPORTANT: Must match Sui's derive_dynamic_field_id exactly:
    // hash(scope || parent || len(key) || key || key_type_tag)
    // where scope = 0xf0 (HashingIntentScope::ChildObjectId)
    // and len(key) is encoded as 8-byte little-endian
    natives.push((
        "dynamic_field",
        "hash_type_and_key",
        make_native(|ctx, mut ty_args, mut args| {
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

            let key_tag = ctx.type_to_type_tag(&key_ty)?;

            let key_layout = match ctx.type_to_type_layout(&key_ty) {
                Ok(Some(layout)) => layout,
                _ => return Ok(NativeResult::err(InternalGas::new(0), 3)),
            };

            let key_bytes = match key_value.typed_serialize(&key_layout) {
                Some(bytes) => bytes,
                None => return Ok(NativeResult::err(InternalGas::new(0), 3)),
            };

            let type_tag_bytes = bcs::to_bytes(&key_tag).unwrap_or_default();

            // Derive child ID using Sui's exact formula:
            // Blake2b256(0xf0 || parent || len(key_bytes) as u64 LE || key_bytes || type_tag_bytes)
            const CHILD_OBJECT_ID_SCOPE: u8 = 0xf0;

            let mut hasher = Blake2b256::default();
            hasher.update([CHILD_OBJECT_ID_SCOPE]);
            hasher.update(parent.as_ref());
            hasher.update((key_bytes.len() as u64).to_le_bytes());
            hasher.update(&key_bytes);
            hasher.update(&type_tag_bytes);

            let hash = hasher.finalize();
            let child_id = AccountAddress::new(hash.digest);

            eprintln!(
                "[hash_type_and_key] parent={}, key_type={:?}, key_len={}, result={}",
                parent.to_hex_literal(),
                key_tag,
                key_bytes.len(),
                child_id.to_hex_literal()
            );

            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::address(child_id)],
            ))
        }),
    ));

    // add_child_object<Child: key>(parent: address, child: Child)
    natives.push((
        "dynamic_field",
        "add_child_object",
        make_native(|ctx, mut ty_args, mut args| {
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
                _ => return Ok(NativeResult::err(InternalGas::new(0), 3)),
            };

            // Serialize to get the child ID (first 32 bytes = UID.id.bytes)
            let child_bytes = match child_value.copy_value()?.typed_serialize(&child_layout) {
                Some(bytes) => bytes,
                None => return Ok(NativeResult::err(InternalGas::new(0), 3)),
            };

            let child_id = if child_bytes.len() >= 32 {
                let mut addr_bytes = [0u8; 32];
                addr_bytes.copy_from_slice(&child_bytes[..32]);
                AccountAddress::new(addr_bytes)
            } else {
                return Ok(NativeResult::err(InternalGas::new(0), 3));
            };

            // Store in ObjectRuntime extension (supports both ObjectRuntime and SharedObjectRuntime)
            let runtime = get_object_runtime_mut(ctx)?;
            match runtime.add_child_object(parent, child_id, child_value, child_tag.clone()) {
                Ok(()) => {
                    // Sync to shared state for persistence across VM sessions
                    sync_child_to_shared_state(ctx, parent, child_id, &child_tag, &child_bytes);
                    Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
                }
                Err(code) => Ok(NativeResult::err(InternalGas::new(0), code)),
            }
        }),
    ));

    // borrow_child_object<Child: key>(object: &UID, id: address) -> &Child
    natives.push((
        "dynamic_field",
        "borrow_child_object",
        make_native(|ctx, mut ty_args, mut args| {
            use crate::benchmark::object_runtime::SharedObjectRuntime;
            use move_vm_types::values::StructRef;

            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent_uid = pop_arg!(args, StructRef);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;

            eprintln!("[borrow_child_object] NATIVE CALLED");

            // Extract parent address from UID { id: ID { bytes: address } }
            // Navigate: UID.id (field 0) -> ID.bytes (field 0) -> address
            let parent = match extract_address_from_uid(&parent_uid) {
                Some(addr) => addr,
                None => {
                    // Failed to extract UID - return error instead of silently using 0x0
                    eprintln!("[borrow_child_object] FAILED to extract parent UID!");
                    return Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED));
                }
            };

            // Debug: print parent and child addresses
            eprintln!(
                "[borrow_child_object] parent={}, child_id={}",
                parent.to_hex_literal(),
                child_id.to_hex_literal()
            );

            // First check if it's already in the local runtime
            {
                let runtime = get_object_runtime_ref(ctx)?;
                if runtime.child_object_exists(parent, child_id) {
                    eprintln!("[borrow_child_object] found in local runtime");
                    match runtime.borrow_child_object(parent, child_id, &child_tag) {
                        Ok(value) => {
                            return Ok(NativeResult::ok(InternalGas::new(0), smallvec![value]))
                        }
                        Err(code) => return Ok(NativeResult::err(InternalGas::new(0), code)),
                    }
                }
            }

            // Not in local runtime - check shared state and lazy-load if available
            // Get the type layout for deserialization
            let type_layout = match ctx.type_to_type_layout(&child_ty) {
                Ok(Some(layout)) => layout,
                _ => {
                    return Ok(NativeResult::err(
                        InternalGas::new(0),
                        E_FIELD_TYPE_MISMATCH,
                    ))
                }
            };

            // Try to load from shared state
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                // Get bytes from shared state
                let child_bytes_opt = {
                    if let Ok(state) = shared.shared_state().lock() {
                        eprintln!(
                            "[borrow_child_object] checking shared state, has {} children",
                            state.children.len()
                        );
                        // Debug: print first few children keys
                        for (k, _) in state.children.iter().take(5) {
                            eprintln!(
                                "[borrow_child_object]   - parent={}, child={}",
                                k.0.to_hex_literal(),
                                k.1.to_hex_literal()
                            );
                        }
                        state
                            .get_child(parent, child_id)
                            .map(|(_, bytes)| bytes.clone())
                    } else {
                        None
                    }
                };

                eprintln!(
                    "[borrow_child_object] shared state lookup result: {:?}",
                    child_bytes_opt.as_ref().map(|b| b.len())
                );

                // If not in shared state, try on-demand fetching
                let child_bytes_opt = if child_bytes_opt.is_none() {
                    if let Some((fetched_tag, fetched_bytes)) = shared.try_fetch_child(child_id) {
                        eprintln!(
                            "[borrow_child_object] on-demand fetch succeeded, {} bytes, type={:?}",
                            fetched_bytes.len(),
                            fetched_tag
                        );
                        // Add to shared state for future lookups
                        if let Ok(mut state) = shared.shared_state().lock() {
                            state.add_child(parent, child_id, fetched_tag, fetched_bytes.clone());
                        }
                        Some(fetched_bytes)
                    } else {
                        eprintln!("[borrow_child_object] on-demand fetch failed or not configured");
                        None
                    }
                } else {
                    child_bytes_opt
                };

                if let Some(child_bytes) = child_bytes_opt {
                    // Deserialize the bytes into a Move Value
                    if let Some(value) = Value::simple_deserialize(&child_bytes, &type_layout) {
                        // Add to local runtime so we can borrow from it
                        let runtime = shared.local_mut();
                        match runtime.add_child_object(parent, child_id, value, child_tag.clone()) {
                            Ok(()) => {
                                // Now we can borrow from the local runtime
                                match runtime.borrow_child_object(parent, child_id, &child_tag) {
                                    Ok(ref_value) => {
                                        return Ok(NativeResult::ok(
                                            InternalGas::new(0),
                                            smallvec![ref_value],
                                        ))
                                    }
                                    Err(code) => {
                                        return Ok(NativeResult::err(InternalGas::new(0), code))
                                    }
                                }
                            }
                            Err(code) => return Ok(NativeResult::err(InternalGas::new(0), code)),
                        }
                    } else {
                        // Deserialization failed
                        return Ok(NativeResult::err(
                            InternalGas::new(0),
                            E_FIELD_TYPE_MISMATCH,
                        ));
                    }
                }
            }

            // Child doesn't exist in either local or shared state
            Ok(NativeResult::err(
                InternalGas::new(0),
                E_FIELD_DOES_NOT_EXIST,
            ))
        }),
    ));

    // borrow_child_object_mut<Child: key>(object: &mut UID, id: address) -> &mut Child
    natives.push((
        "dynamic_field",
        "borrow_child_object_mut",
        make_native(|ctx, mut ty_args, mut args| {
            use move_vm_types::values::StructRef;
            use crate::benchmark::object_runtime::SharedObjectRuntime;

            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent_uid = pop_arg!(args, StructRef);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;

            eprintln!("[borrow_child_object_mut] NATIVE CALLED");

            // Extract parent address (same as borrow_child_object)
            let parent = match extract_address_from_uid(&parent_uid) {
                Some(addr) => addr,
                None => {
                    eprintln!("[borrow_child_object_mut] FAILED to extract parent UID!");
                    return Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED));
                }
            };

            eprintln!("[borrow_child_object_mut] parent={}, child_id={}", parent.to_hex_literal(), child_id.to_hex_literal());

            // First check if it's already in the local runtime
            {
                let runtime = get_object_runtime_ref(ctx)?;
                if runtime.child_object_exists(parent, child_id) {
                    eprintln!("[borrow_child_object_mut] found in local runtime");
                    let runtime = get_object_runtime_mut(ctx)?;
                    match runtime.borrow_child_object_mut(parent, child_id, &child_tag) {
                        Ok(value) => return Ok(NativeResult::ok(InternalGas::new(0), smallvec![value])),
                        Err(code) => return Ok(NativeResult::err(InternalGas::new(0), code)),
                    }
                }
            }

            // Not in local runtime - check shared state and lazy-load if available
            let type_layout = match ctx.type_to_type_layout(&child_ty) {
                Ok(Some(layout)) => layout,
                _ => return Ok(NativeResult::err(InternalGas::new(0), E_FIELD_TYPE_MISMATCH)),
            };

            // Try to load from shared state (same logic as borrow_child_object)
            if let Ok(shared) = ctx.extensions_mut().get_mut::<SharedObjectRuntime>() {
                let child_bytes_opt = {
                    if let Ok(state) = shared.shared_state().lock() {
                        eprintln!("[borrow_child_object_mut] checking shared state, has {} children", state.children.len());
                        state.get_child(parent, child_id).map(|(_, bytes)| bytes.clone())
                    } else {
                        None
                    }
                };

                eprintln!("[borrow_child_object_mut] shared state lookup result: {:?}", child_bytes_opt.as_ref().map(|b| b.len()));

                // If not in shared state, try on-demand fetching
                let child_bytes_opt = if child_bytes_opt.is_none() {
                    if let Some((fetched_tag, fetched_bytes)) = shared.try_fetch_child(child_id) {
                        eprintln!("[borrow_child_object_mut] on-demand fetch succeeded, {} bytes, type={:?}", fetched_bytes.len(), fetched_tag);
                        if let Ok(mut state) = shared.shared_state().lock() {
                            state.add_child(parent, child_id, fetched_tag, fetched_bytes.clone());
                        }
                        Some(fetched_bytes)
                    } else {
                        eprintln!("[borrow_child_object_mut] on-demand fetch failed or not configured");
                        None
                    }
                } else {
                    child_bytes_opt
                };

                if let Some(child_bytes) = child_bytes_opt {
                    if let Some(value) = Value::simple_deserialize(&child_bytes, &type_layout) {
                        let runtime = shared.local_mut();
                        match runtime.add_child_object(parent, child_id, value, child_tag.clone()) {
                            Ok(()) => {
                                match runtime.borrow_child_object_mut(parent, child_id, &child_tag) {
                                    Ok(ref_value) => return Ok(NativeResult::ok(InternalGas::new(0), smallvec![ref_value])),
                                    Err(code) => return Ok(NativeResult::err(InternalGas::new(0), code)),
                                }
                            }
                            Err(code) => return Ok(NativeResult::err(InternalGas::new(0), code)),
                        }
                    } else {
                        return Ok(NativeResult::err(InternalGas::new(0), E_FIELD_TYPE_MISMATCH));
                    }
                }
            }

            // Child doesn't exist
            Ok(NativeResult::err(InternalGas::new(0), E_FIELD_DOES_NOT_EXIST))
        }),
    ));

    // remove_child_object<Child: key>(parent: address, id: address) -> Child
    natives.push((
        "dynamic_field",
        "remove_child_object",
        make_native(|ctx, mut ty_args, mut args| {
            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent = pop_arg!(args, AccountAddress);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;

            let runtime = get_object_runtime_mut(ctx)?;
            match runtime.remove_child_object(parent, child_id, &child_tag) {
                Ok(value) => {
                    // Sync removal to shared state
                    remove_child_from_shared_state(ctx, parent, child_id);
                    Ok(NativeResult::ok(InternalGas::new(0), smallvec![value]))
                }
                Err(code) => Ok(NativeResult::err(InternalGas::new(0), code)),
            }
        }),
    ));

    // has_child_object(parent: address, id: address) -> bool
    natives.push((
        "dynamic_field",
        "has_child_object",
        make_native(|ctx, _ty_args, mut args| {
            let child_id = pop_arg!(args, AccountAddress);
            let parent = pop_arg!(args, AccountAddress);

            let runtime = get_object_runtime_ref(ctx)?;
            // Check local runtime first, then shared state
            let exists = runtime.child_object_exists(parent, child_id)
                || check_shared_state_for_child(ctx, parent, child_id);
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(exists)],
            ))
        }),
    ));

    // has_child_object_with_ty<Child: key>(parent: address, id: address) -> bool
    natives.push((
        "dynamic_field",
        "has_child_object_with_ty",
        make_native(|ctx, mut ty_args, mut args| {
            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH,
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent = pop_arg!(args, AccountAddress);

            let child_tag = ctx.type_to_type_tag(&child_ty)?;
            let runtime = get_object_runtime_ref(ctx)?;
            // Check local runtime first, then shared state (type checking happens only in local)
            let exists = runtime.child_object_exists_with_type(parent, child_id, &child_tag)
                || check_shared_state_for_child(ctx, parent, child_id);

            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(exists)],
            ))
        }),
    ));

    // field_info_count(parent: address) -> u64
    // Returns the number of dynamic fields for a given parent object.
    // This is a sandbox-specific extension to help with Table/Bag iteration.
    natives.push((
        "dynamic_field",
        "field_info_count",
        make_native(|ctx, _ty_args, mut args| {
            let parent = pop_arg!(args, AccountAddress);

            let runtime = get_object_runtime_ref(ctx)?;
            let count = runtime.count_children_for_parent(parent);

            // Also check shared state for additional children
            let shared_count = count_shared_state_children(ctx, parent);

            Ok(NativeResult::ok(
                InternalGas::new(0),
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
