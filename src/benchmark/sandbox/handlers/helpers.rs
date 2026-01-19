//! Shared helper macros and functions for sandbox handlers.
//!
//! This module consolidates common patterns to reduce boilerplate across handlers:
//! - Address/identifier parsing with error handling
//! - Base64 decoding for module bytecode
//! - Object formatting for JSON responses
//! - Verbose logging

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulatedObject;
use crate::benchmark::types::format_type_tag;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;

// =============================================================================
// Parsing Helpers
// =============================================================================

/// Parse an address from hex string, returning a SandboxResponse error on failure.
/// Use this instead of manually matching on `AccountAddress::from_hex_literal`.
#[macro_export]
macro_rules! parse_address_or_return {
    ($value:expr, $context:expr) => {
        match move_core_types::account_address::AccountAddress::from_hex_literal($value) {
            Ok(a) => a,
            Err(e) => {
                return $crate::benchmark::sandbox::types::SandboxResponse::error(format!(
                    "Invalid {}: {}",
                    $context, e
                ))
            }
        }
    };
}

/// Parse an identifier from string, returning a SandboxResponse error on failure.
#[macro_export]
macro_rules! parse_identifier_or_return {
    ($value:expr, $context:expr) => {
        match move_core_types::identifier::Identifier::new($value) {
            Ok(id) => id,
            Err(e) => {
                return $crate::benchmark::sandbox::types::SandboxResponse::error(format!(
                    "Invalid {}: {}",
                    $context, e
                ))
            }
        }
    };
}

/// Parse an address, collecting errors into a vector instead of returning early.
/// For use in validation contexts.
pub fn validate_address(
    value: &str,
    context: &str,
    errors: &mut Vec<String>,
) -> Option<AccountAddress> {
    match AccountAddress::from_hex_literal(value) {
        Ok(a) => Some(a),
        Err(e) => {
            errors.push(format!("Invalid {}: {}", context, e));
            None
        }
    }
}

/// Parse an identifier, collecting errors into a vector instead of returning early.
pub fn validate_identifier(
    value: &str,
    context: &str,
    errors: &mut Vec<String>,
) -> Option<Identifier> {
    match Identifier::new(value) {
        Ok(id) => Some(id),
        Err(e) => {
            errors.push(format!("Invalid {}: {}", context, e));
            None
        }
    }
}

// =============================================================================
// Base64 Decoding
// =============================================================================

/// Decode a list of base64-encoded module bytecodes.
/// Returns Ok with decoded bytes, or Err with error message.
pub fn decode_base64_modules(modules: &[String]) -> Result<Vec<Vec<u8>>, String> {
    use base64::Engine;
    let mut decoded = Vec::with_capacity(modules.len());
    for (i, b64) in modules.iter().enumerate() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(bytes) => decoded.push(bytes),
            Err(e) => {
                return Err(format!("Invalid base64 in module {}: {}", i, e));
            }
        }
    }
    Ok(decoded)
}

// =============================================================================
// Object Formatting
// =============================================================================

/// Get ownership string for an object.
#[inline]
pub fn ownership_string(is_shared: bool, is_immutable: bool) -> &'static str {
    match (is_shared, is_immutable) {
        (true, _) => "shared",
        (false, true) => "immutable",
        _ => "owned",
    }
}

/// Format an object as a JSON summary for list responses.
pub fn format_object_summary(obj: &SimulatedObject) -> serde_json::Value {
    serde_json::json!({
        "object_id": obj.id.to_hex_literal(),
        "type": format_type_tag(&obj.type_tag),
        "ownership": ownership_string(obj.is_shared, obj.is_immutable),
        "version": obj.version,
        "bcs_bytes_len": obj.bcs_bytes.len(),
    })
}

/// Format an object as a minimal JSON summary (for shared objects list).
pub fn format_object_minimal(obj: &SimulatedObject) -> serde_json::Value {
    serde_json::json!({
        "object_id": obj.id.to_hex_literal(),
        "type": format_type_tag(&obj.type_tag),
        "version": obj.version,
    })
}

// =============================================================================
// Verbose Logging
// =============================================================================

/// Conditional verbose logging macro.
#[macro_export]
macro_rules! verbose_log {
    ($verbose:expr, $($arg:tt)*) => {
        if $verbose {
            eprintln!($($arg)*);
        }
    };
}

// =============================================================================
// Response Helpers
// =============================================================================

/// Create a success response with object import details.
pub fn object_import_response(
    id: &AccountAddress,
    type_string: Option<&str>,
    is_shared: bool,
    is_immutable: bool,
    source: &str,
    network: &str,
) -> SandboxResponse {
    SandboxResponse::success_with_data(serde_json::json!({
        "object_id": id.to_hex_literal(),
        "type": type_string,
        "is_shared": is_shared,
        "is_immutable": is_immutable,
        "source": source,
        "network": network,
    }))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ownership_string() {
        assert_eq!(ownership_string(true, false), "shared");
        assert_eq!(ownership_string(true, true), "shared");
        assert_eq!(ownership_string(false, true), "immutable");
        assert_eq!(ownership_string(false, false), "owned");
    }

    #[test]
    fn test_decode_base64_modules() {
        // Valid base64
        let modules = vec!["SGVsbG8gV29ybGQ=".to_string()]; // "Hello World"
        let decoded = decode_base64_modules(&modules).unwrap();
        assert_eq!(decoded[0], b"Hello World");

        // Invalid base64
        let invalid = vec!["not valid base64!!!".to_string()];
        assert!(decode_base64_modules(&invalid).is_err());
    }

    #[test]
    fn test_validate_address() {
        let mut errors = Vec::new();

        // Valid address
        let addr = validate_address("0x2", "test", &mut errors);
        assert!(addr.is_some());
        assert!(errors.is_empty());

        // Invalid address
        let addr = validate_address("invalid", "test", &mut errors);
        assert!(addr.is_none());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_validate_identifier() {
        let mut errors = Vec::new();

        // Valid identifier
        let id = validate_identifier("my_module", "module", &mut errors);
        assert!(id.is_some());
        assert!(errors.is_empty());

        // Invalid identifier (starts with number)
        let id = validate_identifier("123invalid", "module", &mut errors);
        assert!(id.is_none());
        assert_eq!(errors.len(), 1);
    }
}
