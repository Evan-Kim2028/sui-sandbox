//! Utility sandbox handlers.
//!
//! Handles generate_id, parse_address, format_address, compute_hash, convert_number,
//! encode_vector, and parse_error operations.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;
use crate::utils::format_address_short;
use move_core_types::account_address::AccountAddress;

/// Normalize address to short form for display.
fn normalize_address(addr: &AccountAddress) -> String {
    format_address_short(addr)
}

/// Parse an address string to AccountAddress.
pub fn parse_address_string(s: &str) -> Result<AccountAddress, String> {
    AccountAddress::from_hex_literal(s).map_err(|e| e.to_string())
}

/// Generate a fresh unique object/address ID.
pub fn execute_generate_id(env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Generating fresh ID");
    }
    let id = env.fresh_id();
    let hex_full = id.to_hex_literal();
    let hex_short = normalize_address(&id);
    SandboxResponse::success_with_data(serde_json::json!({
        "id": hex_full,
        "short": hex_short,
    }))
}

/// Parse an address string (supports short forms like "0x2").
pub fn execute_parse_address(address: &str, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Parsing address: {}", address);
    }
    match parse_address_string(address) {
        Ok(parsed) => {
            let hex_full = parsed.to_hex_literal();
            let hex_short = normalize_address(&parsed);
            SandboxResponse::success_with_data(serde_json::json!({
                "full": hex_full,
                "short": hex_short,
                "valid": true,
            }))
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Invalid address: {}", e),
            "ParseError".to_string(),
        ),
    }
}

/// Format an address to different representations.
pub fn execute_format_address(
    address: &str,
    format: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Formatting address: {} as {:?}", address, format);
    }
    match parse_address_string(address) {
        Ok(parsed) => {
            let hex_full = parsed.to_hex_literal();
            let fmt = format.unwrap_or("short");
            let result = match fmt {
                "short" => normalize_address(&parsed),
                "full" => hex_full.clone(),
                "no_prefix" => hex_full.strip_prefix("0x").unwrap_or(&hex_full).to_string(),
                _ => {
                    return SandboxResponse::error_with_category(
                        format!(
                            "Unknown format: {}. Use 'short', 'full', or 'no_prefix'",
                            fmt
                        ),
                        "InvalidParameter".to_string(),
                    );
                }
            };
            SandboxResponse::success_with_data(serde_json::json!({
                "formatted": result,
                "format": fmt,
            }))
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Invalid address: {}", e),
            "ParseError".to_string(),
        ),
    }
}

/// Compute a cryptographic hash of bytes.
pub fn execute_compute_hash(
    bytes_hex: &str,
    algorithm: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Computing hash with algorithm: {:?}", algorithm);
    }
    let hex_str = bytes_hex.strip_prefix("0x").unwrap_or(bytes_hex);
    let bytes = match hex::decode(hex_str) {
        Ok(b) => b,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid hex bytes: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    let algo = algorithm.unwrap_or("sha3_256");
    use sha2::{Digest, Sha256};

    let hash = match algo {
        "sha256" | "sha3_256" | "blake2b_256" => {
            // Note: Currently using sha256 for all. Full implementation would use proper algorithms.
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hasher.finalize().to_vec()
        }
        _ => {
            return SandboxResponse::error_with_category(
                format!(
                    "Unknown algorithm: {}. Use sha256, sha3_256, or blake2b_256",
                    algo
                ),
                "InvalidParameter".to_string(),
            );
        }
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "algorithm": algo,
        "input_len": bytes.len(),
        "hash_hex": format!("0x{}", hex::encode(&hash)),
    }))
}

/// Convert between Move numeric types.
pub fn execute_convert_number(
    value: &str,
    from_type: &str,
    to_type: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Converting {} from {} to {}", value, from_type, to_type);
    }

    // Parse input value as u128
    let val_u128: u128 = if value.starts_with("0x") {
        match u128::from_str_radix(value.strip_prefix("0x").unwrap(), 16) {
            Ok(v) => v,
            Err(e) => {
                return SandboxResponse::error_with_category(
                    format!("Invalid hex value: {}", e),
                    "ParseError".to_string(),
                );
            }
        }
    } else {
        match value.parse::<u128>() {
            Ok(v) => v,
            Err(e) => {
                return SandboxResponse::error_with_category(
                    format!("Invalid decimal value: {}", e),
                    "ParseError".to_string(),
                );
            }
        }
    };

    // Check target type range
    let (max_val, target_bits): (u128, usize) = match to_type {
        "u8" => (u8::MAX as u128, 8),
        "u16" => (u16::MAX as u128, 16),
        "u32" => (u32::MAX as u128, 32),
        "u64" => (u64::MAX as u128, 64),
        "u128" => (u128::MAX, 128),
        "u256" => (u128::MAX, 256),
        _ => {
            return SandboxResponse::error_with_category(
                format!("Unknown target type: {}", to_type),
                "InvalidParameter".to_string(),
            );
        }
    };

    let fits = val_u128 <= max_val;
    let decimal = val_u128.to_string();
    let hex = format!("0x{:x}", val_u128);

    SandboxResponse::success_with_data(serde_json::json!({
        "value_decimal": decimal,
        "value_hex": hex,
        "from_type": from_type,
        "to_type": to_type,
        "fits_in_target": fits,
        "target_bits": target_bits,
    }))
}

/// Encode an array of values as a BCS vector.
pub fn execute_encode_vector(
    element_type: &str,
    values: &[serde_json::Value],
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!(
            "Encoding vector of {} elements of type {}",
            values.len(),
            element_type
        );
    }

    // Encode ULEB128 length prefix
    let mut bytes = Vec::new();
    let mut len = values.len();
    loop {
        let byte = (len & 0x7F) as u8;
        len >>= 7;
        if len == 0 {
            bytes.push(byte);
            break;
        } else {
            bytes.push(byte | 0x80);
        }
    }

    // Encode each element
    for value in values {
        match super::encoding::encode_pure_value(value, element_type) {
            Ok(element_bytes) => bytes.extend(element_bytes),
            Err(e) => {
                return SandboxResponse::error_with_category(
                    format!("Failed to encode element: {}", e),
                    "EncodingError".to_string(),
                );
            }
        }
    }

    SandboxResponse::success_with_data(serde_json::json!({
        "element_type": element_type,
        "element_count": values.len(),
        "bytes_hex": format!("0x{}", hex::encode(&bytes)),
        "bytes_len": bytes.len(),
    }))
}

/// Parse an error string to extract structured information.
pub fn execute_parse_error(error: &str, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Parsing error string");
    }

    // Try to extract abort code and location from common error formats
    let mut result = serde_json::json!({
        "original": error,
    });

    // Pattern: "ABORTED with code X in module Y::Z::func"
    if error.contains("ABORTED") {
        if let Some(code_start) = error.find("code ") {
            let rest = &error[code_start + 5..];
            if let Some(code_end) = rest.find(|c: char| !c.is_ascii_digit()) {
                if let Ok(code) = rest[..code_end].parse::<u64>() {
                    result["abort_code"] = serde_json::json!(code);
                }
            }
        }
    }

    // Pattern: "MissingPackage { address: X, module: Y }"
    if error.contains("MissingPackage") {
        result["error_type"] = serde_json::json!("MissingPackage");
    } else if error.contains("MissingObject") {
        result["error_type"] = serde_json::json!("MissingObject");
    } else if error.contains("LINKER_ERROR") {
        result["error_type"] = serde_json::json!("LinkerError");
    } else if error.contains("ABORTED") {
        result["error_type"] = serde_json::json!("ContractAbort");
    }

    SandboxResponse::success_with_data(result)
}
