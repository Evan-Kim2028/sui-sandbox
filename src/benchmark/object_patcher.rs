//! Object BCS Patching for Historical Transaction Replay
//!
//! This module provides automatic patching of object BCS data to fix
//! application-level version and time mismatches when replaying historical
//! transactions with current bytecode.
//!
//! ## Problem
//!
//! When replaying historical transactions:
//! 1. We fetch current bytecode (packages) from mainnet
//! 2. We fetch historical object state from archives
//! 3. The bytecode may have a newer expected version than the historical object
//! 4. Time-based validations may fail if historical timestamps are used
//!
//! ## Solution
//!
//! Automatically detect and patch known protocol objects:
//! - Scallop: Version.value field
//! - Cetus: GlobalConfig.package_version, RewarderManager.last_updated_time
//! - Other protocols as needed

use std::collections::HashMap;

/// Object type patterns to detect for patching
#[derive(Debug, Clone)]
pub struct PatchRule {
    /// Type pattern to match (e.g., "::version::Version")
    pub type_pattern: String,
    /// Field offset in BCS (after UID which is 32 bytes)
    pub field_offset: usize,
    /// Field size in bytes
    pub field_size: usize,
    /// Patch type
    pub patch_type: PatchType,
}

#[derive(Debug, Clone)]
pub enum PatchType {
    /// Set version to a specific value
    SetVersion(u64),
    /// Set time to transaction timestamp
    SetToTxTimestamp,
    /// Set time to a large value (far future)
    SetToFarFuture,
    /// Set to zero
    SetToZero,
}

/// Object patcher that can fix version/time mismatches in BCS data
pub struct ObjectPatcher {
    /// Rules for patching different object types
    rules: Vec<PatchRule>,
    /// Transaction timestamp (ms) for time-based patches
    tx_timestamp_ms: Option<u64>,
    /// Statistics
    patches_applied: HashMap<String, usize>,
}

impl ObjectPatcher {
    /// Create a new patcher with default rules for known DeFi protocols
    pub fn new() -> Self {
        Self {
            rules: Self::default_rules(),
            tx_timestamp_ms: None,
            patches_applied: HashMap::new(),
        }
    }

    /// Create a patcher with transaction timestamp for time-based patches
    pub fn with_timestamp(tx_timestamp_ms: u64) -> Self {
        Self {
            rules: Self::default_rules(),
            tx_timestamp_ms: Some(tx_timestamp_ms),
            patches_applied: HashMap::new(),
        }
    }

    /// Default rules for known protocol objects
    fn default_rules() -> Vec<PatchRule> {
        vec![
            // Scallop Version object
            // struct Version { id: UID, value: u64 }
            // UID is 32 bytes, value is u64 at offset 32
            PatchRule {
                type_pattern: "::version::Version".to_string(),
                field_offset: 32,
                field_size: 8,
                patch_type: PatchType::SetVersion(u64::MAX), // Will be replaced dynamically
            },
            // Cetus GlobalConfig package_version
            // struct GlobalConfig { id: UID, protocol_fee_rate: u64, ..., package_version: u64, ... }
            // This is more complex - need to find the exact offset
            PatchRule {
                type_pattern: "::config::GlobalConfig".to_string(),
                field_offset: 0, // Will use smart detection
                field_size: 8,
                patch_type: PatchType::SetVersion(1), // Cetus expects exactly 1
            },
            // Cetus RewarderManager last_updated_time
            // Part of a larger structure - need smart patching
            PatchRule {
                type_pattern: "::rewarder::RewarderManager".to_string(),
                field_offset: 0, // Will use smart detection
                field_size: 8,
                patch_type: PatchType::SetToTxTimestamp,
            },
        ]
    }

    /// Add a custom patch rule
    pub fn add_rule(&mut self, rule: PatchRule) {
        self.rules.push(rule);
    }

    /// Set the transaction timestamp for time-based patches
    pub fn set_timestamp(&mut self, timestamp_ms: u64) {
        self.tx_timestamp_ms = Some(timestamp_ms);
    }

    /// Attempt to patch object BCS data based on its type
    ///
    /// Returns the patched bytes if a rule matches, otherwise returns original bytes
    pub fn patch_object(&mut self, type_str: &str, bcs_bytes: &[u8]) -> Vec<u8> {
        let mut result = bcs_bytes.to_vec();

        for rule in &self.rules {
            if type_str.contains(&rule.type_pattern) {
                if let Some(patched) = self.apply_rule(&rule.clone(), &result, type_str) {
                    *self
                        .patches_applied
                        .entry(rule.type_pattern.clone())
                        .or_insert(0) += 1;
                    result = patched;
                }
            }
        }

        result
    }

    /// Apply a specific patch rule to BCS bytes
    fn apply_rule(&self, rule: &PatchRule, bcs_bytes: &[u8], type_str: &str) -> Option<Vec<u8>> {
        let result = bcs_bytes.to_vec();

        // For Scallop Version - simple fixed offset
        if type_str.contains("::version::Version") && !type_str.contains("VersionCap") {
            return self.patch_scallop_version(&result);
        }

        // For Cetus GlobalConfig - find package_version field
        if type_str.contains("::config::GlobalConfig") {
            return self.patch_cetus_global_config(&result);
        }

        // For time-based fields, use smart detection
        if matches!(
            rule.patch_type,
            PatchType::SetToTxTimestamp | PatchType::SetToFarFuture
        ) {
            if let Some(ts) = self.tx_timestamp_ms {
                // Look for timestamp-like u64 values and patch them
                return self.patch_timestamp_fields(&result, ts);
            }
        }

        None
    }

    /// Patch Scallop Version object
    /// struct Version { id: UID (32 bytes), value: u64 }
    fn patch_scallop_version(&self, bcs_bytes: &[u8]) -> Option<Vec<u8>> {
        if bcs_bytes.len() < 40 {
            return None;
        }

        let mut result = bcs_bytes.to_vec();

        // The value field is at offset 32 (after UID)
        // Set it to a high value that will pass any version check
        // Most protocols use incrementing versions, so MAX-1 should work
        let new_version: u64 = u64::MAX - 1;
        let version_bytes = new_version.to_le_bytes();
        result[32..40].copy_from_slice(&version_bytes);

        Some(result)
    }

    /// Patch Cetus GlobalConfig package_version
    /// The package_version is expected to be 1
    fn patch_cetus_global_config(&self, bcs_bytes: &[u8]) -> Option<Vec<u8>> {
        // GlobalConfig structure (approximate):
        // - id: UID (32 bytes)
        // - protocol_fee_rate: u64 (8 bytes)
        // - unstaked_liquidity_fee_rate: u64 (8 bytes)
        // - fee_tiers: VecMap (variable)
        // - acl: ACL (variable)
        // - package_version: u64 (8 bytes)
        // - alive_gauges: VecSet (variable)

        // Since the structure has variable-length fields, we need to search for
        // the package_version field. It's typically a small number (1, 2, etc.)
        // We'll look for bytes that look like a version number followed by
        // data that looks like a VecSet

        if bcs_bytes.len() < 50 {
            return None;
        }

        let mut result = bcs_bytes.to_vec();

        // Strategy: Find u64 values that are small (1-10) and likely to be versions
        // The package_version is expected to be 1, so we look for small values
        // and patch them

        // For now, use a heuristic: search for the pattern where package_version
        // would be (after the ACL structure)

        // Simpler approach: just scan for any u64 that's a reasonable version number
        // and patch it to 1
        for i in (40..bcs_bytes.len().saturating_sub(8)).step_by(8) {
            let val = u64::from_le_bytes(result[i..i + 8].try_into().ok()?);
            // If this looks like a version number (small positive integer)
            if val > 0 && val < 100 {
                // Check if next bytes look like a VecSet (starts with length)
                if i + 8 < bcs_bytes.len() {
                    let next_val =
                        u64::from_le_bytes(result[i + 8..i + 16].try_into().unwrap_or([0; 8]));
                    // If next value is small (VecSet length), this might be package_version
                    if next_val < 1000 {
                        // Patch to 1
                        result[i..i + 8].copy_from_slice(&1u64.to_le_bytes());
                        return Some(result);
                    }
                }
            }
        }

        None
    }

    /// Patch timestamp fields to match transaction time
    fn patch_timestamp_fields(&self, bcs_bytes: &[u8], tx_timestamp_ms: u64) -> Option<Vec<u8>> {
        let mut result = bcs_bytes.to_vec();
        let mut patched = false;

        // Look for u64 values that look like timestamps (in milliseconds)
        // Timestamps are typically in the range of 1600000000000 to 1800000000000 (2020-2027)
        let min_ts = 1600000000000u64;
        let max_ts = 1900000000000u64;

        for i in (0..bcs_bytes.len().saturating_sub(8)).step_by(8) {
            if let Ok(bytes) = result[i..i + 8].try_into() {
                let val = u64::from_le_bytes(bytes);
                if val >= min_ts && val <= max_ts {
                    // This looks like a timestamp - patch it to be <= tx_timestamp
                    if val > tx_timestamp_ms {
                        result[i..i + 8].copy_from_slice(&tx_timestamp_ms.to_le_bytes());
                        patched = true;
                    }
                }
            }
        }

        if patched {
            Some(result)
        } else {
            None
        }
    }

    /// Get statistics about patches applied
    pub fn stats(&self) -> &HashMap<String, usize> {
        &self.patches_applied
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.patches_applied.clear();
    }
}

impl Default for ObjectPatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function to patch a single object
pub fn patch_object_bcs(type_str: &str, bcs_bytes: &[u8], tx_timestamp_ms: Option<u64>) -> Vec<u8> {
    let mut patcher = if let Some(ts) = tx_timestamp_ms {
        ObjectPatcher::with_timestamp(ts)
    } else {
        ObjectPatcher::new()
    };
    patcher.patch_object(type_str, bcs_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scallop_version_patch() {
        let mut patcher = ObjectPatcher::new();

        // Create a mock Version object: 32 bytes UID + 8 bytes version
        let mut bcs = vec![0u8; 40];
        // Set version to 5
        bcs[32..40].copy_from_slice(&5u64.to_le_bytes());

        let patched = patcher.patch_object("0xabc::version::Version", &bcs);

        // Check version was patched to MAX-1
        let new_version = u64::from_le_bytes(patched[32..40].try_into().unwrap());
        assert_eq!(new_version, u64::MAX - 1);
    }

    #[test]
    fn test_timestamp_patch() {
        let tx_time = 1700000000000u64; // Some past time
        let mut patcher = ObjectPatcher::with_timestamp(tx_time);

        // Create mock data with a future timestamp
        let mut bcs = vec![0u8; 48];
        let future_time = 1800000000000u64;
        bcs[32..40].copy_from_slice(&future_time.to_le_bytes());

        let patched = patcher.patch_object("::rewarder::RewarderManager", &bcs);

        // Check timestamp was patched
        let new_time = u64::from_le_bytes(patched[32..40].try_into().unwrap());
        assert!(new_time <= tx_time);
    }
}
