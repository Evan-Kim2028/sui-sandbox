//! Encoding/decoding sandbox handlers.
//!
//! Handles encode_bcs, decode_bcs, and validate_type operations.

use crate::benchmark::sandbox::types::SandboxResponse;
use anyhow::Result;
use move_core_types::account_address::AccountAddress;

/// Encode a pure value to BCS bytes.
pub fn encode_pure_value(value: &serde_json::Value, value_type: &str) -> Result<Vec<u8>> {
    use bcs;

    match value_type {
        "u8" => {
            let v: u8 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u64" => {
            let v: u64 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u128" => {
            let v: u128 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "bool" => {
            let v: bool = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "address" => {
            let s: String = serde_json::from_value(value.clone())?;
            let addr = AccountAddress::from_hex_literal(&s)?;
            Ok(bcs::to_bytes(&addr)?)
        }
        "vector<u8>" => {
            // Can be hex string or array of u8
            if let Some(s) = value.as_str() {
                let bytes = hex::decode(s.trim_start_matches("0x"))?;
                Ok(bcs::to_bytes(&bytes)?)
            } else {
                let v: Vec<u8> = serde_json::from_value(value.clone())?;
                Ok(bcs::to_bytes(&v)?)
            }
        }
        "0x1::string::String" | "String" => {
            let s: String = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&s)?)
        }
        _ => Err(anyhow::anyhow!("Cannot encode type: {}", value_type)),
    }
}

/// Encode a value to BCS.
pub fn execute_encode_bcs(
    type_str: &str,
    value: &serde_json::Value,
    _verbose: bool,
) -> SandboxResponse {
    match encode_pure_value(value, type_str) {
        Ok(bytes) => {
            let hex_str = hex::encode::<&[u8]>(&bytes);
            SandboxResponse::success_with_data(serde_json::json!({
                "type": type_str,
                "bytes_hex": hex_str,
                "bytes_len": bytes.len(),
            }))
        }
        Err(e) => SandboxResponse::error(format!("BCS encode failed: {}", e)),
    }
}

/// Decode BCS bytes.
pub fn execute_decode_bcs(type_str: &str, bytes_hex: &str, _verbose: bool) -> SandboxResponse {
    let bytes = match hex::decode(bytes_hex.trim_start_matches("0x")) {
        Ok(b) => b,
        Err(e) => return SandboxResponse::error(format!("Invalid hex: {}", e)),
    };

    // Decode based on type
    let decoded: serde_json::Value = match type_str {
        "bool" => {
            if bytes.is_empty() {
                return SandboxResponse::error("Empty bytes");
            }
            serde_json::json!(bytes[0] != 0)
        }
        "u8" => {
            if bytes.is_empty() {
                return SandboxResponse::error("Empty bytes");
            }
            serde_json::json!(bytes[0])
        }
        "u64" => {
            if bytes.len() < 8 {
                return SandboxResponse::error("Not enough bytes for u64");
            }
            let val = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            serde_json::json!(val)
        }
        "address" => {
            if bytes.len() < 32 {
                return SandboxResponse::error("Not enough bytes for address");
            }
            serde_json::json!(format!("0x{}", hex::encode(&bytes[..32])))
        }
        _ => {
            return SandboxResponse::error(format!("Cannot decode type: {}", type_str));
        }
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "type": type_str,
        "value": decoded,
    }))
}

/// Validate a type string.
pub fn execute_validate_type(type_str: &str, _verbose: bool) -> SandboxResponse {
    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
        Ok(type_tag) => SandboxResponse::success_with_data(serde_json::json!({
            "valid": true,
            "type_str": type_str,
            "parsed": format!("{}", type_tag),
        })),
        Err(e) => SandboxResponse::success_with_data(serde_json::json!({
            "valid": false,
            "type_str": type_str,
            "error": e.to_string(),
        })),
    }
}
