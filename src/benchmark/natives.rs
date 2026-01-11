//! Mock native function implementations for Sui Move VM execution.
//!
//! This module provides native function implementations that enable Tier B execution
//! of Sui Move code without requiring the full Sui runtime.
//!
//! ## Semantic Model
//!
//! Tier B = "execution completes without abort", NOT "produces correct values".
//!
//! For type inhabitation, we only need to verify that a function CAN be called
//! with synthesized arguments. We don't care about return values or side effects.
//!
//! ## Native Categories
//!
//! **Category A: Real implementations (from move-stdlib-natives)**
//! - vector::*, bcs::to_bytes, hash::{sha2_256, sha3_256}
//! - string::*, type_name::*, debug::*, signer::*
//!
//! **Category B: Safe mocks (return valid placeholder values)**
//! - tx_context::* - All return valid placeholder values
//! - object::{delete_impl, record_new_uid, borrow_uid} - No-op or passthrough
//! - transfer::* - No-op (we don't track ownership)
//! - event::emit - No-op (we don't track events)
//! - hash::{blake2b256, keccak256} - Return zeros (valid 32-byte output)
//! - types::is_one_time_witness - Real check (one bool field + UPPERCASE module name)
//!
//! **Category C: Abort stubs (E_NOT_SUPPORTED = 1000)**
//! These would produce false positives if mocked, so they abort explicitly:
//! - dynamic_field::* - Requires object storage we don't have
//! - Crypto verification (bls12381, ecdsa_*, ed25519, groth16, etc.)
//! - zklogin, config, random - Requires runtime state we don't have

use move_binary_format::errors::PartialVMResult;
use move_core_types::{
    account_address::AccountAddress, 
    gas_algebra::InternalGas,
    language_storage::TypeTag,
    runtime_value::{MoveStructLayout, MoveTypeLayout},
};
use move_vm_runtime::native_functions::{make_table_from_iter, NativeFunction, NativeFunctionTable};
use move_vm_types::{
    loaded_data::runtime_types::Type, natives::function::NativeResult, pop_arg, values::Value,
};
use smallvec::smallvec;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::benchmark::object_store::ObjectStore;

const MOVE_STDLIB_ADDRESS: AccountAddress = AccountAddress::ONE;
const SUI_FRAMEWORK_ADDRESS: AccountAddress = AccountAddress::TWO;
const SUI_SYSTEM_ADDRESS: AccountAddress = AccountAddress::new([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3,
]);

/// Error code for operations that cannot be mocked without false positives
const E_NOT_SUPPORTED: u64 = 1000;

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

/// Mock state for native function execution.
pub struct MockNativeState {
    pub sender: AccountAddress,
    pub epoch: u64,
    pub epoch_timestamp_ms: u64,
    ids_created: AtomicU64,
    /// In-memory object store for dynamic field simulation
    pub object_store: ObjectStore,
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
            epoch_timestamp_ms: 0,
            ids_created: AtomicU64::new(0),
            object_store: ObjectStore::new(),
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
    
    /// Clear the object store (for test isolation between runs)
    pub fn clear_objects(&self) {
        self.object_store.clear();
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
        make_native(|_ctx, _ty_args, args| {
            // Pass through the argument - this is a reference operation
            let obj = args.into_iter().next().unwrap();
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![obj]))
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

    // transfer natives - all no-op since we don't track ownership
    // Native names must match the bytecode: freeze_object_impl, share_object_impl, etc.
    natives.push((
        "transfer",
        "transfer_impl",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    natives.push((
        "transfer",
        "freeze_object_impl",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    natives.push((
        "transfer",
        "share_object_impl",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    natives.push((
        "transfer",
        "receive_impl",
        make_native(|_ctx, _ty_args, _args| {
            // Cannot receive objects without storage - abort
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    natives.push((
        "transfer",
        "party_transfer_impl",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    // event natives
    natives.push((
        "event",
        "emit",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    natives.push((
        "event",
        "emit_authenticated_impl",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    natives.push((
        "event",
        "events_by_type",
        make_native(|_ctx, _ty_args, _args| {
            // Return empty - we don't track events
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(vec![])],
            ))
        }),
    ));

    natives.push((
        "event",
        "num_events",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u64(0)],
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
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::u256(move_core_types::u256::U256::from_le_bytes(
                    &bytes.try_into().unwrap()
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

    // hash natives - return valid 32-byte outputs (zeros)
    // These are safe because the output is still valid type-wise
    natives.push((
        "hash",
        "blake2b256",
        make_native(|_ctx, _ty_args, mut args| {
            let _data = pop_arg!(args, Vec<u8>);
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(vec![0u8; 32])],
            ))
        }),
    ));

    natives.push((
        "hash",
        "keccak256",
        make_native(|_ctx, _ty_args, mut args| {
            let _data = pop_arg!(args, Vec<u8>);
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::vector_u8(vec![0u8; 32])],
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
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    natives.push((
        "accumulator",
        "emit_withdraw_event",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    natives.push((
        "accumulator_settlement",
        "record_settlement_sui_conservation",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    ));

    // ============================================================
    // CATEGORY B+: DYNAMIC FIELD SUPPORT (partial)
    // ============================================================
    add_dynamic_field_natives(&mut natives, state);

    // ============================================================
    // CATEGORY C: ABORT STUBS - would produce false positives if mocked
    // ============================================================
    add_abort_stubs(&mut natives);

    natives
}

/// Build Sui system native functions (at 0x3)
fn build_sui_system_natives() -> Vec<(&'static str, &'static str, NativeFunction)> {
    vec![(
        "validator",
        "validate_metadata_bcs",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
        }),
    )]
}

/// Add dynamic field natives that we CAN support.
/// 
/// We support:
/// - hash_type_and_key: deterministic hashing of parent + key type + key value
/// - add_child_object: store object in our in-memory store
/// - has_child_object: check if object exists
/// - has_child_object_with_ty: check if object exists with specific type
///
/// We do NOT support (see add_abort_stubs):
/// - borrow_child_object / borrow_child_object_mut: requires VM extension for references
/// - remove_child_object: requires VM extension for move_from semantics
fn add_dynamic_field_natives(
    natives: &mut Vec<(&'static str, &'static str, NativeFunction)>,
    state: Arc<MockNativeState>,
) {
    use sha2::{Sha256, Digest};
    
    // hash_type_and_key<K>(parent: address, k: K) -> address
    // Deterministically derives child ID from parent + key type + key value
    natives.push((
        "dynamic_field",
        "hash_type_and_key",
        make_native(|ctx, mut ty_args, mut args| {
            // ty_args[0] = K (key type)
            // args[0] = parent: address
            // args[1] = k: K (key value)
            
            let key_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH
                )
            })?;
            let key_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH
                )
            })?;
            let parent = pop_arg!(args, AccountAddress);
            
            // Get type tag for hashing
            let key_tag = ctx.type_to_type_tag(&key_ty)?;
            
            // Get key layout and serialize
            let key_layout = match ctx.type_to_type_layout(&key_ty) {
                Ok(Some(layout)) => layout,
                _ => {
                    return Ok(NativeResult::err(InternalGas::new(0), 3)); // E_BCS_SERIALIZATION_FAILURE
                }
            };
            
            let key_bytes = match key_value.typed_serialize(&key_layout) {
                Some(bytes) => bytes,
                None => {
                    return Ok(NativeResult::err(InternalGas::new(0), 3)); // E_BCS_SERIALIZATION_FAILURE
                }
            };
            
            // Derive child ID: SHA256(parent || type_tag_bcs || key_bcs)[0..32]
            let mut hasher = Sha256::new();
            hasher.update(parent.as_ref());
            
            // Serialize type tag
            let type_tag_bytes = bcs::to_bytes(&key_tag).unwrap_or_default();
            hasher.update(&type_tag_bytes);
            hasher.update(&key_bytes);
            
            let hash = hasher.finalize();
            let child_id = AccountAddress::new(hash.into());
            
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::address(child_id)],
            ))
        }),
    ));
    
    // add_child_object<Child: key>(parent: address, child: Child)
    // Stores the child object in our in-memory object store
    let state_for_add = state.clone();
    natives.push((
        "dynamic_field",
        "add_child_object",
        make_native(move |ctx, mut ty_args, mut args| {
            // ty_args[0] = Child type
            // args[0] = parent: address  
            // args[1] = child: Child
            
            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH
                )
            })?;
            let child_value = args.pop_back().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH
                )
            })?;
            let parent = pop_arg!(args, AccountAddress);
            
            // Get type tag for the child
            let child_tag = ctx.type_to_type_tag(&child_ty)?;
            
            // Get layout and serialize child value
            let child_layout = match ctx.type_to_type_layout(&child_ty) {
                Ok(Some(layout)) => layout,
                _ => {
                    return Ok(NativeResult::err(InternalGas::new(0), 3)); // E_BCS_SERIALIZATION_FAILURE
                }
            };
            
            let child_bytes = match child_value.typed_serialize(&child_layout) {
                Some(bytes) => bytes,
                None => {
                    return Ok(NativeResult::err(InternalGas::new(0), 3)); // E_BCS_SERIALIZATION_FAILURE
                }
            };
            
            // Extract child's object ID from the first field (UID.id.bytes)
            // Field<K,V> has structure: id: UID, name: K, value: V
            // UID has structure: id: ID
            // ID has structure: bytes: address
            // So the object ID is at offset 0 in the BCS bytes (first 32 bytes)
            let child_id = if child_bytes.len() >= 32 {
                let mut addr_bytes = [0u8; 32];
                addr_bytes.copy_from_slice(&child_bytes[..32]);
                AccountAddress::new(addr_bytes)
            } else {
                return Ok(NativeResult::err(InternalGas::new(0), 3));
            };
            
            // Store in object store
            match state_for_add.object_store.add_child(parent, child_id, child_bytes, child_tag) {
                Ok(()) => Ok(NativeResult::ok(InternalGas::new(0), smallvec![])),
                Err(_) => Ok(NativeResult::err(InternalGas::new(0), 0)), // EFieldAlreadyExists
            }
        }),
    ));
    
    // has_child_object(parent: address, id: address) -> bool
    let state_for_has = state.clone();
    natives.push((
        "dynamic_field",
        "has_child_object",
        make_native(move |_ctx, _ty_args, mut args| {
            let child_id = pop_arg!(args, AccountAddress);
            let parent = pop_arg!(args, AccountAddress);
            
            let exists = state_for_has.object_store.has_child(parent, child_id);
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(exists)],
            ))
        }),
    ));
    
    // has_child_object_with_ty<Child: key>(parent: address, id: address) -> bool
    let state_for_has_ty = state;
    natives.push((
        "dynamic_field",
        "has_child_object_with_ty",
        make_native(move |ctx, mut ty_args, mut args| {
            let child_ty = ty_args.pop().ok_or_else(|| {
                move_binary_format::errors::PartialVMError::new(
                    move_core_types::vm_status::StatusCode::TYPE_MISMATCH
                )
            })?;
            let child_id = pop_arg!(args, AccountAddress);
            let parent = pop_arg!(args, AccountAddress);
            
            let child_tag = ctx.type_to_type_tag(&child_ty)?;
            let exists = state_for_has_ty.object_store.has_child_with_type(parent, child_id, &child_tag);
            
            Ok(NativeResult::ok(
                InternalGas::new(0),
                smallvec![Value::bool(exists)],
            ))
        }),
    ));
}

/// Add stubs that abort with E_NOT_SUPPORTED for operations that cannot
/// be safely mocked without producing false positives.
fn add_abort_stubs(natives: &mut Vec<(&'static str, &'static str, NativeFunction)>) {
    // dynamic_field operations that REQUIRE returning references abort.
    // We can't safely implement borrow/remove without VM extension support.
    // See add_dynamic_field_natives() for operations we DO support.
    for func in [
        "borrow_child_object",
        "borrow_child_object_mut",
        "remove_child_object",
    ] {
        natives.push((
            "dynamic_field",
            func,
            make_native(|_ctx, _ty_args, _args| {
                Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
            }),
        ));
    }

    // Crypto verification - would silently pass/fail verification
    for func in ["bls12381_min_sig_verify", "bls12381_min_pk_verify"] {
        natives.push((
            "bls12381",
            func,
            make_native(|_ctx, _ty_args, _args| {
                Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
            }),
        ));
    }

    for func in ["secp256k1_ecrecover", "decompress_pubkey", "secp256k1_verify"] {
        natives.push((
            "ecdsa_k1",
            func,
            make_native(|_ctx, _ty_args, _args| {
                Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
            }),
        ));
    }

    for func in ["secp256r1_ecrecover", "secp256r1_verify"] {
        natives.push((
            "ecdsa_r1",
            func,
            make_native(|_ctx, _ty_args, _args| {
                Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
            }),
        ));
    }

    natives.push((
        "ed25519",
        "ed25519_verify",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    natives.push((
        "ecvrf",
        "ecvrf_verify",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    for func in [
        "verify_groth16_proof_internal",
        "prepare_verifying_key_internal",
    ] {
        natives.push((
            "groth16",
            func,
            make_native(|_ctx, _ty_args, _args| {
                Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
            }),
        ));
    }

    natives.push((
        "hmac",
        "hmac_sha3_256",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    for func in [
        "internal_validate",
        "internal_add",
        "internal_sub",
        "internal_mul",
        "internal_div",
        "internal_hash_to",
        "internal_multi_scalar_mul",
        "internal_pairing",
        "internal_convert",
        "internal_sum",
    ] {
        natives.push((
            "group_ops",
            func,
            make_native(|_ctx, _ty_args, _args| {
                Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
            }),
        ));
    }

    natives.push((
        "poseidon",
        "poseidon_bn254",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    for func in ["vdf_verify", "vdf_hash_to_input"] {
        natives.push((
            "vdf",
            func,
            make_native(|_ctx, _ty_args, _args| {
                Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
            }),
        ));
    }

    natives.push((
        "zklogin_verified_id",
        "check_zklogin_id",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    natives.push((
        "zklogin_verified_issuer",
        "check_zklogin_issuer",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    natives.push((
        "nitro_attestation",
        "verify_nitro_attestation",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    natives.push((
        "config",
        "read_setting_impl",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    natives.push((
        "random",
        "random_internal",
        make_native(|_ctx, _ty_args, _args| {
            Ok(NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED))
        }),
    ));

    for func in [
        "add_to_accumulator_address",
        "withdraw_from_accumulator_address",
    ] {
        natives.push((
            "funds_accumulator",
            func,
            make_native(|_ctx, _ty_args, _args| {
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
