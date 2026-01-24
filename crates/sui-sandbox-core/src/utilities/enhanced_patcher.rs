//! Enhanced Object Patcher
//!
//! This module provides a unified patching system with layered fallback strategies:
//!
//! 1. **Manual overrides**: User-specified patches for specific objects
//! 2. **Struct-based patching**: Using introspected field layouts
//! 3. **Raw byte patches**: Direct byte manipulation as fallback
//! 4. **Configurable failure handling**: Skip, warn, or error
//!
//! ## Usage
//!
//! ```ignore
//! use sui_sandbox_core::utilities::EnhancedObjectPatcher;
//!
//! let mut patcher = EnhancedObjectPatcher::new();
//!
//! // Register manual override for problematic objects
//! patcher.register_override(ManualPatchOverride {
//!     object_id: "0xdaa46292...".to_string(),
//!     patches: vec![(32, 3u64.to_le_bytes().to_vec())],
//! });
//!
//! // Patch an object
//! let patched = patcher.patch_object(&object_id, &type_str, &bcs_bytes);
//! ```

use std::collections::HashMap;

use super::generic_patcher::GenericObjectPatcher;
use super::version_field_detector::{
    calculate_offset, find_well_known_config, VersionFieldDetector,
};

/// How to handle patching failures.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FailureStrategy {
    /// Skip patching silently
    Skip,
    /// Warn and skip
    #[default]
    WarnAndSkip,
    /// Return an error
    Error,
}

/// A manual patch override for a specific object.
#[derive(Debug, Clone)]
pub struct ManualPatchOverride {
    /// Object ID to match (normalized)
    pub object_id: String,
    /// Type pattern to match (optional, substring match)
    pub type_pattern: Option<String>,
    /// Raw byte patches: (offset, bytes)
    pub patches: Vec<(usize, Vec<u8>)>,
}

/// Statistics about patching operations.
#[derive(Debug, Clone, Default)]
pub struct PatchingStats {
    /// Objects patched via struct introspection
    pub struct_patched: usize,
    /// Objects patched via raw byte patches
    pub raw_patched: usize,
    /// Objects patched via manual overrides
    pub override_patched: usize,
    /// Objects where patching was skipped (failure or no match)
    pub skipped: usize,
    /// Objects where patching failed (error mode)
    pub failed: usize,
    /// Breakdown by field name
    pub by_field: HashMap<String, usize>,
}

impl PatchingStats {
    /// Total number of objects patched (any method).
    pub fn total_patched(&self) -> usize {
        self.struct_patched + self.raw_patched + self.override_patched
    }

    /// Record a struct-based patch.
    pub fn record_struct_patch(&mut self, field_name: &str) {
        self.struct_patched += 1;
        *self.by_field.entry(field_name.to_string()).or_insert(0) += 1;
    }

    /// Record a raw byte patch.
    pub fn record_raw_patch(&mut self) {
        self.raw_patched += 1;
    }

    /// Record a manual override patch.
    pub fn record_override_patch(&mut self) {
        self.override_patched += 1;
    }

    /// Record a skip.
    pub fn record_skip(&mut self) {
        self.skipped += 1;
    }

    /// Record a failure.
    pub fn record_failure(&mut self) {
        self.failed += 1;
    }
}

/// Enhanced object patcher with layered fallback strategies.
pub struct EnhancedObjectPatcher {
    /// Underlying generic patcher for struct-based patching
    generic_patcher: GenericObjectPatcher,
    /// Version field detector (reserved for future use)
    #[allow(dead_code)]
    version_detector: VersionFieldDetector,
    /// Manual overrides by object ID
    overrides: HashMap<String, ManualPatchOverride>,
    /// Raw byte patches by type pattern
    raw_patches: HashMap<String, Vec<(usize, Vec<u8>)>>,
    /// Failure handling strategy
    failure_strategy: FailureStrategy,
    /// Patching statistics
    stats: PatchingStats,
    /// Detected version from bytecode (package_addr -> version)
    detected_versions: HashMap<String, u64>,
}

impl EnhancedObjectPatcher {
    /// Create a new enhanced patcher.
    pub fn new() -> Self {
        Self {
            generic_patcher: GenericObjectPatcher::new(),
            version_detector: VersionFieldDetector::new(),
            overrides: HashMap::new(),
            raw_patches: HashMap::new(),
            failure_strategy: FailureStrategy::default(),
            stats: PatchingStats::default(),
            detected_versions: HashMap::new(),
        }
    }

    /// Set the failure handling strategy.
    pub fn with_failure_strategy(mut self, strategy: FailureStrategy) -> Self {
        self.failure_strategy = strategy;
        self
    }

    /// Get mutable access to the underlying generic patcher.
    pub fn generic_patcher_mut(&mut self) -> &mut GenericObjectPatcher {
        &mut self.generic_patcher
    }

    /// Get the underlying generic patcher.
    pub fn generic_patcher(&self) -> &GenericObjectPatcher {
        &self.generic_patcher
    }

    /// Add modules for struct layout extraction.
    pub fn add_modules<'a>(
        &mut self,
        modules: impl Iterator<Item = &'a move_binary_format::file_format::CompiledModule>,
    ) {
        self.generic_patcher.add_modules(modules);
    }

    /// Set the transaction timestamp for time-based patches.
    pub fn set_timestamp(&mut self, timestamp_ms: u64) {
        self.generic_patcher.set_timestamp(timestamp_ms);
    }

    /// Add default patching rules.
    pub fn add_default_rules(&mut self) {
        self.generic_patcher.add_default_rules();
    }

    /// Register a version for a package address.
    pub fn register_version(&mut self, package_addr: &str, version: u64) {
        let normalized = super::historical_bytecode::normalize_id(package_addr);
        self.detected_versions.insert(normalized.clone(), version);
        self.generic_patcher.register_version(package_addr, version);
    }

    /// Register detected versions from bytecode scanning.
    pub fn register_detected_versions(&mut self, versions: &HashMap<String, u64>) {
        for (addr, version) in versions {
            self.register_version(addr, *version);
        }
    }

    /// Register a manual patch override for a specific object.
    pub fn register_override(&mut self, override_spec: ManualPatchOverride) {
        let normalized = super::historical_bytecode::normalize_id(&override_spec.object_id);
        self.overrides.insert(normalized, override_spec);
    }

    /// Register a raw byte patch for a type pattern.
    ///
    /// This is used as a fallback when struct decoding fails.
    pub fn add_raw_patch(&mut self, type_pattern: &str, offset: usize, value: &[u8]) {
        self.raw_patches
            .entry(type_pattern.to_string())
            .or_default()
            .push((offset, value.to_vec()));

        // Also register with generic patcher for compatibility
        self.generic_patcher
            .add_raw_patch(type_pattern, offset, value);
    }

    /// Convenience method to add a raw u64 patch.
    pub fn add_raw_u64_patch(&mut self, type_pattern: &str, offset: usize, value: u64) {
        self.add_raw_patch(type_pattern, offset, &value.to_le_bytes());
    }

    /// Auto-register raw patches for well-known version field offsets.
    ///
    /// NOTE: This is a no-op because well-known offsets now use FieldPosition::FromEnd
    /// which requires knowing the BCS length at patch time. Use manual overrides or
    /// rely on the layered patching in `patch_object` which handles FieldPosition correctly.
    pub fn auto_register_well_known_patches(&mut self) {
        // Well-known offsets now use FieldPosition which requires BCS length.
        // The actual patching happens in patch_object's Layer 4.
    }

    /// Patch an object's BCS data using layered strategy.
    ///
    /// 1. Check manual overrides
    /// 2. Try well-known protocol configurations (highest priority for known protocols)
    /// 3. Try struct-based patching
    /// 4. Fall back to raw patches
    /// 5. Handle failure per strategy
    pub fn patch_object(&mut self, object_id: &str, type_str: &str, bcs_bytes: &[u8]) -> Vec<u8> {
        let normalized_id = super::historical_bytecode::normalize_id(object_id);

        // Layer 1: Check manual overrides
        if let Some(override_spec) = self.overrides.get(&normalized_id).cloned() {
            // Check type pattern if specified
            let type_matches = override_spec
                .type_pattern
                .as_ref()
                .map(|p| type_str.contains(p))
                .unwrap_or(true);

            if type_matches && !override_spec.patches.is_empty() {
                let result = self.apply_raw_patches(bcs_bytes, &override_spec.patches);
                self.stats.record_override_patch();
                return result;
            }
        }

        // Layer 2: Try well-known protocol configurations FIRST
        // This handles protocols like Cetus GlobalConfig where:
        // - The version field is at a known position
        // - There's a specific version required (hardcoded in bytecode)
        //
        // IMPORTANT: This runs BEFORE struct-based patching because well-known configs
        // have specific hardcoded values that MUST be used. Struct-based patching might
        // patch to detected versions which could be wrong.
        if let Some((position, size, default_version)) = find_well_known_config(type_str) {
            if size == 8 {
                // Calculate actual offset from position and BCS length
                if let Some(offset) = calculate_offset(position, bcs_bytes.len()) {
                    // Use the well-known default version (required for equality checks)
                    let patches = vec![(offset, default_version.to_le_bytes().to_vec())];
                    let result = self.apply_raw_patches(bcs_bytes, &patches);
                    if result != bcs_bytes {
                        self.stats.record_raw_patch();
                        return result;
                    }
                }
            }
        }

        // Layer 3: Try struct-based patching via generic patcher
        let patched = self.generic_patcher.patch_object(type_str, bcs_bytes);

        // Check if any patching was done (bytes changed)
        if patched != bcs_bytes {
            // Record stats from generic patcher
            for (field, count) in self.generic_patcher.stats() {
                for _ in 0..*count {
                    self.stats.record_struct_patch(field);
                }
            }
            self.generic_patcher.reset_stats();
            return patched;
        }

        // Layer 4: Try raw patches by type pattern
        if let Some(patches) = self.find_raw_patches_for_type(type_str) {
            if !patches.is_empty() {
                let result = self.apply_raw_patches(bcs_bytes, &patches);
                if result != bcs_bytes {
                    self.stats.record_raw_patch();
                    return result;
                }
            }
        }

        // No patching applied
        match self.failure_strategy {
            FailureStrategy::Skip => {
                self.stats.record_skip();
            }
            FailureStrategy::WarnAndSkip => {
                // Only warn if this type looks like it might need patching
                if self.type_might_need_patching(type_str) {
                    eprintln!(
                        "[EnhancedPatcher] No patches applied to {} ({}...)",
                        &type_str[..type_str.len().min(50)],
                        &normalized_id[..normalized_id.len().min(20)]
                    );
                }
                self.stats.record_skip();
            }
            FailureStrategy::Error => {
                self.stats.record_failure();
            }
        }

        bcs_bytes.to_vec()
    }

    /// Find raw patches matching a type string.
    fn find_raw_patches_for_type(&self, type_str: &str) -> Option<Vec<(usize, Vec<u8>)>> {
        let mut all_patches = Vec::new();

        for (pattern, patches) in &self.raw_patches {
            if type_str.contains(pattern) {
                all_patches.extend(patches.clone());
            }
        }

        if all_patches.is_empty() {
            None
        } else {
            Some(all_patches)
        }
    }

    /// Apply raw byte patches to BCS data.
    fn apply_raw_patches(&self, bcs_bytes: &[u8], patches: &[(usize, Vec<u8>)]) -> Vec<u8> {
        let mut result = bcs_bytes.to_vec();

        for (offset, value) in patches {
            if *offset + value.len() <= result.len() {
                result[*offset..*offset + value.len()].copy_from_slice(value);
            }
        }

        result
    }

    /// Check if a type might need version patching.
    fn type_might_need_patching(&self, type_str: &str) -> bool {
        // Check for common version-locked type patterns
        type_str.contains("Config")
            || type_str.contains("Version")
            || type_str.contains("Global")
            || type_str.contains("Registry")
    }

    /// Find the version to use for a type based on its package.
    /// Reserved for future use when auto-detecting versions from bytecode.
    #[allow(dead_code)]
    fn find_version_for_type(&self, type_str: &str) -> Option<u64> {
        // Extract package address from type (first part before ::)
        let pkg_part = type_str.split("::").next()?;
        let normalized = super::historical_bytecode::normalize_id(pkg_part);

        // Look up in detected versions
        self.detected_versions.get(&normalized).copied()
    }

    /// Get patching statistics.
    pub fn stats(&self) -> &PatchingStats {
        &self.stats
    }

    /// Reset statistics.
    pub fn reset_stats(&mut self) {
        self.stats = PatchingStats::default();
        self.generic_patcher.reset_stats();
    }

    /// Get detected versions.
    pub fn detected_versions(&self) -> &HashMap<String, u64> {
        &self.detected_versions
    }
}

impl Default for EnhancedObjectPatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failure_strategy_default() {
        assert_eq!(FailureStrategy::default(), FailureStrategy::WarnAndSkip);
    }

    #[test]
    fn test_patching_stats() {
        let mut stats = PatchingStats::default();

        stats.record_struct_patch("package_version");
        stats.record_struct_patch("package_version");
        stats.record_raw_patch();
        stats.record_override_patch();
        stats.record_skip();

        assert_eq!(stats.struct_patched, 2);
        assert_eq!(stats.raw_patched, 1);
        assert_eq!(stats.override_patched, 1);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.total_patched(), 4);
        assert_eq!(stats.by_field.get("package_version"), Some(&2));
    }

    #[test]
    fn test_enhanced_patcher_new() {
        let patcher = EnhancedObjectPatcher::new();
        assert_eq!(patcher.stats().total_patched(), 0);
    }

    #[test]
    fn test_register_override() {
        let mut patcher = EnhancedObjectPatcher::new();

        patcher.register_override(ManualPatchOverride {
            object_id: "0xabc".to_string(),
            type_pattern: None,
            patches: vec![(32, vec![1, 2, 3, 4, 5, 6, 7, 8])],
        });

        // Should be registered (normalized)
        let normalized =
            "0x0000000000000000000000000000000000000000000000000000000000000abc".to_string();
        assert!(patcher.overrides.contains_key(&normalized));
    }

    #[test]
    fn test_add_raw_patch() {
        let mut patcher = EnhancedObjectPatcher::new();

        patcher.add_raw_u64_patch("::config::GlobalConfig", 32, 5);

        assert!(patcher.raw_patches.contains_key("::config::GlobalConfig"));
    }

    #[test]
    fn test_apply_raw_patches() {
        let patcher = EnhancedObjectPatcher::new();

        let bcs = vec![0u8; 48];
        let patches = vec![(32, 42u64.to_le_bytes().to_vec())];

        let result = patcher.apply_raw_patches(&bcs, &patches);

        // Check that bytes 32-39 are now 42 (little-endian)
        let value = u64::from_le_bytes(result[32..40].try_into().unwrap());
        assert_eq!(value, 42);
    }

    #[test]
    fn test_type_might_need_patching() {
        let patcher = EnhancedObjectPatcher::new();

        assert!(patcher.type_might_need_patching("0x1::config::GlobalConfig"));
        assert!(patcher.type_might_need_patching("0x1::version::Version"));
        assert!(!patcher.type_might_need_patching("0x1::coin::Coin"));
    }

    #[test]
    fn test_find_version_for_type() {
        let mut patcher = EnhancedObjectPatcher::new();
        patcher.register_version("0x1eabed72", 3);

        let version = patcher.find_version_for_type("0x1eabed72::config::GlobalConfig");
        assert_eq!(version, Some(3));

        let version = patcher.find_version_for_type("0xunknown::config::GlobalConfig");
        assert_eq!(version, None);
    }
}
