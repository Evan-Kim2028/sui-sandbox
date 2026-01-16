//! Object management sandbox handlers.
//!
//! Handles create_object, inspect_object, list_objects, list_shared_objects,
//! get_shared_object_info, and list_shared_locks operations.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;
use crate::benchmark::types::format_type_tag;
use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::HashMap;

use super::utils::parse_address_string;

/// Parse an object ID from hex string.
fn parse_object_id(id_str: &str) -> Result<[u8; 32]> {
    let addr = AccountAddress::from_hex_literal(id_str)
        .map_err(|e| anyhow::anyhow!("Invalid hex: {}", e))?;
    Ok(addr.into_bytes())
}

/// Create an object with specific field values.
pub fn execute_create_object(
    env: &mut SimulationEnvironment,
    object_type: &str,
    fields: &HashMap<String, serde_json::Value>,
    object_id: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Creating object of type: {}", object_type);
        eprintln!("Fields: {:?}", fields);
    }

    // Parse object ID if provided
    let id = if let Some(id_str) = object_id {
        match parse_object_id(id_str) {
            Ok(id) => Some(id),
            Err(e) => return SandboxResponse::error(format!("Invalid object ID: {}", e)),
        }
    } else {
        None
    };

    // Create the object in the environment
    match env.create_object_from_json(object_type, fields, id) {
        Ok(created_id) => SandboxResponse::success_with_data(serde_json::json!({
            "object_id": created_id.to_hex_literal(),
            "type": object_type,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to create object: {}", e),
            "ObjectCreationError",
        ),
    }
}

/// Inspect an object's current state (decode BCS to readable fields).
pub fn execute_inspect_object(
    env: &SimulationEnvironment,
    object_id: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Inspecting object: {}", object_id);
    }

    let addr = match AccountAddress::from_hex_literal(object_id) {
        Ok(a) => a,
        Err(e) => return SandboxResponse::error(format!("Invalid object ID: {}", e)),
    };

    let obj = match env.get_object(&addr) {
        Some(o) => o,
        None => return SandboxResponse::error(format!("Object {} not found", object_id)),
    };

    // Try to decode the BCS bytes based on the type
    let decoded_fields = decode_object_state(env, obj);

    let ownership = if obj.is_shared {
        "shared"
    } else if obj.is_immutable {
        "immutable"
    } else {
        "owned"
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "object_id": object_id,
        "type": format_type_tag(&obj.type_tag),
        "ownership": ownership,
        "version": obj.version,
        "bcs_bytes_hex": hex::encode(&obj.bcs_bytes),
        "bcs_bytes_len": obj.bcs_bytes.len(),
        "decoded": decoded_fields,
    }))
}

/// Decode object BCS state into readable JSON.
fn decode_object_state(
    env: &SimulationEnvironment,
    obj: &crate::benchmark::simulation::SimulatedObject,
) -> serde_json::Value {
    // Handle common types specially
    match &obj.type_tag {
        TypeTag::Struct(st) => {
            let type_str = format!(
                "{}::{}::{}",
                st.address.to_hex_literal(),
                st.module,
                st.name
            );

            // Handle Coin<T> specially
            if st.address.to_hex_literal() == "0x2"
                && st.module.as_str() == "coin"
                && st.name.as_str() == "Coin"
            {
                return decode_coin(&obj.bcs_bytes, &st.type_params);
            }

            // Handle Balance<T> specially
            if st.address.to_hex_literal() == "0x2"
                && st.module.as_str() == "balance"
                && st.name.as_str() == "Balance"
            {
                return decode_balance(&obj.bcs_bytes);
            }

            // Try to decode using struct definition from loaded modules
            if let Ok(defs) = env.get_struct_definitions(
                &st.address.to_hex_literal(),
                Some(st.module.as_str()),
                Some(st.name.as_str()),
            ) {
                if let Some(def) = defs.first() {
                    return decode_struct_with_definition(&obj.bcs_bytes, def);
                }
            }

            // Fallback: return raw info
            serde_json::json!({
                "type": type_str,
                "raw_hex": hex::encode(&obj.bcs_bytes),
                "note": "Could not decode struct fields - type definition not loaded"
            })
        }
        _ => {
            // For non-struct types, return raw
            serde_json::json!({
                "type": format_type_tag(&obj.type_tag),
                "raw_hex": hex::encode(&obj.bcs_bytes),
            })
        }
    }
}

/// Decode a Coin<T> object.
fn decode_coin(bcs_bytes: &[u8], type_params: &[TypeTag]) -> serde_json::Value {
    // Coin<T> = { id: UID (32 bytes), balance: Balance<T> (8 bytes) }
    if bcs_bytes.len() < 40 {
        return serde_json::json!({
            "error": "Invalid Coin: too few bytes",
            "raw_hex": hex::encode(bcs_bytes),
        });
    }

    let id_bytes = &bcs_bytes[0..32];
    let balance_bytes = &bcs_bytes[32..40];
    let balance = u64::from_le_bytes(balance_bytes.try_into().unwrap_or([0; 8]));

    let coin_type = type_params
        .first()
        .map(|t| format!("{}", t))
        .unwrap_or_else(|| "unknown".to_string());

    serde_json::json!({
        "type": format!("0x2::coin::Coin<{}>", coin_type),
        "id": format!("0x{}", hex::encode(id_bytes)),
        "balance": balance,
        "coin_type": coin_type,
    })
}

/// Decode a Balance<T> object.
fn decode_balance(bcs_bytes: &[u8]) -> serde_json::Value {
    // Balance<T> = { value: u64 (8 bytes) }
    if bcs_bytes.len() < 8 {
        return serde_json::json!({
            "error": "Invalid Balance: too few bytes",
            "raw_hex": hex::encode(bcs_bytes),
        });
    }

    let value = u64::from_le_bytes(bcs_bytes[0..8].try_into().unwrap_or([0; 8]));

    serde_json::json!({
        "type": "0x2::balance::Balance",
        "value": value,
    })
}

/// Decode a struct using its definition.
fn decode_struct_with_definition(
    bcs_bytes: &[u8],
    def: &crate::benchmark::simulation::StructDefinition,
) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    let mut offset = 0;

    for field in &def.fields {
        let (value, consumed) = decode_field_value(&bcs_bytes[offset..], &field.field_type);
        fields.insert(field.name.clone(), value);
        offset += consumed;
        if offset >= bcs_bytes.len() {
            break;
        }
    }

    serde_json::json!({
        "type": format!("{}::{}::{}", def.package, def.module, def.name),
        "fields": fields,
    })
}

/// Decode a single field value from BCS bytes.
/// Returns (decoded_value, bytes_consumed).
fn decode_field_value(bytes: &[u8], field_type: &str) -> (serde_json::Value, usize) {
    if bytes.is_empty() {
        return (serde_json::json!(null), 0);
    }

    match field_type {
        "u8" => {
            if bytes.is_empty() {
                (serde_json::json!(null), 0)
            } else {
                (serde_json::json!(bytes[0]), 1)
            }
        }
        "u16" => {
            if let Some(arr) = bytes.get(0..2).and_then(|s| <[u8; 2]>::try_from(s).ok()) {
                (serde_json::json!(u16::from_le_bytes(arr)), 2)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "u32" => {
            if let Some(arr) = bytes.get(0..4).and_then(|s| <[u8; 4]>::try_from(s).ok()) {
                (serde_json::json!(u32::from_le_bytes(arr)), 4)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "u64" => {
            if let Some(arr) = bytes.get(0..8).and_then(|s| <[u8; 8]>::try_from(s).ok()) {
                (serde_json::json!(u64::from_le_bytes(arr)), 8)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "u128" => {
            if let Some(arr) = bytes.get(0..16).and_then(|s| <[u8; 16]>::try_from(s).ok()) {
                (serde_json::json!(u128::from_le_bytes(arr).to_string()), 16)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "u256" => {
            if let Some(slice) = bytes.get(0..32) {
                (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "bool" => {
            if bytes.is_empty() {
                (serde_json::json!(null), 0)
            } else {
                (serde_json::json!(bytes[0] != 0), 1)
            }
        }
        "address" => {
            if let Some(slice) = bytes.get(0..32) {
                (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        // UID is { id: { bytes: address } }
        "0x2::object::UID" | "UID" => {
            if let Some(slice) = bytes.get(0..32) {
                (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        // ID is { bytes: address }
        "0x2::object::ID" | "ID" => {
            if let Some(slice) = bytes.get(0..32) {
                (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        // String/vector<u8> - ULEB128 length prefix
        t if t.starts_with("vector<u8>") || t == "0x1::string::String" || t == "String" => {
            decode_vector_u8(bytes)
        }
        // Generic vector - ULEB128 length prefix
        t if t.starts_with("vector<") => {
            // For now, return raw hex for complex vectors
            (
                serde_json::json!({
                    "raw_hex": hex::encode(bytes),
                    "note": format!("Cannot fully decode {}", t)
                }),
                bytes.len(),
            )
        }
        // Option<T> - 0 for None, 1 + value for Some
        t if t.starts_with("0x1::option::Option<") => {
            if bytes.is_empty() {
                (serde_json::json!(null), 0)
            } else if bytes[0] == 0 {
                (serde_json::json!(null), 1)
            } else {
                // Extract inner type and decode
                let inner = t
                    .strip_prefix("0x1::option::Option<")
                    .and_then(|s| s.strip_suffix(">"))
                    .unwrap_or("unknown");
                let (inner_val, consumed) = decode_field_value(&bytes[1..], inner);
                (inner_val, 1 + consumed)
            }
        }
        // Unknown type - return raw hex
        _ => (
            serde_json::json!({
                "raw_hex": hex::encode(bytes),
                "type": field_type,
            }),
            bytes.len(),
        ),
    }
}

/// Decode a vector<u8> or String from BCS bytes.
fn decode_vector_u8(bytes: &[u8]) -> (serde_json::Value, usize) {
    // ULEB128 length prefix
    let mut offset = 0;
    let mut len: usize = 0;
    let mut shift = 0;

    while offset < bytes.len() {
        let byte = bytes[offset];
        len |= ((byte & 0x7f) as usize) << shift;
        offset += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }

    if offset + len > bytes.len() {
        return (
            serde_json::json!({
                "error": "Invalid vector: length exceeds available bytes",
                "raw_hex": hex::encode(bytes),
            }),
            bytes.len(),
        );
    }

    let data = &bytes[offset..offset + len];

    // Try to interpret as UTF-8 string
    if let Ok(s) = std::str::from_utf8(data) {
        (serde_json::json!(s), offset + len)
    } else {
        (
            serde_json::json!(format!("0x{}", hex::encode(data))),
            offset + len,
        )
    }
}

/// List all objects in the sandbox with their types.
pub fn execute_list_objects(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing all objects");
    }

    let objects: Vec<serde_json::Value> = env
        .list_objects()
        .into_iter()
        .map(|obj| {
            let ownership = if obj.is_shared {
                "shared"
            } else if obj.is_immutable {
                "immutable"
            } else {
                "owned"
            };

            serde_json::json!({
                "object_id": obj.id.to_hex_literal(),
                "type": format_type_tag(&obj.type_tag),
                "ownership": ownership,
                "version": obj.version,
                "bcs_bytes_len": obj.bcs_bytes.len(),
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "objects": objects,
        "count": objects.len(),
    }))
}

/// List all shared objects and their current lock status.
pub fn execute_list_shared_objects(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing shared objects and locks");
    }

    // Get all shared objects from the environment
    let shared_objects: Vec<serde_json::Value> = env
        .list_objects()
        .into_iter()
        .filter(|obj| obj.is_shared)
        .map(|obj| {
            serde_json::json!({
                "object_id": obj.id.to_hex_literal(),
                "type": format_type_tag(&obj.type_tag),
                "version": obj.version,
            })
        })
        .collect();

    // Get current locks
    let locks: Vec<serde_json::Value> = env
        .get_shared_locks()
        .into_iter()
        .map(|lock| {
            serde_json::json!({
                "object_id": lock.object_id.to_hex_literal(),
                "version": lock.version,
                "is_mutable": lock.is_mutable,
                "held_by": lock.transaction_id,
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "shared_objects": shared_objects,
        "shared_object_count": shared_objects.len(),
        "active_locks": locks,
        "lock_count": locks.len(),
    }))
}

/// Get detailed information about a shared object including version and lock status.
pub fn execute_get_shared_object_info(
    env: &SimulationEnvironment,
    object_id: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting shared object info for: {}", object_id);
    }

    // Parse object ID
    let id = match parse_address_string(object_id) {
        Ok(addr) => addr,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid object ID: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    // Get the object
    let obj = match env.get_object(&id) {
        Some(o) => o,
        None => {
            return SandboxResponse::error_with_category(
                format!("Object not found: {}", object_id),
                "ObjectNotFound".to_string(),
            );
        }
    };

    // Check if object is shared
    if !obj.is_shared {
        return SandboxResponse::success_with_data(serde_json::json!({
            "object_id": object_id,
            "is_shared": false,
            "version": obj.version,
            "type": format!("{}", obj.type_tag),
            "message": "Object is not shared"
        }));
    }

    // Get lock status for this object
    let lock_info = env.get_lock_for_object(&id);

    SandboxResponse::success_with_data(serde_json::json!({
        "object_id": object_id,
        "is_shared": true,
        "version": obj.version,
        "type": format!("{}", obj.type_tag),
        "is_locked": lock_info.is_some(),
        "lock_info": lock_info.map(|lock| serde_json::json!({
            "version": lock.version,
            "is_mutable": lock.is_mutable,
            "transaction_id": lock.transaction_id
        })),
        "lamport_clock": env.lamport_clock()
    }))
}

/// List all currently held shared object locks.
pub fn execute_list_shared_locks(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing shared object locks");
    }

    let locks = env.list_shared_locks();
    let lock_list: Vec<serde_json::Value> = locks
        .iter()
        .map(|lock| {
            serde_json::json!({
                "object_id": lock.object_id.to_hex_literal(),
                "version": lock.version,
                "is_mutable": lock.is_mutable,
                "transaction_id": lock.transaction_id
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "locks": lock_list,
        "count": lock_list.len(),
        "lamport_clock": env.lamport_clock()
    }))
}

/// Create a test object in the sandbox.
pub fn execute_create_test_object(
    env: &mut SimulationEnvironment,
    type_tag: &str,
    value: &serde_json::Value,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Creating test object of type: {}", type_tag);
    }

    match env.create_test_object(type_tag, value.clone()) {
        Ok(object_id) => SandboxResponse::success_with_data(serde_json::json!({
            "object_id": object_id.to_hex_literal(),
            "type": type_tag,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to create test object: {}", e),
            "ObjectCreationError",
        ),
    }
}
