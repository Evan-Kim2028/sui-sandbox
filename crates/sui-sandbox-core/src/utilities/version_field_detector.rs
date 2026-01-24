//! Version Field Detection
//!
//! This module identifies objects with version fields that need patching for
//! historical transaction replay.
//!
//! ## Problem
//!
//! Many Sui protocols implement version-locking patterns:
//! - Objects store a `package_version` field (e.g., 5)
//! - Bytecode has a `CURRENT_VERSION` constant (e.g., 8)
//! - Version checks fail when these don't match
//!
//! ## Detection Strategy
//!
//! 1. Scan struct layouts for known version field patterns
//! 2. Check if field values look like version numbers (small positive integers)
//! 3. Return detected fields for patching
//!
//! ## Usage
//!
//! ```ignore
//! use sui_sandbox_core::utilities::VersionFieldDetector;
//!
//! let mut detector = VersionFieldDetector::new();
//! let fields = detector.detect_version_fields(&type_str, &bcs_bytes, &layout);
//! ```

use std::collections::HashSet;

/// Information about a detected version field.
#[derive(Debug, Clone)]
pub struct DetectedVersionField {
    /// Full type string of the containing object
    pub type_string: String,
    /// Name of the version field
    pub field_name: String,
    /// Byte offset of the field in BCS data
    pub byte_offset: usize,
    /// Size of the field in bytes
    pub field_size: usize,
    /// Current value of the field
    pub current_value: u64,
}

/// Patterns for detecting version fields.
#[derive(Debug, Clone)]
pub struct VersionPattern {
    /// Field name to match (exact match)
    pub field_name: String,
    /// Optional type pattern - only match if type contains this substring
    pub type_pattern: Option<String>,
    /// Expected field type (U64, U128, etc.)
    pub field_type: FieldType,
}

/// Supported version field types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldType {
    U64,
    U128,
}

impl FieldType {
    /// Get the size of this field type in bytes.
    pub fn size(&self) -> usize {
        match self {
            FieldType::U64 => 8,
            FieldType::U128 => 16,
        }
    }
}

/// Default version field patterns found in common DeFi protocols.
pub fn default_patterns() -> Vec<VersionPattern> {
    vec![
        // Common in Cetus, Bluefin, and many protocols
        VersionPattern {
            field_name: "package_version".to_string(),
            type_pattern: None,
            field_type: FieldType::U64,
        },
        // Scallop uses Version struct with `value` field
        VersionPattern {
            field_name: "value".to_string(),
            type_pattern: Some("Version".to_string()),
            field_type: FieldType::U64,
        },
        // Some protocols use `version` directly
        VersionPattern {
            field_name: "version".to_string(),
            type_pattern: None,
            field_type: FieldType::U64,
        },
    ]
}

/// Detector for version fields in objects.
pub struct VersionFieldDetector {
    patterns: Vec<VersionPattern>,
    /// Range of values considered "version-like"
    version_range: (u64, u64),
}

impl VersionFieldDetector {
    /// Create a new detector with default patterns.
    pub fn new() -> Self {
        Self {
            patterns: default_patterns(),
            version_range: (1, 100),
        }
    }

    /// Add a custom pattern.
    pub fn add_pattern(&mut self, pattern: VersionPattern) {
        self.patterns.push(pattern);
    }

    /// Set the range of values considered "version-like".
    pub fn set_version_range(&mut self, min: u64, max: u64) {
        self.version_range = (min, max);
    }

    /// Get the patterns being used.
    pub fn patterns(&self) -> &[VersionPattern] {
        &self.patterns
    }

    /// Check if a value looks like a version number.
    pub fn is_version_like(&self, value: u64) -> bool {
        value >= self.version_range.0 && value <= self.version_range.1
    }

    /// Check if a type string matches a pattern's type filter.
    pub fn type_matches_pattern(&self, type_str: &str, pattern: &VersionPattern) -> bool {
        match &pattern.type_pattern {
            Some(type_pattern) => type_str.contains(type_pattern),
            None => true,
        }
    }

    /// Detect version fields from field layout information.
    ///
    /// This uses layout data (field names, types, offsets) to identify
    /// version fields that need patching.
    pub fn detect_from_layout(
        &self,
        type_str: &str,
        fields: &[(String, super::generic_patcher::MoveType, usize)], // (name, type, offset)
        bcs_bytes: &[u8],
    ) -> Vec<DetectedVersionField> {
        let mut detected = Vec::new();

        for pattern in &self.patterns {
            if !self.type_matches_pattern(type_str, pattern) {
                continue;
            }

            // Find matching field
            for (field_name, field_type, offset) in fields {
                if field_name != &pattern.field_name {
                    continue;
                }

                // Check field type matches
                let matches_type = matches!(
                    (pattern.field_type, field_type),
                    (FieldType::U64, super::generic_patcher::MoveType::U64)
                        | (FieldType::U128, super::generic_patcher::MoveType::U128)
                );

                if !matches_type {
                    continue;
                }

                // Try to read current value
                let field_size = pattern.field_type.size();
                if *offset + field_size <= bcs_bytes.len() {
                    let value = match pattern.field_type {
                        FieldType::U64 => {
                            let bytes: [u8; 8] =
                                bcs_bytes[*offset..*offset + 8].try_into().unwrap_or([0; 8]);
                            u64::from_le_bytes(bytes)
                        }
                        FieldType::U128 => {
                            let bytes: [u8; 16] = bcs_bytes[*offset..*offset + 16]
                                .try_into()
                                .unwrap_or([0; 16]);
                            u128::from_le_bytes(bytes) as u64 // Truncate for comparison
                        }
                    };

                    // Only include if value looks like a version number
                    if self.is_version_like(value) {
                        detected.push(DetectedVersionField {
                            type_string: type_str.to_string(),
                            field_name: field_name.clone(),
                            byte_offset: *offset,
                            field_size,
                            current_value: value,
                        });
                    }
                }
            }
        }

        detected
    }

    /// Detect version fields using only type information (no BCS parsing).
    ///
    /// This is useful when you have struct layout but can't parse the BCS data.
    /// Returns potential fields that might need patching.
    pub fn detect_potential_fields(
        &self,
        type_str: &str,
        field_names: &HashSet<String>,
    ) -> Vec<String> {
        let mut potential = Vec::new();

        for pattern in &self.patterns {
            if !self.type_matches_pattern(type_str, pattern) {
                continue;
            }

            if field_names.contains(&pattern.field_name) {
                potential.push(pattern.field_name.clone());
            }
        }

        potential
    }
}

impl Default for VersionFieldDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Well-known version field positions for protocols that can't be introspected.
///
/// These are positions for complex structs where BCS decoding fails.
/// Format: (type_pattern, position, field_size)
/// where position is `FromEnd(n)` meaning `len - n` or `FromStart(n)` meaning offset n.
///
/// **Important**: Cetus GlobalConfig's `package_version` is the LAST field in the struct,
/// so it's at `len - 8`, NOT at a fixed offset like 32. The struct has variable-length
/// fields (fee_tiers Table, acl ACL) before package_version.
#[derive(Debug, Clone, Copy)]
pub enum FieldPosition {
    /// Offset from start of BCS data
    FromStart(usize),
    /// Offset from end of BCS data (e.g., FromEnd(8) means last 8 bytes)
    FromEnd(usize),
}

/// Well-known version field configurations for protocols.
///
/// Format: (type_pattern, position, field_size, default_version)
/// - `type_pattern`: Substring to match in the full type string
/// - `position`: Where the version field is located in BCS
/// - `field_size`: Size in bytes (8 for u64)
/// - `default_version`: The version value to patch to (usually 1 for v1 bytecode)
///
/// The `default_version` is used when no version is detected from bytecode.
/// For Cetus v1, the bytecode checks `package_version == 1` (equality, not GE),
/// so we MUST patch to exactly 1.
pub const WELL_KNOWN_VERSION_CONFIGS: &[(&str, FieldPosition, usize, u64)] = &[
    // Cetus GlobalConfig: package_version is the LAST field (u64)
    // Struct: id (32) + protocol_fee_rate (8) + fee_tiers (variable) + acl (variable) + package_version (8)
    // Cetus v1 bytecode uses equality check: package_version == 1
    ("::config::GlobalConfig", FieldPosition::FromEnd(8), 8, 1),
    // Bluefin GlobalConfig (same pattern - version is last field)
    // Bluefin also uses version 1 in early bytecode
    (
        "bluefin::config::GlobalConfig",
        FieldPosition::FromEnd(8),
        8,
        1,
    ),
    // NOTE: Scallop Version is NOT included here. Scallop's version check fails even
    // with patching because the current Scallop package uses a different version constant
    // than the historical transaction's Version object. Proper replay requires fetching
    // the historical version of the Scallop package that was active at transaction time.
];

/// Legacy constant for backward compatibility
pub const WELL_KNOWN_VERSION_OFFSETS: &[(&str, FieldPosition, usize)] = &[
    ("::config::GlobalConfig", FieldPosition::FromEnd(8), 8),
    (
        "bluefin::config::GlobalConfig",
        FieldPosition::FromEnd(8),
        8,
    ),
];

/// Find well-known version field configuration for a type.
/// Returns (position, size, default_version).
pub fn find_well_known_config(type_str: &str) -> Option<(FieldPosition, usize, u64)> {
    for (pattern, position, size, default_version) in WELL_KNOWN_VERSION_CONFIGS {
        if type_str.contains(pattern) {
            return Some((*position, *size, *default_version));
        }
    }
    None
}

/// Find well-known version field offset for a type (legacy API).
/// Returns (position, size) where position is calculated based on the BCS length.
pub fn find_well_known_offset(type_str: &str) -> Option<(FieldPosition, usize)> {
    find_well_known_config(type_str).map(|(pos, size, _)| (pos, size))
}

/// Calculate actual byte offset from field position and BCS length.
pub fn calculate_offset(position: FieldPosition, bcs_len: usize) -> Option<usize> {
    match position {
        FieldPosition::FromStart(offset) => {
            if offset < bcs_len {
                Some(offset)
            } else {
                None
            }
        }
        FieldPosition::FromEnd(from_end) => {
            if from_end <= bcs_len {
                Some(bcs_len - from_end)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_patterns() {
        let patterns = default_patterns();
        assert!(patterns.iter().any(|p| p.field_name == "package_version"));
        assert!(patterns.iter().any(|p| p.field_name == "value"));
    }

    #[test]
    fn test_version_field_detector_new() {
        let detector = VersionFieldDetector::new();
        assert!(!detector.patterns().is_empty());
    }

    #[test]
    fn test_is_version_like() {
        let detector = VersionFieldDetector::new();
        assert!(detector.is_version_like(1));
        assert!(detector.is_version_like(50));
        assert!(detector.is_version_like(100));
        assert!(!detector.is_version_like(0));
        assert!(!detector.is_version_like(101));
        assert!(!detector.is_version_like(1000));
    }

    #[test]
    fn test_type_matches_pattern() {
        let detector = VersionFieldDetector::new();

        let pattern_any = VersionPattern {
            field_name: "version".to_string(),
            type_pattern: None,
            field_type: FieldType::U64,
        };
        assert!(detector.type_matches_pattern("0x1::foo::Bar", &pattern_any));

        let pattern_version = VersionPattern {
            field_name: "value".to_string(),
            type_pattern: Some("Version".to_string()),
            field_type: FieldType::U64,
        };
        assert!(detector.type_matches_pattern("0x1::module::Version", &pattern_version));
        assert!(!detector.type_matches_pattern("0x1::module::Config", &pattern_version));
    }

    #[test]
    fn test_detect_potential_fields() {
        let detector = VersionFieldDetector::new();
        let mut fields = HashSet::new();
        fields.insert("id".to_string());
        fields.insert("package_version".to_string());
        fields.insert("data".to_string());

        let potential = detector.detect_potential_fields("0x1::config::GlobalConfig", &fields);
        assert!(potential.contains(&"package_version".to_string()));
        assert!(!potential.contains(&"id".to_string()));
    }

    #[test]
    fn test_well_known_offsets() {
        assert!(find_well_known_offset("0x1eab::config::GlobalConfig").is_some());
        assert!(find_well_known_offset("0x1::random::Struct").is_none());
    }

    #[test]
    fn test_field_type_size() {
        assert_eq!(FieldType::U64.size(), 8);
        assert_eq!(FieldType::U128.size(), 16);
    }
}
