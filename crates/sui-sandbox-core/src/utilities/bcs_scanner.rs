//! BCS Data Scanner for Embedded Addresses
//!
//! This module provides utilities for scanning BCS-serialized object data
//! to extract embedded addresses. This is essential for discovering:
//!
//! 1. **Dynamically referenced packages** - Package addresses stored in object fields
//!    that are called at runtime (e.g., callback contracts, routers)
//!
//! 2. **Cross-references** - Object IDs stored in one object that reference another
//!
//! 3. **Configuration addresses** - Admin addresses, fee recipients, etc.
//!
//! ## The Problem
//!
//! Some packages are not discoverable through static bytecode analysis because:
//! - They're stored as `address` fields in objects
//! - They're loaded at runtime via `borrow_global` or similar
//! - They're passed through dynamic dispatch patterns
//!
//! ## Solution
//!
//! Scan raw BCS bytes for patterns that look like 32-byte Sui addresses.
//! This is a heuristic approach that may have false positives, but catches
//! real addresses that would otherwise be missed.
//!
//! ## Usage
//!
//! ```ignore
//! use sui_sandbox_core::utilities::BcsAddressScanner;
//!
//! let scanner = BcsAddressScanner::new();
//! let addresses = scanner.scan_for_addresses(&bcs_bytes);
//!
//! // Filter to likely package addresses (high entropy, non-zero)
//! let likely_packages: Vec<_> = addresses
//!     .into_iter()
//!     .filter(|addr| scanner.looks_like_package(addr))
//!     .collect();
//! ```

use std::collections::HashSet;

/// Scanner for extracting embedded addresses from BCS-serialized data.
#[derive(Debug, Clone)]
pub struct BcsAddressScanner {
    /// Minimum entropy threshold for an address to be considered "real"
    /// (filters out addresses that are mostly zeros or repetitive)
    min_entropy: f64,
    /// Whether to include framework addresses (0x1, 0x2, 0x3)
    include_framework: bool,
    /// Known addresses to skip (e.g., already-fetched packages)
    skip_addresses: HashSet<String>,
}

impl Default for BcsAddressScanner {
    fn default() -> Self {
        Self {
            min_entropy: 2.0, // Reasonable default - filters out very low entropy
            include_framework: false,
            skip_addresses: HashSet::new(),
        }
    }
}

impl BcsAddressScanner {
    /// Create a new scanner with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set minimum entropy threshold.
    pub fn with_min_entropy(mut self, entropy: f64) -> Self {
        self.min_entropy = entropy;
        self
    }

    /// Include framework addresses in results.
    pub fn with_framework(mut self, include: bool) -> Self {
        self.include_framework = include;
        self
    }

    /// Add addresses to skip.
    pub fn with_skip_addresses(mut self, addresses: HashSet<String>) -> Self {
        self.skip_addresses = addresses;
        self
    }

    /// Scan BCS bytes for potential 32-byte addresses.
    ///
    /// Returns a list of discovered addresses as hex strings.
    pub fn scan_for_addresses(&self, bcs_bytes: &[u8]) -> Vec<String> {
        let mut addresses = Vec::new();

        // Scan for 32-byte aligned sequences
        if bcs_bytes.len() < 32 {
            return addresses;
        }

        // Slide through the bytes looking for potential addresses
        // We check at every byte position since addresses may not be aligned
        for i in 0..=(bcs_bytes.len() - 32) {
            let potential_addr = &bcs_bytes[i..i + 32];

            // Skip if it's all zeros
            if potential_addr.iter().all(|&b| b == 0) {
                continue;
            }

            // Calculate entropy
            let entropy = self.byte_entropy(potential_addr);
            if entropy < self.min_entropy {
                continue;
            }

            // Convert to hex string
            let hex = format!("0x{}", hex::encode(potential_addr));

            // Skip framework if configured
            if !self.include_framework && is_framework_address(&hex) {
                continue;
            }

            // Skip known addresses
            let normalized = super::normalize_address(&hex);
            if self.skip_addresses.contains(&normalized) {
                continue;
            }

            addresses.push(normalized);
        }

        // Deduplicate
        let unique: HashSet<_> = addresses.into_iter().collect();
        unique.into_iter().collect()
    }

    /// Scan multiple BCS objects and aggregate results.
    pub fn scan_multiple(&self, objects: &[&[u8]]) -> Vec<String> {
        let mut all_addresses = HashSet::new();

        for bcs in objects {
            for addr in self.scan_for_addresses(bcs) {
                all_addresses.insert(addr);
            }
        }

        all_addresses.into_iter().collect()
    }

    /// Check if an address looks like it could be a package.
    ///
    /// Packages tend to have:
    /// - High entropy (not simple/derived)
    /// - Non-zero prefix bytes
    /// - Not a well-known system address
    pub fn looks_like_package(&self, addr: &str) -> bool {
        let bytes = match hex_to_bytes(addr) {
            Some(b) => b,
            None => return false,
        };

        // Must be 32 bytes
        if bytes.len() != 32 {
            return false;
        }

        // Skip framework
        if is_framework_address(addr) {
            return false;
        }

        // Check entropy
        let entropy = self.byte_entropy(&bytes);
        entropy >= self.min_entropy
    }

    /// Calculate Shannon entropy of a byte sequence.
    /// Higher entropy = more random/unique, lower = more repetitive.
    fn byte_entropy(&self, bytes: &[u8]) -> f64 {
        let mut counts = [0u64; 256];
        for &b in bytes {
            counts[b as usize] += 1;
        }

        let len = bytes.len() as f64;
        let mut entropy = 0.0;

        for &count in &counts {
            if count > 0 {
                let p = count as f64 / len;
                entropy -= p * p.log2();
            }
        }

        entropy
    }
}

/// Extract addresses from a type string's type parameters.
///
/// Type parameters often contain package addresses in nested types.
/// e.g., `Pool<0xabc::coin::COIN, 0xdef::token::TOKEN>`
pub fn extract_addresses_from_type_params(type_str: &str) -> Vec<String> {
    let mut addresses = HashSet::new();

    // Find all hex addresses in the type string
    let chars: Vec<char> = type_str.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Look for 0x prefix
        if i + 2 < chars.len() && chars[i] == '0' && chars[i + 1] == 'x' {
            let start = i;
            i += 2;

            // Consume hex digits
            while i < chars.len() && chars[i].is_ascii_hexdigit() {
                i += 1;
            }

            let addr: String = chars[start..i].iter().collect();
            if addr.len() > 4 {
                // Must be more than just "0x"
                let normalized = super::normalize_address(&addr);
                if !is_framework_address(&normalized) {
                    addresses.insert(normalized);
                }
            }
        } else {
            i += 1;
        }
    }

    addresses.into_iter().collect()
}

/// Extract addresses from bytecode constants.
///
/// Move bytecode can contain hardcoded addresses as constants.
/// This scans the constant pool for 32-byte values.
pub fn extract_addresses_from_bytecode_constants(bytecode: &[u8]) -> Vec<String> {
    use move_binary_format::CompiledModule;

    let module = match CompiledModule::deserialize_with_defaults(bytecode) {
        Ok(m) => m,
        Err(_) => return vec![],
    };

    let mut addresses = HashSet::new();
    let scanner = BcsAddressScanner::new();

    // Check constant pool
    for constant in &module.constant_pool {
        // Constants are BCS-encoded, so we can scan them
        let found = scanner.scan_for_addresses(&constant.data);
        for addr in found {
            addresses.insert(addr);
        }
    }

    // Also check module handles for referenced packages
    for handle in &module.module_handles {
        let addr = module.address_identifier_at(handle.address);
        let hex = format!("0x{}", addr.to_hex());
        let normalized = super::normalize_address(&hex);
        if !is_framework_address(&normalized) {
            addresses.insert(normalized);
        }
    }

    addresses.into_iter().collect()
}

/// Check if an address is a framework address (0x1, 0x2, 0x3).
fn is_framework_address(addr: &str) -> bool {
    sui_resolver::is_framework_address(addr)
}

/// Convert hex string to bytes.
fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    hex::decode(hex).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_entropy() {
        let scanner = BcsAddressScanner::new();

        // All zeros = 0 entropy
        let zeros = [0u8; 32];
        assert_eq!(scanner.byte_entropy(&zeros), 0.0);

        // All same value = 0 entropy
        let same = [0xAB; 32];
        assert_eq!(scanner.byte_entropy(&same), 0.0);

        // Alternating = 1 bit entropy
        let alternating: Vec<u8> = (0..32).map(|i| if i % 2 == 0 { 0 } else { 1 }).collect();
        assert!(scanner.byte_entropy(&alternating) > 0.9);
        assert!(scanner.byte_entropy(&alternating) < 1.1);

        // Random-ish = high entropy
        let random: Vec<u8> = (0..32).map(|i| (i * 7 + 13) as u8).collect();
        assert!(scanner.byte_entropy(&random) > 3.0);
    }

    #[test]
    fn test_framework_detection() {
        assert!(is_framework_address("0x1"));
        assert!(is_framework_address("0x2"));
        assert!(is_framework_address("0x3"));
        assert!(is_framework_address(
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        ));
        assert!(!is_framework_address("0x4"));
        assert!(!is_framework_address(
            "0xabc123def456789012345678901234567890123456789012345678901234"
        ));
    }

    #[test]
    fn test_scan_for_addresses() {
        let scanner = BcsAddressScanner::new();

        // Create bytes with an embedded address
        let mut data = vec![0u8; 100];
        // Put a "random" looking address at offset 20
        for i in 0..32 {
            data[20 + i] = ((i * 17 + 5) % 256) as u8;
        }

        let found = scanner.scan_for_addresses(&data);
        assert!(!found.is_empty());
    }

    #[test]
    fn test_extract_from_type_params() {
        let type_str = "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809::pool::Pool<0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC>";

        let addresses = extract_addresses_from_type_params(type_str);
        assert!(addresses.len() >= 2);
    }
}
