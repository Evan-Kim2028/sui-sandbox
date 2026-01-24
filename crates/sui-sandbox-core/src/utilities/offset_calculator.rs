//! Byte Offset Calculator for BCS-encoded Structs
//!
//! This module calculates byte offsets for fields in BCS-encoded Move structs.
//! This is essential for raw byte patching when full struct decoding fails.
//!
//! ## BCS Encoding Rules
//!
//! BCS (Binary Canonical Serialization) encodes structs by serializing fields
//! in declaration order:
//! - Fixed-size types: Bool (1), U8 (1), U16 (2), U32 (4), U64 (8), U128 (16), U256 (32), Address (32)
//! - Variable-size types: Vector (ULEB128 length + elements), String (ULEB128 length + bytes)
//!
//! ## Limitation
//!
//! When a variable-length field precedes the target field, we cannot calculate
//! the exact offset without parsing the BCS data. In such cases, this calculator
//! returns `OffsetResult::Unknown`.
//!
//! ## Usage
//!
//! ```ignore
//! use sui_sandbox_core::utilities::OffsetCalculator;
//!
//! let calc = OffsetCalculator::new();
//! let offset = calc.calculate_field_offset(&struct_layout, "package_version");
//! ```

use super::generic_patcher::{MoveType, StructLayout};

/// Result of calculating a field offset.
#[derive(Debug, Clone, PartialEq)]
pub enum OffsetResult {
    /// Exact byte offset is known
    Known(usize),
    /// Offset cannot be determined (variable-length field precedes target)
    Unknown,
    /// Field not found in struct
    NotFound,
}

impl OffsetResult {
    /// Get the offset if known, or None otherwise.
    pub fn as_known(&self) -> Option<usize> {
        match self {
            OffsetResult::Known(offset) => Some(*offset),
            _ => None,
        }
    }

    /// Check if offset is known.
    pub fn is_known(&self) -> bool {
        matches!(self, OffsetResult::Known(_))
    }
}

/// Calculator for field byte offsets in BCS-encoded structs.
pub struct OffsetCalculator {
    /// Cache of known struct offsets (type_str -> field_name -> offset)
    cached_offsets: std::collections::HashMap<String, std::collections::HashMap<String, usize>>,
}

impl OffsetCalculator {
    /// Create a new offset calculator.
    pub fn new() -> Self {
        Self {
            cached_offsets: std::collections::HashMap::new(),
        }
    }

    /// Get the fixed size of a Move type in bytes, if it has one.
    ///
    /// Returns None for variable-size types (Vector, String, etc.).
    pub fn type_fixed_size(move_type: &MoveType) -> Option<usize> {
        match move_type {
            MoveType::Bool | MoveType::U8 => Some(1),
            MoveType::U16 => Some(2),
            MoveType::U32 => Some(4),
            MoveType::U64 => Some(8),
            MoveType::U128 => Some(16),
            MoveType::U256 => Some(32),
            MoveType::Address => Some(32),
            MoveType::Signer => Some(32), // Same as Address
            MoveType::Vector(_) => None,  // Variable length
            MoveType::Struct {
                address,
                module,
                name,
                ..
            } => {
                // Well-known fixed-size structs
                let sui_framework =
                    move_core_types::account_address::AccountAddress::from_hex_literal("0x2")
                        .unwrap_or(move_core_types::account_address::AccountAddress::ZERO);
                if *address == sui_framework {
                    match (module.as_str(), name.as_str()) {
                        ("object", "UID") | ("object", "ID") => Some(32),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            MoveType::TypeParameter(_) => None, // Can't know without concrete type
        }
    }

    /// Check if a type has fixed size.
    pub fn is_fixed_size(move_type: &MoveType) -> bool {
        Self::type_fixed_size(move_type).is_some()
    }

    /// Calculate the byte offset of a field in a struct.
    ///
    /// This walks through fields in declaration order, accumulating offsets.
    /// If a variable-size field is encountered before the target, returns Unknown.
    pub fn calculate_field_offset(&self, layout: &StructLayout, field_name: &str) -> OffsetResult {
        let mut offset = 0;

        for field in &layout.fields {
            if field.name == field_name {
                return OffsetResult::Known(offset);
            }

            match Self::type_fixed_size(&field.field_type) {
                Some(size) => {
                    offset += size;
                }
                None => {
                    // Variable-size field encountered before target
                    // We can't determine the exact offset
                    return OffsetResult::Unknown;
                }
            }
        }

        OffsetResult::NotFound
    }

    /// Calculate offsets for all fixed-position fields.
    ///
    /// Returns a map of field_name -> offset for all fields that can be
    /// determined. Stops at the first variable-size field.
    pub fn calculate_all_known_offsets(
        &self,
        layout: &StructLayout,
    ) -> std::collections::HashMap<String, usize> {
        let mut offsets = std::collections::HashMap::new();
        let mut current_offset = 0;

        for field in &layout.fields {
            // Add this field's offset
            offsets.insert(field.name.clone(), current_offset);

            // Try to advance to next field
            match Self::type_fixed_size(&field.field_type) {
                Some(size) => {
                    current_offset += size;
                }
                None => {
                    // Can't calculate further offsets
                    break;
                }
            }
        }

        offsets
    }

    /// Cache offsets for a type.
    pub fn cache_offsets(&mut self, type_str: &str, layout: &StructLayout) {
        let offsets = self.calculate_all_known_offsets(layout);
        self.cached_offsets.insert(type_str.to_string(), offsets);
    }

    /// Get cached offset for a field.
    pub fn get_cached_offset(&self, type_str: &str, field_name: &str) -> Option<usize> {
        self.cached_offsets
            .get(type_str)
            .and_then(|fields| fields.get(field_name).copied())
    }

    /// Calculate offset with caching.
    pub fn get_or_calculate(
        &mut self,
        type_str: &str,
        layout: &StructLayout,
        field_name: &str,
    ) -> OffsetResult {
        // Check cache first
        if let Some(offset) = self.get_cached_offset(type_str, field_name) {
            return OffsetResult::Known(offset);
        }

        // Calculate and cache
        let result = self.calculate_field_offset(layout, field_name);
        if result.is_known() {
            self.cache_offsets(type_str, layout);
        }

        result
    }
}

impl Default for OffsetCalculator {
    fn default() -> Self {
        Self::new()
    }
}

/// Well-known struct layouts for common protocols.
///
/// These are manually determined layouts for structs that are frequently
/// patched but may have complex fields that prevent automatic introspection.
pub mod well_known {
    /// Offset information for a well-known field.
    #[derive(Debug, Clone)]
    pub struct WellKnownField {
        pub field_name: &'static str,
        pub byte_offset: usize,
        pub size: usize,
    }

    /// Get well-known fields for a type.
    pub fn get_well_known_fields(type_str: &str) -> Option<Vec<WellKnownField>> {
        // Cetus GlobalConfig
        // Layout: { id: UID (32), package_version: u64 (8), ... }
        if type_str.contains("::config::GlobalConfig") {
            return Some(vec![WellKnownField {
                field_name: "package_version",
                byte_offset: 32,
                size: 8,
            }]);
        }

        // Add more well-known layouts here as discovered

        None
    }

    /// Get offset for a specific field in a well-known type.
    pub fn get_well_known_offset(type_str: &str, field_name: &str) -> Option<(usize, usize)> {
        get_well_known_fields(type_str)?
            .into_iter()
            .find(|f| f.field_name == field_name)
            .map(|f| (f.byte_offset, f.size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utilities::generic_patcher::FieldLayout;
    use move_core_types::account_address::AccountAddress;

    fn make_layout(fields: Vec<(&str, MoveType)>) -> StructLayout {
        StructLayout {
            address: AccountAddress::ZERO,
            module: "test".to_string(),
            name: "TestStruct".to_string(),
            fields: fields
                .into_iter()
                .map(|(name, field_type)| FieldLayout {
                    name: name.to_string(),
                    field_type,
                })
                .collect(),
        }
    }

    #[test]
    fn test_type_fixed_size() {
        assert_eq!(OffsetCalculator::type_fixed_size(&MoveType::Bool), Some(1));
        assert_eq!(OffsetCalculator::type_fixed_size(&MoveType::U64), Some(8));
        assert_eq!(
            OffsetCalculator::type_fixed_size(&MoveType::Address),
            Some(32)
        );
        assert_eq!(
            OffsetCalculator::type_fixed_size(&MoveType::Vector(Box::new(MoveType::U8))),
            None
        );
    }

    #[test]
    fn test_calculate_field_offset_simple() {
        let layout = make_layout(vec![
            ("id", MoveType::Address),          // 0-31 (32 bytes)
            ("package_version", MoveType::U64), // 32-39 (8 bytes)
            ("enabled", MoveType::Bool),        // 40 (1 byte)
        ]);

        let calc = OffsetCalculator::new();

        assert_eq!(
            calc.calculate_field_offset(&layout, "id"),
            OffsetResult::Known(0)
        );
        assert_eq!(
            calc.calculate_field_offset(&layout, "package_version"),
            OffsetResult::Known(32)
        );
        assert_eq!(
            calc.calculate_field_offset(&layout, "enabled"),
            OffsetResult::Known(40)
        );
        assert_eq!(
            calc.calculate_field_offset(&layout, "nonexistent"),
            OffsetResult::NotFound
        );
    }

    #[test]
    fn test_calculate_field_offset_with_variable_size() {
        let layout = make_layout(vec![
            ("id", MoveType::Address),                          // 0-31 (32 bytes)
            ("data", MoveType::Vector(Box::new(MoveType::U8))), // Variable
            ("version", MoveType::U64),                         // Unknown
        ]);

        let calc = OffsetCalculator::new();

        assert_eq!(
            calc.calculate_field_offset(&layout, "id"),
            OffsetResult::Known(0)
        );
        assert_eq!(
            calc.calculate_field_offset(&layout, "data"),
            OffsetResult::Known(32)
        );
        // version comes after variable-size field
        assert_eq!(
            calc.calculate_field_offset(&layout, "version"),
            OffsetResult::Unknown
        );
    }

    #[test]
    fn test_calculate_all_known_offsets() {
        let layout = make_layout(vec![
            ("a", MoveType::U8),                             // 0
            ("b", MoveType::U64),                            // 1
            ("c", MoveType::Vector(Box::new(MoveType::U8))), // 9 (variable)
            ("d", MoveType::U64),                            // Unknown (after variable)
        ]);

        let calc = OffsetCalculator::new();
        let offsets = calc.calculate_all_known_offsets(&layout);

        assert_eq!(offsets.get("a"), Some(&0));
        assert_eq!(offsets.get("b"), Some(&1));
        assert_eq!(offsets.get("c"), Some(&9)); // Offset is known, but can't go further
        assert_eq!(offsets.get("d"), None); // Not calculated
    }

    #[test]
    fn test_offset_result_methods() {
        let known = OffsetResult::Known(32);
        assert_eq!(known.as_known(), Some(32));
        assert!(known.is_known());

        let unknown = OffsetResult::Unknown;
        assert_eq!(unknown.as_known(), None);
        assert!(!unknown.is_known());

        let not_found = OffsetResult::NotFound;
        assert_eq!(not_found.as_known(), None);
        assert!(!not_found.is_known());
    }

    #[test]
    fn test_well_known_fields() {
        let fields = well_known::get_well_known_fields("0x1eab::config::GlobalConfig");
        assert!(fields.is_some());
        let fields = fields.unwrap();
        assert!(fields.iter().any(|f| f.field_name == "package_version"));

        assert!(well_known::get_well_known_fields("0x1::random::Struct").is_none());
    }

    #[test]
    fn test_well_known_offset() {
        let offset =
            well_known::get_well_known_offset("0x1eab::config::GlobalConfig", "package_version");
        assert_eq!(offset, Some((32, 8)));

        let offset =
            well_known::get_well_known_offset("0x1eab::config::GlobalConfig", "nonexistent");
        assert_eq!(offset, None);
    }
}
