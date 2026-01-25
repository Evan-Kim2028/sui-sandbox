//! Package upgrade resolution - bidirectional mapping between original and upgraded addresses.
//!
//! When a package is upgraded in Sui:
//! - The **original_id** (runtime_id) stays stable - types always reference this
//! - The **storage_id** changes - this is where the actual bytecode lives
//!
//! This module provides a resolver that maintains bidirectional mappings
//! to translate between these addresses.

use std::collections::HashMap;

use crate::address::normalize_address;

/// Resolver for package upgrade mappings.
///
/// Maintains bidirectional maps between original (runtime) IDs and storage IDs.
/// This enables:
/// - Normalizing any address to its original_id (for type comparison)
/// - Finding the current storage_id for fetching bytecode
#[derive(Debug, Default, Clone)]
pub struct PackageUpgradeResolver {
    /// Maps storage_id -> original_id
    storage_to_original: HashMap<String, String>,
    /// Maps original_id -> latest storage_id
    original_to_storage: HashMap<String, String>,
}

impl PackageUpgradeResolver {
    /// Create a new empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a package's address mapping.
    ///
    /// Call this when fetching packages - each package carries both its
    /// storage_id (where it lives) and original_id (stable identifier).
    pub fn register_package(&mut self, storage_id: &str, original_id: &str) {
        let storage_norm = normalize_address(storage_id);
        let original_norm = normalize_address(original_id);

        self.storage_to_original
            .insert(storage_norm.clone(), original_norm.clone());
        self.original_to_storage.insert(original_norm, storage_norm);
    }

    /// Register a linkage upgrade mapping.
    ///
    /// Linkage tables in packages tell us about dependency upgrades.
    /// This is another source of upgrade information.
    pub fn register_linkage(&mut self, original_id: &str, upgraded_id: &str) {
        let original_norm = normalize_address(original_id);
        let upgraded_norm = normalize_address(upgraded_id);

        // The upgraded_id is a storage_id that maps to the original_id
        self.storage_to_original
            .insert(upgraded_norm.clone(), original_norm.clone());
        // Update the latest storage for this original
        self.original_to_storage
            .insert(original_norm, upgraded_norm);
    }

    /// Normalize any address to its original_id (stable form).
    ///
    /// If the address is a storage_id of an upgraded package, returns the original_id.
    /// If unknown or already an original_id, returns the normalized input.
    pub fn normalize_to_original(&self, addr: &str) -> String {
        let normalized = normalize_address(addr);

        // Check if this is a known storage_id
        if let Some(original) = self.storage_to_original.get(&normalized) {
            return original.clone();
        }

        // Unknown - return as-is (might already be an original)
        normalized
    }

    /// Get the storage_id for an original_id.
    ///
    /// Returns the latest known storage address for this package.
    /// If unknown, returns the normalized input (package might not be upgraded).
    pub fn get_storage_id(&self, original_id: &str) -> String {
        let normalized = normalize_address(original_id);

        if let Some(storage) = self.original_to_storage.get(&normalized) {
            return storage.clone();
        }

        normalized
    }

    /// Check if an address is a known storage_id (upgraded package address).
    pub fn is_storage_id(&self, addr: &str) -> bool {
        let normalized = normalize_address(addr);
        self.storage_to_original.contains_key(&normalized)
    }

    /// Check if an address is a known original_id.
    pub fn is_original_id(&self, addr: &str) -> bool {
        let normalized = normalize_address(addr);
        self.original_to_storage.contains_key(&normalized)
    }

    /// Get all registered mappings (original_id -> storage_id).
    pub fn all_upgrades(&self) -> &HashMap<String, String> {
        &self.original_to_storage
    }

    /// Get all storage_id -> original_id mappings.
    ///
    /// This is the reverse direction from `all_upgrades()` and is useful
    /// for setting up module resolver aliases where we need to redirect
    /// lookups from storage addresses to bytecode addresses.
    pub fn all_storage_to_original(&self) -> &HashMap<String, String> {
        &self.storage_to_original
    }

    /// Get the number of registered packages.
    pub fn len(&self) -> usize {
        self.original_to_storage.len()
    }

    /// Check if the resolver is empty.
    pub fn is_empty(&self) -> bool {
        self.original_to_storage.is_empty()
    }

    /// Normalize a StructTag address field.
    ///
    /// StructTags in dynamic field keys may use storage_id addresses.
    /// This normalizes them to original_id for consistent comparison.
    pub fn normalize_struct_tag_address(
        &self,
        tag: &move_core_types::language_storage::StructTag,
    ) -> move_core_types::language_storage::StructTag {
        let normalized_addr = self.normalize_to_original(&tag.address.to_hex_literal());
        let new_addr =
            move_core_types::account_address::AccountAddress::from_hex_literal(&normalized_addr)
                .unwrap_or(tag.address);

        move_core_types::language_storage::StructTag {
            address: new_addr,
            module: tag.module.clone(),
            name: tag.name.clone(),
            type_params: tag
                .type_params
                .iter()
                .map(|tp| self.normalize_type_tag(tp))
                .collect(),
        }
    }

    /// Normalize a TypeTag, recursively normalizing any struct addresses.
    pub fn normalize_type_tag(
        &self,
        tag: &move_core_types::language_storage::TypeTag,
    ) -> move_core_types::language_storage::TypeTag {
        use move_core_types::language_storage::TypeTag;

        match tag {
            TypeTag::Struct(st) => TypeTag::Struct(Box::new(self.normalize_struct_tag_address(st))),
            TypeTag::Vector(inner) => TypeTag::Vector(Box::new(self.normalize_type_tag(inner))),
            // Primitives don't have addresses
            other => other.clone(),
        }
    }

    /// Normalize a type string by replacing all storage_id addresses with original_id.
    ///
    /// This scans through a type string (e.g., from GraphQL) and replaces any
    /// storage_id addresses with their corresponding original_id addresses.
    /// This is essential for matching dynamic field types where GraphQL returns
    /// storage_id but bytecode analysis uses original_id.
    ///
    /// Example: `"0xd384...::market::Market<0x2::sui::SUI>"` becomes
    ///          `"0xefe8...::market::Market<0x2::sui::SUI>"`
    /// where `0xd384...` is a storage_id that maps to original_id `0xefe8...`
    pub fn normalize_type_string(&self, type_str: &str) -> String {
        let mut result = String::with_capacity(type_str.len());
        let chars: Vec<char> = type_str.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            // Look for 0x prefix indicating an address
            if i + 1 < chars.len() && chars[i] == '0' && chars[i + 1] == 'x' {
                // Extract the full hex address
                let mut end = i + 2;
                while end < chars.len() && chars[end].is_ascii_hexdigit() {
                    end += 1;
                }

                let addr: String = chars[i..end].iter().collect();
                // Normalize this address (storage_id -> original_id)
                let normalized_addr = self.normalize_to_original(&addr);
                result.push_str(&normalized_addr);
                i = end;
            } else {
                result.push(chars[i]);
                i += 1;
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_normalize() {
        let mut resolver = PackageUpgradeResolver::new();

        // Package v1: original and storage are same
        resolver.register_package("0xabc", "0xabc");

        // Package v2: storage is different
        resolver.register_package("0xdef", "0xabc");

        // Both should normalize to the original
        assert_eq!(
            resolver.normalize_to_original("0xabc"),
            normalize_address("0xabc")
        );
        assert_eq!(
            resolver.normalize_to_original("0xdef"),
            normalize_address("0xabc")
        );

        // Storage lookup should return latest
        assert_eq!(resolver.get_storage_id("0xabc"), normalize_address("0xdef"));
    }

    #[test]
    fn test_linkage_registration() {
        let mut resolver = PackageUpgradeResolver::new();

        resolver.register_linkage("0xoriginal", "0xupgraded");

        assert_eq!(
            resolver.normalize_to_original("0xupgraded"),
            normalize_address("0xoriginal")
        );
    }

    #[test]
    fn test_unknown_address() {
        let resolver = PackageUpgradeResolver::new();

        // Unknown addresses should normalize to themselves
        assert_eq!(
            resolver.normalize_to_original("0xunknown"),
            normalize_address("0xunknown")
        );
    }

    #[test]
    fn test_normalize_type_string_simple() {
        let mut resolver = PackageUpgradeResolver::new();
        // Register: storage_id 0xabc123 maps to original_id 0xdef456
        resolver.register_linkage("0xdef456", "0xabc123");

        let type_str = "0xabc123::module::Type";
        let normalized = resolver.normalize_type_string(type_str);

        // Should replace storage with original (both get normalized to full 64-char hex)
        assert!(normalized.contains("::module::Type"));
        // Should contain the original_id (def456), not the storage_id (abc123)
        let normalized_original = normalize_address("0xdef456");
        let normalized_storage = normalize_address("0xabc123");
        assert!(normalized.contains(&normalized_original));
        assert!(!normalized.contains(&normalized_storage));
    }

    #[test]
    fn test_normalize_type_string_nested_generics() {
        let mut resolver = PackageUpgradeResolver::new();
        resolver.register_linkage(
            "0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf",
            "0xd384ded6b9503e6400000000000000000000000000000000000000000000abcd",
        );

        let type_str = "0x2::dynamic_field::Field<0xd384ded6b9503e6400000000000000000000000000000000000000000000abcd::market::Key<0x2::sui::SUI>, u64>";
        let normalized = resolver.normalize_type_string(type_str);

        // Should contain the original_id, not the storage_id
        assert!(normalized
            .contains("0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf"));
        // Framework addresses (0x2) should pass through unchanged
        assert!(normalized
            .contains("0x0000000000000000000000000000000000000000000000000000000000000002"));
    }

    #[test]
    fn test_normalize_type_string_multiple_addresses() {
        let mut resolver = PackageUpgradeResolver::new();
        resolver.register_linkage("0xaaa", "0xbbb");
        resolver.register_linkage("0xccc", "0xddd");

        let type_str = "0xbbb::mod::Outer<0xddd::mod::Inner>";
        let normalized = resolver.normalize_type_string(type_str);

        // Both addresses should be normalized to their originals
        let normalized_aaa = normalize_address("0xaaa");
        let normalized_ccc = normalize_address("0xccc");
        assert!(normalized.contains(&normalized_aaa));
        assert!(normalized.contains(&normalized_ccc));
    }
}
