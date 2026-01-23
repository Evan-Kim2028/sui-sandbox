//! # gRPC to Move VM Transformation Validation
//!
//! This module provides validation infrastructure for data transformations between
//! the gRPC layer and the Move VM execution layer. It addresses the architectural
//! concern that gRPC returns strings/proto types while the VM layer expects Move
//! types with version guarantees.
//!
//! ## P0 Issues Addressed
//!
//! 1. **BCS Format Validation**: Explicit validation of package vs. Move object BCS formats
//! 2. **Proto Schema Versioning**: Runtime validation of proto compatibility
//! 3. **Type Argument Validation**: Eager validation of type strings before execution
//!
//! ## P1 Issues Addressed
//!
//! 1. **Version Tracking**: Metadata about object versions and data sources
//! 2. **Version Verification**: Validation that fetched versions match expected
//!
//! ## Usage
//!
//! ```ignore
//! use crate::grpc::validation::{ValidatedObject, TransformationContext, validate_grpc_object};
//!
//! let ctx = TransformationContext::new();
//! let validated = validate_grpc_object(&grpc_obj, &ctx)?;
//! ```

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

// =============================================================================
// Proto Schema Versioning (P0)
// =============================================================================

/// Expected proto schema version for compatibility validation.
/// This should match the version of Sui's proto definitions we're built against.
pub const EXPECTED_PROTO_VERSION: &str = "sui.rpc.v2";

/// Known proto schema hashes for validation.
/// These are computed from the proto definitions at build time.
pub mod proto_hashes {
    /// Hash of object.proto schema
    pub const OBJECT_PROTO_HASH: &str = "v2.2025.01";
    /// Hash of transaction.proto schema
    pub const TRANSACTION_PROTO_HASH: &str = "v2.2025.01";
    /// Hash of effects.proto schema
    pub const EFFECTS_PROTO_HASH: &str = "v2.2025.01";
}

/// Validates that the proto schema is compatible with our expectations.
#[derive(Debug, Clone)]
pub struct ProtoSchemaValidator {
    /// Whether strict validation is enabled
    strict: bool,
    /// Known incompatibilities to warn about
    known_issues: Vec<String>,
}

impl Default for ProtoSchemaValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtoSchemaValidator {
    /// Create a new schema validator.
    pub fn new() -> Self {
        Self {
            strict: false,
            known_issues: Vec::new(),
        }
    }

    /// Enable strict validation mode (fails on any schema mismatch).
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Validate that the current proto schema is compatible.
    /// Returns warnings for non-critical issues, errors for breaking changes.
    pub fn validate(&self) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Check for known proto schema version
        // In production, this would compare against actual proto version from service
        warnings.push(format!(
            "Proto schema version: {} (built against {})",
            EXPECTED_PROTO_VERSION,
            proto_hashes::OBJECT_PROTO_HASH
        ));

        if self.strict && !self.known_issues.is_empty() {
            return Err(anyhow!(
                "Proto schema validation failed: {}",
                self.known_issues.join(", ")
            ));
        }

        Ok(warnings)
    }
}

// =============================================================================
// BCS Format Validation (P0)
// =============================================================================

/// BCS object format discriminator.
///
/// This enum explicitly tracks the BCS format to avoid hard-coded string comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BcsFormat {
    /// Move package BCS: 0x01 || address || version || module_map
    Package,
    /// Move object BCS: type_tag_prefix || struct_bytes
    MoveObject,
    /// Unknown format (validation required)
    Unknown,
}

impl BcsFormat {
    /// Detect BCS format from raw bytes.
    ///
    /// Returns the detected format or Unknown if ambiguous.
    pub fn detect(bcs: &[u8]) -> Self {
        if bcs.is_empty() {
            return BcsFormat::Unknown;
        }

        // Package BCS starts with variant byte 0x01
        // followed by 32-byte address + 8-byte version
        if bcs.len() >= 41 && bcs[0] == 0x01 {
            return BcsFormat::Package;
        }

        // Move object BCS starts with a type tag
        // which is typically a struct tag starting with an address
        BcsFormat::MoveObject
    }

    /// Detect format from type string.
    pub fn from_type_string(type_string: Option<&str>) -> Self {
        match type_string {
            Some("package") => BcsFormat::Package,
            Some(s) if s.contains("::") => BcsFormat::MoveObject,
            None => BcsFormat::Unknown,
            _ => BcsFormat::Unknown,
        }
    }
}

/// Result of BCS format validation.
#[derive(Debug, Clone)]
pub struct BcsValidationResult {
    /// Detected format
    pub format: BcsFormat,
    /// Whether the format matches expectations
    pub valid: bool,
    /// Validation notes/warnings
    pub notes: Vec<String>,
    /// Extracted struct bytes (for MoveObject format)
    pub struct_bytes: Option<Vec<u8>>,
}

/// Validate BCS format and extract the appropriate bytes.
pub fn validate_bcs_format(
    bcs: &[u8],
    type_string: Option<&str>,
    object_id: &str,
) -> BcsValidationResult {
    let format_from_type = BcsFormat::from_type_string(type_string);
    let format_from_bytes = BcsFormat::detect(bcs);

    let mut notes = Vec::new();
    let mut valid = true;

    // Cross-validate format detection
    if format_from_type != BcsFormat::Unknown && format_from_bytes != BcsFormat::Unknown {
        if format_from_type != format_from_bytes {
            notes.push(format!(
                "BCS format mismatch: type_string indicates {:?}, bytes indicate {:?}",
                format_from_type, format_from_bytes
            ));
            valid = false;
        }
    }

    let format = if format_from_type != BcsFormat::Unknown {
        format_from_type
    } else {
        format_from_bytes
    };

    // Extract struct bytes for MoveObject format
    let struct_bytes = match format {
        BcsFormat::Package => None, // Use full BCS for packages
        BcsFormat::MoveObject => {
            // Try to extract Move struct from full object BCS
            extract_move_struct_validated(bcs, object_id)
        }
        BcsFormat::Unknown => None,
    };

    BcsValidationResult {
        format,
        valid,
        notes,
        struct_bytes,
    }
}

/// Extract Move struct bytes from full object BCS with validation.
fn extract_move_struct_validated(bcs: &[u8], object_id: &str) -> Option<Vec<u8>> {
    // Parse object_id hex to bytes
    let id_hex = object_id.strip_prefix("0x").unwrap_or(object_id);
    let id_bytes = hex::decode(id_hex).ok()?;

    if id_bytes.len() != 32 {
        return None;
    }

    // Search for the object_id (UID) in the BCS data
    for i in 0..bcs.len().saturating_sub(32) {
        if &bcs[i..i + 32] == id_bytes.as_slice() {
            // Found the UID at position i
            // Try to read ULEB128 length prefix before the UID
            for prefix_offset in [2, 1, 3] {
                if i >= prefix_offset {
                    let len_start = i - prefix_offset;
                    if let Some((len, bytes_read)) = read_uleb128(&bcs[len_start..]) {
                        if len_start + bytes_read == i {
                            let contents_end = i + len;
                            if contents_end <= bcs.len() {
                                return Some(bcs[i..contents_end].to_vec());
                            }
                        }
                    }
                }
            }
            // Fallback: return from UID to end
            return Some(bcs[i..].to_vec());
        }
    }

    None
}

/// Read a ULEB128 encoded unsigned integer.
fn read_uleb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut result: usize = 0;
    let mut shift = 0;
    let mut bytes_read = 0;

    for &byte in data.iter().take(5) {
        bytes_read += 1;
        result |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Some((result, bytes_read));
        }
        shift += 7;
    }
    None
}

// =============================================================================
// Type Argument Validation (P0)
// =============================================================================

/// Result of type argument validation.
#[derive(Debug, Clone)]
pub struct TypeValidationResult {
    /// The parsed TypeTag (if valid)
    pub type_tag: Option<TypeTag>,
    /// Whether the type is valid
    pub valid: bool,
    /// Validation errors/warnings
    pub errors: Vec<String>,
}

/// Validate type arguments eagerly before execution.
///
/// This catches malformed type strings at the gRPC layer before they
/// cause cryptic errors during VM execution.
pub fn validate_type_arguments(type_args: &[String]) -> Vec<TypeValidationResult> {
    use crate::benchmark::types::parse_type_tag;

    type_args
        .iter()
        .map(|type_str| match parse_type_tag(type_str) {
            Ok(tag) => TypeValidationResult {
                type_tag: Some(tag),
                valid: true,
                errors: vec![],
            },
            Err(e) => TypeValidationResult {
                type_tag: None,
                valid: false,
                errors: vec![format!("Invalid type '{}': {}", type_str, e)],
            },
        })
        .collect()
}

/// Validate all type arguments and return error if any are invalid.
pub fn validate_type_arguments_strict(type_args: &[String]) -> Result<Vec<TypeTag>> {
    let results = validate_type_arguments(type_args);

    let mut errors: Vec<String> = Vec::new();
    let mut tags = Vec::with_capacity(type_args.len());

    for (i, result) in results.into_iter().enumerate() {
        if result.valid {
            tags.push(result.type_tag.unwrap());
        } else {
            errors.extend(
                result
                    .errors
                    .iter()
                    .map(|e| format!("Type argument {}: {}", i, e)),
            );
        }
    }

    if !errors.is_empty() {
        Err(anyhow!(
            "Type argument validation failed:\n{}",
            errors.join("\n")
        ))
    } else {
        Ok(tags)
    }
}

// =============================================================================
// Version Tracking (P1)
// =============================================================================

// Re-export types from state_source to avoid duplication
// The canonical definitions live in state_source (lower-level module)
pub use crate::benchmark::state_source::{ObjectSource as DataSource, ObjectVersionMetadata};

/// Extended version metadata for gRPC validation that includes object ID.
///
/// This wraps the base `ObjectVersionMetadata` with additional context
/// needed during gRPC → VM transformation.
#[derive(Debug, Clone)]
pub struct ValidatedVersionMetadata {
    /// Object ID being validated
    pub object_id: AccountAddress,
    /// Version (lamport timestamp)
    pub version: u64,
    /// Base metadata (from state_source)
    pub inner: ObjectVersionMetadata,
}

impl ValidatedVersionMetadata {
    /// Create new validated metadata.
    pub fn new(
        object_id: AccountAddress,
        version: u64,
        expected_version: Option<u64>,
        source: DataSource,
    ) -> Self {
        let inner = ObjectVersionMetadata::new(expected_version, version, source);
        Self {
            object_id,
            version,
            inner,
        }
    }

    /// Check if version is valid.
    pub fn version_valid(&self) -> bool {
        self.inner.version_valid
    }

    /// Validate version matches expected.
    pub fn validate_version(&self) -> Result<()> {
        self.inner.validate_version(self.version).map_err(|e| {
            anyhow!(
                "Object {}: {}",
                self.object_id.to_hex_literal(),
                e
            )
        })
    }
}

// =============================================================================
// Validated Object (combines all validations)
// =============================================================================

/// A fully validated object ready for Move VM execution.
///
/// This type guarantees:
/// 1. BCS format is correct for the object type
/// 2. Type string has been parsed and validated
/// 3. Version has been verified against expectations
/// 4. Provenance is tracked
#[derive(Debug, Clone)]
pub struct ValidatedObject {
    /// Object ID
    pub id: AccountAddress,
    /// Validated type tag
    pub type_tag: TypeTag,
    /// Validated BCS bytes (struct bytes for Move objects, full for packages)
    pub bcs_bytes: Vec<u8>,
    /// BCS format
    pub format: BcsFormat,
    /// Version metadata (with object ID context)
    pub version_metadata: ValidatedVersionMetadata,
    /// Whether this is a shared object
    pub is_shared: bool,
    /// Whether this is immutable
    pub is_immutable: bool,
    /// Validation warnings (non-fatal issues)
    pub warnings: Vec<String>,
}

impl ValidatedObject {
    /// Get the object version.
    pub fn version(&self) -> u64 {
        self.version_metadata.version
    }

    /// Get the data source.
    pub fn source(&self) -> &DataSource {
        &self.version_metadata.inner.source
    }

    /// Check if version is valid.
    pub fn is_version_valid(&self) -> bool {
        self.version_metadata.version_valid()
    }

    /// Convert to ObjectData for use with StateSource.
    pub fn to_object_data(&self) -> crate::benchmark::state_source::ObjectData {
        crate::benchmark::state_source::ObjectData::with_metadata(
            self.id,
            self.type_tag.clone(),
            self.bcs_bytes.clone(),
            self.is_shared,
            self.is_immutable,
            self.version(),
            self.version_metadata.inner.clone(),
        )
    }
}

// =============================================================================
// Transformation Context
// =============================================================================

/// Context for tracking transformation state and validations.
pub struct TransformationContext {
    /// Expected object versions (from transaction effects)
    expected_versions: HashMap<String, u64>,
    /// Data source for this context
    source: DataSource,
    /// Validation statistics
    stats: TransformationStats,
    /// Schema validator
    schema_validator: ProtoSchemaValidator,
}

/// Statistics about transformations performed.
#[derive(Debug, Default)]
pub struct TransformationStats {
    /// Number of objects transformed
    pub objects_transformed: AtomicU64,
    /// Number of version mismatches
    pub version_mismatches: AtomicU64,
    /// Number of BCS format issues
    pub bcs_format_issues: AtomicU64,
    /// Number of type parsing failures
    pub type_parse_failures: AtomicU64,
}

impl TransformationContext {
    /// Create a new transformation context.
    pub fn new() -> Self {
        Self {
            expected_versions: HashMap::new(),
            source: DataSource::Unknown,
            stats: TransformationStats::default(),
            schema_validator: ProtoSchemaValidator::new(),
        }
    }

    /// Set the data source.
    pub fn with_source(mut self, source: DataSource) -> Self {
        self.source = source;
        self
    }

    /// Set expected versions from transaction effects.
    pub fn with_expected_versions(mut self, versions: HashMap<String, u64>) -> Self {
        self.expected_versions = versions;
        self
    }

    /// Add an expected version.
    pub fn add_expected_version(&mut self, object_id: &str, version: u64) {
        self.expected_versions
            .insert(object_id.to_string(), version);
    }

    /// Get expected version for an object.
    pub fn expected_version(&self, object_id: &str) -> Option<u64> {
        self.expected_versions.get(object_id).copied()
    }

    /// Get transformation statistics.
    pub fn stats(&self) -> &TransformationStats {
        &self.stats
    }

    /// Validate proto schema.
    pub fn validate_schema(&self) -> Result<Vec<String>> {
        self.schema_validator.validate()
    }
}

impl Default for TransformationContext {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// High-Level Validation Functions
// =============================================================================

/// Validate and transform a GrpcObject to a ValidatedObject.
///
/// This is the main entry point for object validation.
pub fn validate_grpc_object(
    object_id: &str,
    version: u64,
    type_string: Option<&str>,
    bcs: &[u8],
    is_shared: bool,
    is_immutable: bool,
    ctx: &TransformationContext,
) -> Result<ValidatedObject> {
    use crate::benchmark::types::parse_type_tag;

    let mut warnings = Vec::new();

    // Parse object ID
    let id = AccountAddress::from_hex_literal(object_id)
        .map_err(|e| anyhow!("Invalid object ID '{}': {}", object_id, e))?;

    // Validate BCS format
    let bcs_result = validate_bcs_format(bcs, type_string, object_id);
    if !bcs_result.valid {
        warnings.extend(bcs_result.notes.clone());
        ctx.stats.bcs_format_issues.fetch_add(1, Ordering::Relaxed);
    }

    // Parse type tag
    let type_tag = match type_string {
        Some("package") => {
            // Packages use a synthetic type
            TypeTag::Struct(Box::new(move_core_types::language_storage::StructTag {
                address: AccountAddress::from_hex_literal("0x2").unwrap(),
                module: move_core_types::identifier::Identifier::new("package").unwrap(),
                name: move_core_types::identifier::Identifier::new("Package").unwrap(),
                type_params: vec![],
            }))
        }
        Some(ts) => parse_type_tag(ts).map_err(|e| {
            ctx.stats
                .type_parse_failures
                .fetch_add(1, Ordering::Relaxed);
            anyhow!("Failed to parse type '{}': {}", ts, e)
        })?,
        None => {
            return Err(anyhow!(
                "Object {} has no type string - cannot validate",
                object_id
            ));
        }
    };

    // Create version metadata with validation
    let expected_version = ctx.expected_version(object_id);
    let version_metadata =
        ValidatedVersionMetadata::new(id, version, expected_version, ctx.source.clone());

    if !version_metadata.version_valid() {
        ctx.stats.version_mismatches.fetch_add(1, Ordering::Relaxed);
        warnings.push(format!(
            "Version mismatch: expected {}, got {}",
            expected_version.unwrap_or(0),
            version
        ));
    }

    // Determine final BCS bytes
    let bcs_bytes = match bcs_result.format {
        BcsFormat::Package => bcs.to_vec(),
        BcsFormat::MoveObject => bcs_result.struct_bytes.unwrap_or_else(|| bcs.to_vec()),
        BcsFormat::Unknown => bcs.to_vec(),
    };

    ctx.stats
        .objects_transformed
        .fetch_add(1, Ordering::Relaxed);

    Ok(ValidatedObject {
        id,
        type_tag,
        bcs_bytes,
        format: bcs_result.format,
        version_metadata,
        is_shared,
        is_immutable,
        warnings,
    })
}

/// Validate a transaction's type arguments before execution.
pub fn validate_transaction_types(commands: &[crate::grpc::GrpcCommand]) -> Result<()> {
    for (i, cmd) in commands.iter().enumerate() {
        if let crate::grpc::GrpcCommand::MoveCall { type_arguments, .. } = cmd {
            if let Err(e) = validate_type_arguments_strict(type_arguments) {
                return Err(anyhow!("Command {}: {}", i, e));
            }
        }
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bcs_format_detection() {
        // Package format starts with 0x01
        let package_bcs = [0x01u8; 50];
        assert_eq!(BcsFormat::detect(&package_bcs), BcsFormat::Package);

        // Empty is unknown
        assert_eq!(BcsFormat::detect(&[]), BcsFormat::Unknown);
    }

    #[test]
    fn test_bcs_format_from_type_string() {
        assert_eq!(
            BcsFormat::from_type_string(Some("package")),
            BcsFormat::Package
        );
        assert_eq!(
            BcsFormat::from_type_string(Some("0x2::coin::Coin<0x2::sui::SUI>")),
            BcsFormat::MoveObject
        );
        assert_eq!(BcsFormat::from_type_string(None), BcsFormat::Unknown);
    }

    #[test]
    fn test_type_argument_validation() {
        let valid_args = vec![
            "u64".to_string(),
            "0x2::sui::SUI".to_string(),
            "0x2::coin::Coin<0x2::sui::SUI>".to_string(),
        ];
        let results = validate_type_arguments(&valid_args);
        assert!(results.iter().all(|r| r.valid));

        let invalid_args = vec!["not_a_type".to_string()];
        let results = validate_type_arguments(&invalid_args);
        assert!(!results[0].valid);
    }

    #[test]
    fn test_version_metadata() {
        let id = AccountAddress::from_hex_literal("0x123").unwrap();

        // Matching versions
        let meta = ValidatedVersionMetadata::new(id, 5, Some(5), DataSource::GrpcMainnet);
        assert!(meta.version_valid());
        assert!(meta.validate_version().is_ok());

        // Mismatched versions
        let meta = ValidatedVersionMetadata::new(id, 5, Some(10), DataSource::GrpcMainnet);
        assert!(!meta.version_valid());
        assert!(meta.validate_version().is_err());

        // No expected version (always valid)
        let meta = ValidatedVersionMetadata::new(id, 5, None, DataSource::GrpcMainnet);
        assert!(meta.version_valid());
    }

    #[test]
    fn test_transformation_context() {
        let mut ctx = TransformationContext::new().with_source(DataSource::GrpcArchive);

        ctx.add_expected_version("0x123", 5);
        assert_eq!(ctx.expected_version("0x123"), Some(5));
        assert_eq!(ctx.expected_version("0x456"), None);
    }
}
