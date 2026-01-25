//! Key value synthesis for deriving dynamic field child IDs from predictions.
//!
//! This module bridges the gap between static predictions (key types) and
//! ground truth objects (child IDs) by synthesizing key values for deterministic
//! key types.
//!
//! ## Deterministic Key Types
//!
//! Some key types have predictable BCS encodings:
//!
//! 1. **Phantom/Empty Structs** - Zero-field structs like `BalanceKey<T>`
//!    - BCS encoding is empty (`[]`)
//!    - Child ID fully derivable from parent + type
//!
//! 2. **Unit Type** - `()`
//!    - BCS encoding is empty
//!
//! ## Usage
//!
//! ```rust,ignore
//! use sui_sandbox_core::mm2::KeyValueSynthesizer;
//!
//! let synthesizer = KeyValueSynthesizer::new();
//!
//! // For a phantom key like BalanceKey<SUI>
//! let key_type = "0x2c8d...::balance_manager::BalanceKey<0x2::sui::SUI>";
//! if let Some(child_id) = synthesizer.derive_child_id(parent_id, key_type) {
//!     println!("Derived child: {}", child_id);
//! }
//! ```

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::{StructTag, TypeTag};
use std::collections::HashSet;
use std::str::FromStr;

/// Synthesizes key values for deterministic key types.
pub struct KeyValueSynthesizer {
    /// Known phantom/empty struct patterns (module::name format)
    /// These are structs with no fields, so BCS encoding is empty
    known_phantom_patterns: HashSet<String>,
}

impl Default for KeyValueSynthesizer {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyValueSynthesizer {
    /// Create a new synthesizer with common phantom patterns.
    pub fn new() -> Self {
        let mut known_phantom_patterns = HashSet::new();

        // DeepBook patterns
        known_phantom_patterns.insert("balance_manager::BalanceKey".to_string());

        // Common Sui patterns for marker types
        // These are typically phantom structs used as type-indexed keys

        Self {
            known_phantom_patterns,
        }
    }

    /// Register a known phantom/empty struct pattern.
    ///
    /// Pattern format: "module_name::StructName" (without package address)
    pub fn register_phantom_pattern(&mut self, pattern: &str) {
        self.known_phantom_patterns.insert(pattern.to_string());
    }

    /// Check if a key type is a known phantom/empty struct.
    ///
    /// Returns true if:
    /// 1. It matches a known phantom pattern, OR
    /// 2. It's a struct type with generic params that matches phantom heuristics
    pub fn is_phantom_key(&self, key_type: &str) -> bool {
        // Check against known patterns
        for pattern in &self.known_phantom_patterns {
            if key_type.contains(pattern) {
                return true;
            }
        }

        // Heuristic: Struct names ending in "Key" with type params are often phantom
        // e.g., "SomeKey<T>", "BalanceKey<COIN>", "PoolKey<A, B>"
        if self.looks_like_phantom_key(key_type) {
            return true;
        }

        false
    }

    /// Heuristic check for phantom key patterns.
    fn looks_like_phantom_key(&self, key_type: &str) -> bool {
        // Must be a struct (contains ::)
        if !key_type.contains("::") {
            return false;
        }

        // Extract the struct name (last segment before any <)
        let without_generics = key_type.split('<').next().unwrap_or(key_type);
        let struct_name = without_generics.rsplit("::").next().unwrap_or("");

        // Common phantom key naming patterns
        struct_name.ends_with("Key")
            || struct_name.ends_with("Marker")
            || struct_name.ends_with("Witness")
            || struct_name.ends_with("Cap")
    }

    /// Try to derive the child object ID for a predicted key type.
    ///
    /// Returns Some(child_id) if the key type is deterministic (phantom/empty),
    /// None if the key value cannot be synthesized.
    pub fn derive_child_id(
        &self,
        parent: AccountAddress,
        key_type_str: &str,
    ) -> Option<AccountAddress> {
        // Only synthesize for phantom keys
        if !self.is_phantom_key(key_type_str) {
            return None;
        }

        // Parse the key type string to a TypeTag
        let type_tag = self.parse_type_tag(key_type_str)?;

        // For phantom structs, BCS encoding is empty
        let key_bytes: Vec<u8> = vec![];

        // Derive the child ID
        derive_dynamic_field_id(parent, &type_tag, &key_bytes).ok()
    }

    /// Try to derive child IDs for multiple parent candidates.
    ///
    /// This is useful when we have a predicted key type but need to try
    /// multiple potential parent objects.
    pub fn derive_child_ids_for_parents(
        &self,
        parents: &[AccountAddress],
        key_type_str: &str,
    ) -> Vec<(AccountAddress, AccountAddress)> {
        if !self.is_phantom_key(key_type_str) {
            return vec![];
        }

        let type_tag = match self.parse_type_tag(key_type_str) {
            Some(tt) => tt,
            None => return vec![],
        };

        let key_bytes: Vec<u8> = vec![];

        parents
            .iter()
            .filter_map(|parent| {
                derive_dynamic_field_id(*parent, &type_tag, &key_bytes)
                    .ok()
                    .map(|child| (*parent, child))
            })
            .collect()
    }

    /// Parse a type string into a TypeTag.
    fn parse_type_tag(&self, type_str: &str) -> Option<TypeTag> {
        // Handle primitive types
        match type_str {
            "bool" => return Some(TypeTag::Bool),
            "u8" => return Some(TypeTag::U8),
            "u16" => return Some(TypeTag::U16),
            "u32" => return Some(TypeTag::U32),
            "u64" => return Some(TypeTag::U64),
            "u128" => return Some(TypeTag::U128),
            "u256" => return Some(TypeTag::U256),
            "address" => return Some(TypeTag::Address),
            "signer" => return Some(TypeTag::Signer),
            _ => {}
        }

        // Handle vector<T>
        if type_str.starts_with("vector<") && type_str.ends_with('>') {
            let inner = &type_str[7..type_str.len() - 1];
            let inner_tag = self.parse_type_tag(inner)?;
            return Some(TypeTag::Vector(Box::new(inner_tag)));
        }

        // Handle struct types: address::module::Name<TypeArgs>
        self.parse_struct_tag(type_str).map(TypeTag::Struct)
    }

    /// Parse a struct type string into a StructTag.
    fn parse_struct_tag(&self, type_str: &str) -> Option<Box<StructTag>> {
        // Split off type arguments if present
        let (base, type_args_str) = if let Some(idx) = type_str.find('<') {
            let base = &type_str[..idx];
            let args = &type_str[idx + 1..type_str.len() - 1]; // Remove < and >
            (base, Some(args))
        } else {
            (type_str, None)
        };

        // Parse base: address::module::name
        let parts: Vec<&str> = base.split("::").collect();
        if parts.len() < 3 {
            return None;
        }

        // Handle full address format (0x followed by hex)
        let address = AccountAddress::from_hex_literal(parts[0]).ok()?;
        let module = move_core_types::identifier::Identifier::from_str(parts[1]).ok()?;
        let name = move_core_types::identifier::Identifier::from_str(parts[2]).ok()?;

        // Parse type arguments recursively
        let type_params = if let Some(args_str) = type_args_str {
            self.parse_type_args(args_str)?
        } else {
            vec![]
        };

        Some(Box::new(StructTag {
            address,
            module,
            name,
            type_params,
        }))
    }

    /// Parse comma-separated type arguments, handling nested generics.
    fn parse_type_args(&self, args_str: &str) -> Option<Vec<TypeTag>> {
        let mut result = Vec::new();
        let mut current = String::new();
        let mut depth = 0;

        for c in args_str.chars() {
            match c {
                '<' => {
                    depth += 1;
                    current.push(c);
                }
                '>' => {
                    depth -= 1;
                    current.push(c);
                }
                ',' if depth == 0 => {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        result.push(self.parse_type_tag(trimmed)?);
                    }
                    current.clear();
                }
                _ => {
                    current.push(c);
                }
            }
        }

        // Don't forget the last argument
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            result.push(self.parse_type_tag(trimmed)?);
        }

        Some(result)
    }

    /// Get statistics about the synthesizer.
    pub fn stats(&self) -> SynthesizerStats {
        SynthesizerStats {
            known_phantom_patterns: self.known_phantom_patterns.len(),
        }
    }
}

/// Statistics about the synthesizer.
#[derive(Debug, Clone)]
pub struct SynthesizerStats {
    pub known_phantom_patterns: usize,
}

/// Derive the object ID for a dynamic field.
///
/// Implements the same formula as Sui's `dynamic_field::derive_dynamic_field_id`:
/// ```text
/// Blake2b256(0xf0 || parent || len(key_bytes) || key_bytes || bcs(key_type_tag))
/// ```
fn derive_dynamic_field_id(
    parent: AccountAddress,
    key_type_tag: &TypeTag,
    key_bytes: &[u8],
) -> Result<AccountAddress, String> {
    use fastcrypto::hash::{Blake2b256, HashFunction};

    // HashingIntentScope::ChildObjectId = 0xf0
    const CHILD_OBJECT_ID_SCOPE: u8 = 0xf0;

    // BCS-serialize the type tag
    let type_tag_bytes =
        bcs::to_bytes(key_type_tag).map_err(|e| format!("Failed to BCS-serialize type tag: {}", e))?;

    // Build the input: scope || parent || len(key) || key || type_tag
    let mut input = Vec::with_capacity(1 + 32 + 8 + key_bytes.len() + type_tag_bytes.len());
    input.push(CHILD_OBJECT_ID_SCOPE);
    input.extend_from_slice(parent.as_ref());
    input.extend_from_slice(&(key_bytes.len() as u64).to_le_bytes());
    input.extend_from_slice(key_bytes);
    input.extend_from_slice(&type_tag_bytes);

    // Hash with Blake2b-256
    let hash = Blake2b256::digest(&input);

    // Convert to AccountAddress
    Ok(AccountAddress::new(hash.digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_phantom_key_known_pattern() {
        let synth = KeyValueSynthesizer::new();

        // Known pattern
        assert!(synth.is_phantom_key(
            "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809::balance_manager::BalanceKey<0x2::sui::SUI>"
        ));
    }

    #[test]
    fn test_is_phantom_key_heuristic() {
        let synth = KeyValueSynthesizer::new();

        // Heuristic: ends with "Key"
        assert!(synth.is_phantom_key("0xabc::module::SomeKey<T>"));
        assert!(synth.is_phantom_key("0xabc::module::PoolKey<A, B>"));

        // Heuristic: ends with "Marker"
        assert!(synth.is_phantom_key("0xabc::module::TypeMarker<T>"));

        // Not a phantom key pattern
        assert!(!synth.is_phantom_key("0xabc::module::Balance<T>"));
        assert!(!synth.is_phantom_key("u64"));
    }

    #[test]
    fn test_parse_type_tag_primitives() {
        let synth = KeyValueSynthesizer::new();

        assert_eq!(synth.parse_type_tag("u64"), Some(TypeTag::U64));
        assert_eq!(synth.parse_type_tag("bool"), Some(TypeTag::Bool));
        assert_eq!(synth.parse_type_tag("address"), Some(TypeTag::Address));
    }

    #[test]
    fn test_parse_type_tag_vector() {
        let synth = KeyValueSynthesizer::new();

        let tag = synth.parse_type_tag("vector<u8>");
        assert!(matches!(tag, Some(TypeTag::Vector(_))));
    }

    #[test]
    fn test_parse_type_tag_struct() {
        let synth = KeyValueSynthesizer::new();

        let tag = synth.parse_type_tag("0x2::sui::SUI");
        assert!(matches!(tag, Some(TypeTag::Struct(_))));

        if let Some(TypeTag::Struct(st)) = tag {
            assert_eq!(st.module.as_str(), "sui");
            assert_eq!(st.name.as_str(), "SUI");
        }
    }

    #[test]
    fn test_parse_type_tag_generic_struct() {
        let synth = KeyValueSynthesizer::new();

        let tag = synth.parse_type_tag(
            "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809::balance_manager::BalanceKey<0x2::sui::SUI>",
        );
        assert!(matches!(tag, Some(TypeTag::Struct(_))));

        if let Some(TypeTag::Struct(st)) = tag {
            assert_eq!(st.module.as_str(), "balance_manager");
            assert_eq!(st.name.as_str(), "BalanceKey");
            assert_eq!(st.type_params.len(), 1);
        }
    }

    #[test]
    fn test_derive_child_id_for_phantom() {
        let synth = KeyValueSynthesizer::new();

        // This should work for a phantom key
        let parent = AccountAddress::from_hex_literal(
            "0x1d73fdc3474330904cee0a60c9f5b5c0702f7e9e0a1b8d2e4f6a8c0e2d4b6a8c",
        )
        .unwrap();

        let key_type = "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809::balance_manager::BalanceKey<0x2::sui::SUI>";

        let result = synth.derive_child_id(parent, key_type);
        assert!(result.is_some(), "Should derive child ID for phantom key");
    }

    #[test]
    fn test_no_derive_for_non_phantom() {
        let synth = KeyValueSynthesizer::new();

        let parent = AccountAddress::from_hex_literal("0x1").unwrap();

        // u64 is not a phantom key - needs runtime value
        let result = synth.derive_child_id(parent, "u64");
        assert!(result.is_none(), "Should not derive for primitive key");

        // Balance is not a phantom pattern
        let result = synth.derive_child_id(parent, "0xabc::module::Balance<T>");
        assert!(result.is_none(), "Should not derive for non-phantom struct");
    }

    #[test]
    fn test_register_custom_pattern() {
        let mut synth = KeyValueSynthesizer::new();
        synth.register_phantom_pattern("my_module::CustomPhantom");

        assert!(synth.is_phantom_key("0xabc::my_module::CustomPhantom<T>"));
    }
}
