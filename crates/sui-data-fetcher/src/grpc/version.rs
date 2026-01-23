//! # Version Compatibility for gRPC and Sui Client
//!
//! This module handles version compatibility between:
//! 1. **Proto schema versions** - The gRPC proto definitions (vendored in `/proto/`)
//! 2. **Sui client versions** - The Move VM and related crates (pinned in Cargo.toml)
//! 3. **Network protocol versions** - Mainnet/testnet protocol compatibility
//!
//! ## Upgrade Process
//!
//! When upgrading Sui/gRPC versions, follow this process:
//!
//! ### 1. Update Proto Definitions
//! ```bash
//! # Fetch latest protos from Sui repository
//! ./scripts/update-protos.sh mainnet-v1.XX.X
//!
//! # Regenerate Rust code
//! cargo build
//! ```
//!
//! ### 2. Update Cargo.toml Dependencies
//! Change all `tag = "mainnet-v1.XX.X"` entries to the new version.
//!
//! ### 3. Update Version Constants
//! Update `PINNED_SUI_VERSION` and `PROTO_SCHEMA_VERSION` in this file.
//!
//! ### 4. Update Framework Bytecode
//! ```bash
//! # Rebuild framework modules with new sui binary
//! docker build -t sui-extractor .
//! docker run sui-extractor cat /framework_bytecode/move-stdlib > framework_bytecode/move-stdlib
//! # ... repeat for sui-framework, sui-system
//! ```
//!
//! ### 5. Run Compatibility Tests
//! ```bash
//! cargo test version_compat
//! ```

use anyhow::{anyhow, Result};
use std::sync::OnceLock;

// =============================================================================
// Version Constants
// =============================================================================

/// The Sui version that Cargo.toml dependencies are pinned to.
/// This MUST match the `tag = "mainnet-vX.XX.X"` in Cargo.toml.
pub const PINNED_SUI_VERSION: &str = "mainnet-v1.63.3";

/// Proto schema version identifier.
/// Format: "sui.rpc.v{major}" where major is from the proto package name.
pub const PROTO_SCHEMA_VERSION: &str = "sui.rpc.v2";

/// Minimum supported protocol version for mainnet compatibility.
pub const MIN_PROTOCOL_VERSION: u64 = 50;

/// Maximum tested protocol version.
pub const MAX_TESTED_PROTOCOL_VERSION: u64 = 80;

// =============================================================================
// Version Info
// =============================================================================

/// Complete version information for the extractor.
#[derive(Debug, Clone)]
pub struct VersionInfo {
    /// Crate version from Cargo.toml
    pub crate_version: &'static str,
    /// Pinned Sui version for Move VM crates
    pub sui_version: &'static str,
    /// Proto schema version
    pub proto_version: &'static str,
    /// Git commit hash (if available)
    pub git_commit: Option<&'static str>,
    /// Build timestamp
    pub build_timestamp: Option<&'static str>,
}

impl VersionInfo {
    /// Get the current version info.
    pub fn current() -> &'static Self {
        static INFO: OnceLock<VersionInfo> = OnceLock::new();
        INFO.get_or_init(|| VersionInfo {
            crate_version: env!("CARGO_PKG_VERSION"),
            sui_version: PINNED_SUI_VERSION,
            proto_version: PROTO_SCHEMA_VERSION,
            git_commit: option_env!("GIT_COMMIT"),
            build_timestamp: option_env!("BUILD_TIMESTAMP"),
        })
    }

    /// Format as a user-friendly string.
    pub fn display(&self) -> String {
        format!(
            "sui-move-extractor v{} (sui: {}, proto: {})",
            self.crate_version, self.sui_version, self.proto_version
        )
    }
}

// =============================================================================
// Compatibility Matrix
// =============================================================================

/// Known compatibility between Sui versions and proto schemas.
#[derive(Debug, Clone)]
pub struct CompatibilityEntry {
    /// Sui mainnet version tag
    pub sui_version: &'static str,
    /// Compatible proto schema version
    pub proto_version: &'static str,
    /// Protocol version range (min, max)
    pub protocol_range: (u64, u64),
    /// Known breaking changes from previous version
    pub breaking_changes: &'static [&'static str],
}

/// Get the compatibility matrix for known versions.
pub fn compatibility_matrix() -> &'static [CompatibilityEntry] {
    static MATRIX: &[CompatibilityEntry] = &[
        CompatibilityEntry {
            sui_version: "mainnet-v1.63.3",
            proto_version: "sui.rpc.v2",
            protocol_range: (75, 80),
            breaking_changes: &[],
        },
        CompatibilityEntry {
            sui_version: "mainnet-v1.62.1",
            proto_version: "sui.rpc.v2",
            protocol_range: (70, 75),
            breaking_changes: &[],
        },
        CompatibilityEntry {
            sui_version: "mainnet-v1.60.0",
            proto_version: "sui.rpc.v2",
            protocol_range: (65, 70),
            breaking_changes: &["Effects.unchanged_loaded_runtime_objects added"],
        },
        CompatibilityEntry {
            sui_version: "mainnet-v1.55.0",
            proto_version: "sui.rpc.v2",
            protocol_range: (60, 65),
            breaking_changes: &["Object.bcs field format changed"],
        },
    ];
    MATRIX
}

/// Find the compatibility entry for a given Sui version.
pub fn find_compatibility(sui_version: &str) -> Option<&'static CompatibilityEntry> {
    compatibility_matrix()
        .iter()
        .find(|e| e.sui_version == sui_version)
}

// =============================================================================
// Runtime Compatibility Check
// =============================================================================

/// Result of a compatibility check.
#[derive(Debug, Clone)]
pub struct CompatibilityResult {
    /// Whether the versions are compatible
    pub compatible: bool,
    /// Warning messages (non-fatal issues)
    pub warnings: Vec<String>,
    /// Error messages (fatal issues)
    pub errors: Vec<String>,
    /// Detected network protocol version
    pub protocol_version: Option<u64>,
}

impl CompatibilityResult {
    fn ok() -> Self {
        Self {
            compatible: true,
            warnings: vec![],
            errors: vec![],
            protocol_version: None,
        }
    }

    fn with_warning(mut self, msg: impl Into<String>) -> Self {
        self.warnings.push(msg.into());
        self
    }

    fn with_error(mut self, msg: impl Into<String>) -> Self {
        self.errors.push(msg.into());
        self.compatible = false;
        self
    }

    fn with_protocol(mut self, version: u64) -> Self {
        self.protocol_version = Some(version);
        self
    }

    /// Convert to Result, failing if not compatible.
    pub fn into_result(self) -> Result<Vec<String>> {
        if self.compatible {
            Ok(self.warnings)
        } else {
            Err(anyhow!(
                "Version compatibility check failed:\n{}",
                self.errors.join("\n")
            ))
        }
    }
}

/// Check compatibility with a network's protocol version.
pub fn check_protocol_compatibility(protocol_version: u64) -> CompatibilityResult {
    let mut result = CompatibilityResult::ok().with_protocol(protocol_version);

    if protocol_version < MIN_PROTOCOL_VERSION {
        result = result.with_error(format!(
            "Protocol version {} is below minimum supported version {}. \
             This crate requires a newer network.",
            protocol_version, MIN_PROTOCOL_VERSION
        ));
    } else if protocol_version > MAX_TESTED_PROTOCOL_VERSION {
        result = result.with_warning(format!(
            "Protocol version {} is newer than the maximum tested version {}. \
             Some features may not work correctly. Consider upgrading the extractor.",
            protocol_version, MAX_TESTED_PROTOCOL_VERSION
        ));
    }

    result
}

/// Check compatibility between our pinned version and a service's reported version.
pub fn check_service_compatibility(
    service_chain_id: &str,
    service_epoch: u64,
    service_checkpoint: u64,
) -> CompatibilityResult {
    let mut result = CompatibilityResult::ok();

    // Validate chain ID format
    if service_chain_id.is_empty() {
        result = result.with_warning("Service did not report chain ID");
    }

    // Check for known mainnet/testnet chain IDs
    let known_chains = ["35834a8a", "4c78adac"]; // mainnet, testnet
    if !service_chain_id.is_empty() && !known_chains.iter().any(|c| service_chain_id.contains(c)) {
        result = result.with_warning(format!(
            "Unknown chain ID '{}'. This may be a custom network.",
            service_chain_id
        ));
    }

    // Basic sanity checks
    if service_epoch == 0 {
        result = result.with_warning("Service reports epoch 0 - this may be a fresh network");
    }

    if service_checkpoint == 0 {
        result = result.with_warning("Service reports checkpoint 0 - this may be a fresh network");
    }

    result
}

// =============================================================================
// Proto Schema Validation
// =============================================================================

/// Hash of critical proto message structures for detecting schema drift.
///
/// These are computed at build time and compared against runtime parsing
/// to detect incompatible schema changes.
#[derive(Debug, Clone)]
pub struct ProtoSchemaHashes {
    /// Hash of Object message structure
    pub object_hash: &'static str,
    /// Hash of Transaction message structure
    pub transaction_hash: &'static str,
    /// Hash of Effects message structure
    pub effects_hash: &'static str,
}

impl ProtoSchemaHashes {
    /// Get the expected hashes for the current proto version.
    pub fn expected() -> Self {
        // These would ideally be computed at build time from the proto files
        // For now, we use version strings as placeholders
        Self {
            object_hash: "v2.2025.01",
            transaction_hash: "v2.2025.01",
            effects_hash: "v2.2025.01",
        }
    }
}

/// Validate that proto parsing produces expected results.
///
/// This catches cases where the proto definitions have drifted from
/// what the service actually sends.
pub fn validate_proto_parsing<T: prost::Message + Default>(
    message_name: &str,
    sample_bytes: &[u8],
) -> Result<()> {
    // Try to parse the bytes
    let result = T::decode(sample_bytes);

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(anyhow!(
            "Proto parsing failed for {}: {}. \
             This may indicate a schema version mismatch. \
             Current proto version: {}, pinned Sui version: {}",
            message_name,
            e,
            PROTO_SCHEMA_VERSION,
            PINNED_SUI_VERSION
        )),
    }
}

// =============================================================================
// Upgrade Helpers
// =============================================================================

/// Information needed to upgrade to a new Sui version.
#[derive(Debug)]
pub struct UpgradeChecklist {
    /// Current version
    pub from_version: String,
    /// Target version
    pub to_version: String,
    /// Files that need to be updated
    pub files_to_update: Vec<String>,
    /// Commands to run
    pub commands: Vec<String>,
    /// Breaking changes to be aware of
    pub breaking_changes: Vec<String>,
}

/// Generate an upgrade checklist for moving to a new Sui version.
pub fn generate_upgrade_checklist(to_version: &str) -> UpgradeChecklist {
    let from_version = PINNED_SUI_VERSION.to_string();

    UpgradeChecklist {
        from_version: from_version.clone(),
        to_version: to_version.to_string(),
        files_to_update: vec![
            "Cargo.toml - Update all 'tag = \"mainnet-vX.XX.X\"' entries".to_string(),
            "src/grpc/version.rs - Update PINNED_SUI_VERSION constant".to_string(),
            "Dockerfile - Update SUI_VERSION ARG".to_string(),
            "framework_bytecode/* - Regenerate from new sui binary".to_string(),
        ],
        commands: vec![
            format!(
                "# 1. Update proto definitions\n\
                 git clone --depth 1 --branch {} https://github.com/MystenLabs/sui /tmp/sui\n\
                 cp -r /tmp/sui/crates/sui-rpc-api/proto/sui proto/",
                to_version
            ),
            "# 2. Regenerate proto Rust code\n\
             cargo build"
                .to_string(),
            format!(
                "# 3. Update Cargo.toml\n\
                 sed -i 's/{}/{}/g' Cargo.toml",
                from_version, to_version
            ),
            "# 4. Rebuild framework bytecode\n\
             docker build -t sui-extractor .\n\
             docker run --rm sui-extractor cat /framework_bytecode/move-stdlib > framework_bytecode/move-stdlib\n\
             docker run --rm sui-extractor cat /framework_bytecode/sui-framework > framework_bytecode/sui-framework\n\
             docker run --rm sui-extractor cat /framework_bytecode/sui-system > framework_bytecode/sui-system"
                .to_string(),
            "# 5. Run tests\n\
             cargo test"
                .to_string(),
        ],
        breaking_changes: find_compatibility(to_version)
            .map(|e| e.breaking_changes.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default(),
    }
}

// =============================================================================
// Feature Detection
// =============================================================================

/// Features that may or may not be available depending on version.
#[derive(Debug, Clone, Default)]
pub struct FeatureFlags {
    /// Whether unchanged_loaded_runtime_objects is available in Effects
    pub has_unchanged_runtime_objects: bool,
    /// Whether simulation endpoint supports all PTB commands
    pub has_full_simulation: bool,
    /// Whether the bcs field contains full object BCS
    pub has_full_object_bcs: bool,
}

impl FeatureFlags {
    /// Detect features based on protocol version.
    pub fn from_protocol_version(protocol_version: u64) -> Self {
        Self {
            // Added around protocol version 65
            has_unchanged_runtime_objects: protocol_version >= 65,
            // Full simulation support added in v68
            has_full_simulation: protocol_version >= 68,
            // Full object BCS in response added in v60
            has_full_object_bcs: protocol_version >= 60,
        }
    }

    /// Detect features by probing service capabilities.
    pub async fn detect_from_service(client: &crate::grpc::GrpcClient) -> Result<Self> {
        // Probe service to verify connectivity (result unused for now)
        let _info = client.get_service_info().await?;

        // Get protocol version from service
        // For now, assume latest features if we can connect
        Ok(Self {
            has_unchanged_runtime_objects: true,
            has_full_simulation: true,
            has_full_object_bcs: true,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_info() {
        let info = VersionInfo::current();
        assert!(!info.crate_version.is_empty());
        assert_eq!(info.sui_version, PINNED_SUI_VERSION);
        assert_eq!(info.proto_version, PROTO_SCHEMA_VERSION);
    }

    #[test]
    fn test_protocol_compatibility() {
        // Too old
        let result = check_protocol_compatibility(40);
        assert!(!result.compatible);

        // Supported
        let result = check_protocol_compatibility(70);
        assert!(result.compatible);
        assert!(result.warnings.is_empty());

        // Newer than tested
        let result = check_protocol_compatibility(100);
        assert!(result.compatible); // Still compatible, just warns
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_compatibility_matrix() {
        let matrix = compatibility_matrix();
        assert!(!matrix.is_empty());

        // Current version should be in matrix
        let current = find_compatibility(PINNED_SUI_VERSION);
        assert!(current.is_some());
    }

    #[test]
    fn test_upgrade_checklist() {
        let checklist = generate_upgrade_checklist("mainnet-v1.70.0");
        assert_eq!(checklist.from_version, PINNED_SUI_VERSION);
        assert_eq!(checklist.to_version, "mainnet-v1.70.0");
        assert!(!checklist.files_to_update.is_empty());
        assert!(!checklist.commands.is_empty());
    }

    #[test]
    fn test_feature_flags() {
        let old = FeatureFlags::from_protocol_version(55);
        assert!(!old.has_unchanged_runtime_objects);
        assert!(!old.has_full_simulation);

        let new = FeatureFlags::from_protocol_version(70);
        assert!(new.has_unchanged_runtime_objects);
        assert!(new.has_full_simulation);
        assert!(new.has_full_object_bcs);
    }
}
