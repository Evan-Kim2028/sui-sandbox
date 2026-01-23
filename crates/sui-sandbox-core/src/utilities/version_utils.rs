//! Version detection and handling utilities.
//!
//! Many Sui protocols implement version-locking patterns where objects store a
//! `package_version` field that must match a constant in the current bytecode.
//! When replaying historical transactions with current bytecode, these checks fail.
//!
//! This module provides utilities for:
//! - Detecting version constants in bytecode
//! - Collecting historical object versions from gRPC responses
//!
//! For patching version fields in objects, see [`super::generic_patcher::GenericObjectPatcher`].

use move_binary_format::file_format::{Bytecode, CompiledModule, SignatureToken};
use std::collections::HashMap;

/// Scan a module's bytecode to find version constants used in comparisons.
///
/// This analyzes how constants are actually used in the bytecode to find version checks,
/// rather than just looking at constant values. Looks for patterns like:
/// - `LdConst` followed by `Eq`/`Neq` (equality comparison with constant)
/// - Constants in the range 1-100 that are used in comparisons
///
/// Returns a map from package address to the highest detected version.
///
/// # Arguments
///
/// * `modules` - Iterator of compiled modules to scan
///
/// # Example
///
/// ```ignore
/// use sui_sandbox_core::utilities::detect_version_constants;
///
/// let versions = detect_version_constants(resolver.compiled_modules());
/// for (pkg_addr, version) in &versions {
///     println!("Package {} has version {}", pkg_addr, version);
/// }
/// ```
pub fn detect_version_constants<'a>(
    modules: impl Iterator<Item = &'a CompiledModule>,
) -> HashMap<String, u64> {
    let mut version_registry: HashMap<String, u64> = HashMap::new();

    for module in modules {
        scan_module_for_versions(module, &mut version_registry);
    }

    version_registry
}

/// Scan a single module for version constants.
///
/// This is the internal implementation used by `detect_version_constants`.
/// It identifies which constants are used in comparison operations and checks
/// if they look like version numbers (U64 values in range 1-100).
fn scan_module_for_versions(module: &CompiledModule, version_registry: &mut HashMap<String, u64>) {
    let package_addr = module.self_id().address().to_hex_literal();

    // First, identify which constant indices are used in comparison operations
    let mut comparison_constants: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

    for func_def in &module.function_defs {
        if let Some(code) = &func_def.code {
            let instructions = &code.code;
            for (i, instr) in instructions.iter().enumerate() {
                // Look for LdConst followed by comparison (Eq, Neq, Lt, Le, Gt, Ge)
                if let Bytecode::LdConst(const_idx) = instr {
                    // Check if next few instructions include a comparison
                    let next_instrs: Vec<_> = instructions.iter().skip(i + 1).take(3).collect();
                    let has_comparison = next_instrs.iter().any(|instr| {
                        matches!(
                            instr,
                            Bytecode::Eq
                                | Bytecode::Neq
                                | Bytecode::Lt
                                | Bytecode::Le
                                | Bytecode::Gt
                                | Bytecode::Ge
                        )
                    });
                    if has_comparison {
                        comparison_constants.insert(const_idx.0 as usize);
                    }
                }
            }
        }
    }

    // Now check which of these constants are likely version numbers
    for const_idx in comparison_constants {
        if const_idx >= module.constant_pool().len() {
            continue;
        }

        let constant = &module.constant_pool()[const_idx];

        // Only look at U64 constants
        if constant.type_ != SignatureToken::U64 {
            continue;
        }

        // Deserialize the U64 value
        if constant.data.len() == 8 {
            let value = u64::from_le_bytes(constant.data[..8].try_into().unwrap());

            // Version numbers are typically small positive integers
            // Using 1-100 range - now we KNOW these are used in comparisons
            if (1..=100).contains(&value) {
                let key = package_addr.clone();
                let existing = version_registry.get(&key).copied().unwrap_or(0);
                // Keep the highest version found (most likely the current version)
                if value > existing {
                    version_registry.insert(key, value);
                }
            }
        }
    }
}

/// Common version field names found in DeFi protocols.
///
/// These are the field names that typically store version numbers:
/// - `package_version`: Used by Cetus, Bluefin, and many other protocols
/// - `value`: Used by Scallop (inside a Version struct)
pub const COMMON_VERSION_FIELDS: &[&str] = &["package_version", "value"];

/// The typical range for version numbers in Sui protocols.
///
/// Most protocols use small positive integers (1-100) for versioning.
/// This range is used for heuristic detection of version constants.
pub const VERSION_NUMBER_RANGE: std::ops::RangeInclusive<u64> = 1..=100;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_range() {
        assert!(VERSION_NUMBER_RANGE.contains(&1));
        assert!(VERSION_NUMBER_RANGE.contains(&50));
        assert!(VERSION_NUMBER_RANGE.contains(&100));
        assert!(!VERSION_NUMBER_RANGE.contains(&0));
        assert!(!VERSION_NUMBER_RANGE.contains(&101));
    }

    #[test]
    fn test_common_version_fields() {
        assert!(COMMON_VERSION_FIELDS.contains(&"package_version"));
        assert!(COMMON_VERSION_FIELDS.contains(&"value"));
    }
}
