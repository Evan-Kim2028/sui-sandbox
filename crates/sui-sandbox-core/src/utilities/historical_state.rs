//! Historical State Reconstruction Facade
//!
//! This module provides a high-level API for reconstructing historical state
//! for transaction replay. It orchestrates:
//!
//! - Historical bytecode resolution
//! - Version field detection
//! - Object patching with fallbacks
//!
//! ## Usage
//!
//! ```ignore
//! use sui_sandbox_core::utilities::HistoricalStateReconstructor;
//!
//! let mut reconstructor = HistoricalStateReconstructor::new();
//!
//! // Optional: register manual override for problematic objects
//! reconstructor.register_override(ManualPatchOverride {
//!     object_id: "0xdaa46292...".to_string(),
//!     patches: vec![(32, 3u64.to_le_bytes().to_vec())],
//! });
//!
//! // Configure from bytecode (detects versions)
//! reconstructor.configure_from_modules(modules.iter());
//!
//! // Patch objects for replay
//! let patched_objects = reconstructor.patch_objects(&objects, &types, timestamp_ms);
//! ```

use std::collections::HashMap;

use super::enhanced_patcher::{
    EnhancedObjectPatcher, FailureStrategy, ManualPatchOverride, PatchingStats,
};
use super::version_utils::detect_version_constants;

/// Configuration for historical state reconstruction.
#[derive(Debug, Clone)]
pub struct ReconstructionConfig {
    /// How to handle patching failures
    pub failure_strategy: FailureStrategy,
    /// Whether to auto-register well-known patches
    pub auto_register_well_known: bool,
    /// Whether to add default patching rules
    pub add_default_rules: bool,
}

impl Default for ReconstructionConfig {
    fn default() -> Self {
        Self {
            failure_strategy: FailureStrategy::WarnAndSkip,
            auto_register_well_known: true,
            add_default_rules: true,
        }
    }
}

/// Result of historical state reconstruction.
#[derive(Debug)]
pub struct ReconstructedState {
    /// Patched objects: object_id -> patched BCS bytes
    pub objects: HashMap<String, Vec<u8>>,
    /// Statistics about patching operations
    pub stats: PatchingStats,
    /// Detected version constants from bytecode
    pub detected_versions: HashMap<String, u64>,
}

/// High-level facade for historical state reconstruction.
///
/// This orchestrates bytecode resolution, version detection, and object patching
/// to prepare state for historical transaction replay.
pub struct HistoricalStateReconstructor {
    /// Enhanced object patcher with fallback strategies
    patcher: EnhancedObjectPatcher,
    /// Configuration
    config: ReconstructionConfig,
    /// Transaction timestamp (for time-based patches)
    timestamp_ms: Option<u64>,
    /// Whether modules have been configured
    modules_configured: bool,
}

impl HistoricalStateReconstructor {
    /// Create a new reconstructor with default configuration.
    pub fn new() -> Self {
        Self::with_config(ReconstructionConfig::default())
    }

    /// Create a new reconstructor with custom configuration.
    pub fn with_config(config: ReconstructionConfig) -> Self {
        let mut patcher =
            EnhancedObjectPatcher::new().with_failure_strategy(config.failure_strategy);

        if config.add_default_rules {
            patcher.add_default_rules();
        }

        Self {
            patcher,
            config,
            timestamp_ms: None,
            modules_configured: false,
        }
    }

    /// Set the transaction timestamp for time-based patches.
    pub fn set_timestamp(&mut self, timestamp_ms: u64) {
        self.timestamp_ms = Some(timestamp_ms);
        self.patcher.set_timestamp(timestamp_ms);
    }

    /// Register a manual patch override.
    pub fn register_override(&mut self, override_spec: ManualPatchOverride) {
        self.patcher.register_override(override_spec);
    }

    /// Register a version for a specific package.
    pub fn register_version(&mut self, package_addr: &str, version: u64) {
        self.patcher.register_version(package_addr, version);
    }

    /// Add a raw byte patch for a type pattern.
    pub fn add_raw_patch(&mut self, type_pattern: &str, offset: usize, value: &[u8]) {
        self.patcher.add_raw_patch(type_pattern, offset, value);
    }

    /// Convenience method to add a raw u64 patch.
    pub fn add_raw_u64_patch(&mut self, type_pattern: &str, offset: usize, value: u64) {
        self.patcher.add_raw_u64_patch(type_pattern, offset, value);
    }

    /// Configure from compiled modules.
    ///
    /// This:
    /// 1. Adds modules for struct layout extraction
    /// 2. Scans bytecode for version constants
    /// 3. Optionally registers well-known patches
    pub fn configure_from_modules<'a>(
        &mut self,
        modules: impl Iterator<Item = &'a move_binary_format::file_format::CompiledModule>,
    ) {
        // Collect modules into a Vec so we can iterate twice
        let modules_vec: Vec<_> = modules.collect();

        // Add modules for layout extraction
        self.patcher.add_modules(modules_vec.iter().copied());

        // Detect version constants from bytecode
        let versions = detect_version_constants(modules_vec.into_iter());

        // Register detected versions
        for (pkg_addr, version) in &versions {
            self.patcher.register_version(pkg_addr, *version);
        }

        // Auto-register well-known patches if configured
        if self.config.auto_register_well_known {
            self.patcher.auto_register_well_known_patches();
        }

        self.modules_configured = true;
    }

    /// Get detected version constants.
    pub fn detected_versions(&self) -> &HashMap<String, u64> {
        self.patcher.detected_versions()
    }

    /// Patch a single object.
    pub fn patch_object(&mut self, object_id: &str, type_str: &str, bcs_bytes: &[u8]) -> Vec<u8> {
        self.patcher.patch_object(object_id, type_str, bcs_bytes)
    }

    /// Patch multiple objects.
    ///
    /// Takes:
    /// - `objects`: Map of object_id -> raw BCS bytes
    /// - `types`: Map of object_id -> type string
    ///
    /// Returns map of object_id -> patched BCS bytes.
    pub fn patch_objects(
        &mut self,
        objects: &HashMap<String, Vec<u8>>,
        types: &HashMap<String, String>,
    ) -> HashMap<String, Vec<u8>> {
        let mut patched = HashMap::new();

        for (obj_id, bcs_bytes) in objects {
            let type_str = types.get(obj_id).map(|s| s.as_str()).unwrap_or("");
            let patched_bcs = self.patcher.patch_object(obj_id, type_str, bcs_bytes);
            patched.insert(obj_id.clone(), patched_bcs);
        }

        patched
    }

    /// Perform full reconstruction and return results.
    ///
    /// This is a convenience method that patches all objects and returns
    /// comprehensive results including statistics.
    pub fn reconstruct(
        &mut self,
        objects: &HashMap<String, Vec<u8>>,
        types: &HashMap<String, String>,
    ) -> ReconstructedState {
        // Reset stats before reconstruction
        self.patcher.reset_stats();

        // Patch all objects
        let patched_objects = self.patch_objects(objects, types);

        ReconstructedState {
            objects: patched_objects,
            stats: self.patcher.stats().clone(),
            detected_versions: self.patcher.detected_versions().clone(),
        }
    }

    /// Get patching statistics.
    pub fn stats(&self) -> &PatchingStats {
        self.patcher.stats()
    }

    /// Get mutable access to the underlying patcher.
    ///
    /// Useful for advanced configuration.
    pub fn patcher_mut(&mut self) -> &mut EnhancedObjectPatcher {
        &mut self.patcher
    }

    /// Get the underlying patcher.
    pub fn patcher(&self) -> &EnhancedObjectPatcher {
        &self.patcher
    }
}

impl Default for HistoricalStateReconstructor {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for HistoricalStateReconstructor.
pub struct ReconstructorBuilder {
    config: ReconstructionConfig,
    overrides: Vec<ManualPatchOverride>,
    versions: HashMap<String, u64>,
    raw_patches: Vec<(String, usize, Vec<u8>)>,
    timestamp_ms: Option<u64>,
}

impl ReconstructorBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            config: ReconstructionConfig::default(),
            overrides: Vec::new(),
            versions: HashMap::new(),
            raw_patches: Vec::new(),
            timestamp_ms: None,
        }
    }

    /// Set the failure strategy.
    pub fn failure_strategy(mut self, strategy: FailureStrategy) -> Self {
        self.config.failure_strategy = strategy;
        self
    }

    /// Disable auto-registration of well-known patches.
    pub fn no_auto_well_known(mut self) -> Self {
        self.config.auto_register_well_known = false;
        self
    }

    /// Disable default patching rules.
    pub fn no_default_rules(mut self) -> Self {
        self.config.add_default_rules = false;
        self
    }

    /// Add a manual override.
    pub fn with_override(mut self, override_spec: ManualPatchOverride) -> Self {
        self.overrides.push(override_spec);
        self
    }

    /// Register a version.
    pub fn with_version(mut self, package_addr: &str, version: u64) -> Self {
        self.versions.insert(package_addr.to_string(), version);
        self
    }

    /// Add a raw patch.
    pub fn with_raw_patch(mut self, type_pattern: &str, offset: usize, value: Vec<u8>) -> Self {
        self.raw_patches
            .push((type_pattern.to_string(), offset, value));
        self
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp_ms: u64) -> Self {
        self.timestamp_ms = Some(timestamp_ms);
        self
    }

    /// Build the reconstructor.
    pub fn build(self) -> HistoricalStateReconstructor {
        let mut reconstructor = HistoricalStateReconstructor::with_config(self.config);

        for override_spec in self.overrides {
            reconstructor.register_override(override_spec);
        }

        for (addr, version) in self.versions {
            reconstructor.register_version(&addr, version);
        }

        for (type_pattern, offset, value) in self.raw_patches {
            reconstructor.add_raw_patch(&type_pattern, offset, &value);
        }

        if let Some(ts) = self.timestamp_ms {
            reconstructor.set_timestamp(ts);
        }

        reconstructor
    }
}

impl Default for ReconstructorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utilities::historical_bytecode;

    #[test]
    fn test_reconstruction_config_default() {
        let config = ReconstructionConfig::default();
        assert_eq!(config.failure_strategy, FailureStrategy::WarnAndSkip);
        assert!(config.auto_register_well_known);
        assert!(config.add_default_rules);
    }

    #[test]
    fn test_reconstructor_new() {
        let reconstructor = HistoricalStateReconstructor::new();
        assert!(reconstructor.detected_versions().is_empty());
    }

    #[test]
    fn test_reconstructor_set_timestamp() {
        let mut reconstructor = HistoricalStateReconstructor::new();
        reconstructor.set_timestamp(1700000000000);
        assert_eq!(reconstructor.timestamp_ms, Some(1700000000000));
    }

    #[test]
    fn test_reconstructor_register_version() {
        let mut reconstructor = HistoricalStateReconstructor::new();
        reconstructor.register_version("0x1eabed72", 3);

        // Should be stored in detected versions
        let versions = reconstructor.detected_versions();
        let normalized = historical_bytecode::normalize_id("0x1eabed72");
        assert_eq!(versions.get(&normalized), Some(&3));
    }

    #[test]
    fn test_builder_basic() {
        let reconstructor = ReconstructorBuilder::new()
            .failure_strategy(FailureStrategy::Skip)
            .with_version("0xabc", 5)
            .with_timestamp(1700000000000)
            .build();

        assert_eq!(reconstructor.timestamp_ms, Some(1700000000000));
    }

    #[test]
    fn test_builder_no_defaults() {
        let reconstructor = ReconstructorBuilder::new()
            .no_auto_well_known()
            .no_default_rules()
            .build();

        // Should be configured but with no defaults
        assert!(reconstructor.detected_versions().is_empty());
    }

    #[test]
    fn test_patch_objects_empty() {
        let mut reconstructor = HistoricalStateReconstructor::new();
        let objects = HashMap::new();
        let types = HashMap::new();

        let result = reconstructor.patch_objects(&objects, &types);
        assert!(result.is_empty());
    }

    #[test]
    fn test_reconstruct_returns_stats() {
        let mut reconstructor = HistoricalStateReconstructor::new();
        let objects = HashMap::new();
        let types = HashMap::new();

        let result = reconstructor.reconstruct(&objects, &types);
        assert!(result.objects.is_empty());
        assert_eq!(result.stats.total_patched(), 0);
    }
}
