//! Transaction types for Sui sandbox.
//!
//! This module contains the core transaction types used throughout the sui-sandbox
//! workspace for transaction replay and caching.

use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::encoding::{base64_encode, try_base64_decode};

// Note: ObjectID is now canonically defined in fetched.rs and re-exported from lib.rs.

/// Transaction digest (32 bytes, base58 encoded in JSON)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransactionDigest(pub String);

impl TransactionDigest {
    pub fn new(digest: impl Into<String>) -> Self {
        Self(digest.into())
    }
}

/// Represents a fetched transaction from the Sui network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedTransaction {
    /// Transaction digest
    pub digest: TransactionDigest,

    /// Sender address
    pub sender: AccountAddress,

    /// Gas budget
    pub gas_budget: u64,

    /// Gas price
    pub gas_price: u64,

    /// The PTB commands in this transaction
    pub commands: Vec<PtbCommand>,

    /// Input objects (owned, shared, immutable)
    pub inputs: Vec<TransactionInput>,

    /// Transaction effects (for comparison)
    pub effects: Option<TransactionEffectsSummary>,

    /// Timestamp (milliseconds since epoch)
    pub timestamp_ms: Option<u64>,

    /// Checkpoint that included this transaction
    pub checkpoint: Option<u64>,
}

/// A command in a Programmable Transaction Block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PtbCommand {
    /// Move function call
    MoveCall {
        package: String,
        module: String,
        function: String,
        type_arguments: Vec<String>,
        arguments: Vec<PtbArgument>,
    },

    /// Split coins
    SplitCoins {
        coin: PtbArgument,
        amounts: Vec<PtbArgument>,
    },

    /// Merge coins
    MergeCoins {
        destination: PtbArgument,
        sources: Vec<PtbArgument>,
    },

    /// Transfer objects
    TransferObjects {
        objects: Vec<PtbArgument>,
        address: PtbArgument,
    },

    /// Make move vector
    MakeMoveVec {
        type_arg: Option<String>,
        elements: Vec<PtbArgument>,
    },

    /// Publish new package
    Publish {
        modules: Vec<String>, // base64 encoded
        dependencies: Vec<String>,
    },

    /// Upgrade package
    Upgrade {
        modules: Vec<String>,
        package: String,
        ticket: PtbArgument,
    },
}

/// Argument reference in a PTB command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PtbArgument {
    /// Reference to a transaction input
    Input { index: u16 },

    /// Reference to a previous command result
    Result { index: u16 },

    /// Reference to a nested result (for multi-return functions)
    NestedResult { index: u16, result_index: u16 },

    /// Gas coin (special input)
    GasCoin,
}

/// Transaction input object or pure value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TransactionInput {
    /// Pure BCS-encoded value
    Pure {
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },

    /// Object reference (owned)
    Object {
        object_id: String,
        version: u64,
        digest: String,
    },

    /// Shared object reference
    SharedObject {
        object_id: String,
        initial_shared_version: u64,
        mutable: bool,
    },

    /// Immutable object (e.g., package, Clock)
    ImmutableObject {
        object_id: String,
        version: u64,
        digest: String,
    },

    /// Receiving object
    Receiving {
        object_id: String,
        version: u64,
        digest: String,
    },
}

/// Summary of transaction effects for comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionEffectsSummary {
    /// Transaction status
    pub status: TransactionStatus,

    /// Created object IDs
    pub created: Vec<String>,

    /// Mutated object IDs
    pub mutated: Vec<String>,

    /// Deleted object IDs
    pub deleted: Vec<String>,

    /// Wrapped object IDs
    pub wrapped: Vec<String>,

    /// Unwrapped object IDs
    pub unwrapped: Vec<String>,

    /// Gas used
    pub gas_used: GasSummary,

    /// Events emitted
    pub events_count: usize,

    /// Shared object versions at transaction time (object_id -> version)
    /// This is extracted from effects.sharedObjects for historical replay.
    #[serde(default)]
    pub shared_object_versions: HashMap<String, u64>,
}

/// Transaction execution status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransactionStatus {
    Success,
    Failure { error: String },
}

/// Gas usage summary.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GasSummary {
    pub computation_cost: u64,
    pub storage_cost: u64,
    pub storage_rebate: u64,
    pub non_refundable_storage_fee: u64,
}

/// Result of replaying a transaction locally.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayResult {
    /// Original transaction digest
    pub digest: TransactionDigest,

    /// Whether local execution succeeded
    pub local_success: bool,

    /// Local execution error (if any)
    pub local_error: Option<String>,

    /// Comparison with on-chain effects
    pub comparison: Option<EffectsComparison>,

    /// Commands that were executed
    pub commands_executed: usize,

    /// Commands that failed
    pub commands_failed: usize,

    // =========================================================================
    // Version Tracking Results (populated when version tracking is enabled)
    // =========================================================================
    /// Number of objects with version tracking info
    #[serde(default)]
    pub objects_tracked: usize,

    /// Lamport timestamp used for this execution
    #[serde(default)]
    pub lamport_timestamp: Option<u64>,

    /// Summary of version changes by type
    #[serde(default)]
    pub version_summary: Option<VersionSummary>,

    // =========================================================================
    // Gas Tracking Results (populated when accurate_gas is enabled)
    // =========================================================================
    /// Computation gas used (from PTB execution, in gas units)
    #[serde(default)]
    pub gas_used: u64,
}

/// Summary of version changes in a transaction.
#[derive(Debug, Clone, Serialize, Default)]
pub struct VersionSummary {
    /// Number of created objects
    pub created: usize,
    /// Number of mutated objects
    pub mutated: usize,
    /// Number of deleted objects
    pub deleted: usize,
    /// Number of wrapped objects
    pub wrapped: usize,
}

/// Comparison between local and on-chain effects.
#[derive(Debug, Clone, Serialize)]
pub struct EffectsComparison {
    /// Status match (both success or both failure)
    pub status_match: bool,

    /// Created objects count match
    pub created_count_match: bool,

    /// Mutated objects count match
    pub mutated_count_match: bool,

    /// Deleted objects count match
    pub deleted_count_match: bool,

    /// Overall match score (0.0 - 1.0)
    pub match_score: f64,

    /// Notes about differences
    pub notes: Vec<String>,

    // =========================================================================
    // Object-Level ID Comparison (optional)
    // =========================================================================
    /// Whether created object IDs matched exactly
    #[serde(default)]
    pub created_ids_match: bool,
    /// Whether mutated object IDs matched (allows extra gas objects)
    #[serde(default)]
    pub mutated_ids_match: bool,
    /// Whether deleted object IDs matched exactly
    #[serde(default)]
    pub deleted_ids_match: bool,

    /// Created IDs missing from local execution
    #[serde(default)]
    pub created_ids_missing: Vec<String>,
    /// Created IDs extra in local execution
    #[serde(default)]
    pub created_ids_extra: Vec<String>,
    /// Mutated IDs missing from local execution
    #[serde(default)]
    pub mutated_ids_missing: Vec<String>,
    /// Mutated IDs extra in local execution
    #[serde(default)]
    pub mutated_ids_extra: Vec<String>,
    /// Deleted IDs missing from local execution
    #[serde(default)]
    pub deleted_ids_missing: Vec<String>,
    /// Deleted IDs extra in local execution
    #[serde(default)]
    pub deleted_ids_extra: Vec<String>,

    // =========================================================================
    // Version Tracking Comparison (populated when version info is provided)
    // =========================================================================
    /// Whether version tracking comparison was performed
    #[serde(default)]
    pub version_tracking_enabled: bool,

    /// Number of objects where input versions matched expected
    #[serde(default)]
    pub input_versions_matched: usize,

    /// Number of objects where input versions were checked
    #[serde(default)]
    pub input_versions_total: usize,

    /// Number of objects where version increments were valid (output = input + 1)
    #[serde(default)]
    pub version_increments_valid: usize,

    /// Number of version increments checked
    #[serde(default)]
    pub version_increments_total: usize,

    /// Specific version mismatches for debugging
    #[serde(default)]
    pub version_mismatches: Vec<VersionMismatch>,
}

/// Details about a version mismatch between local and on-chain.
#[derive(Debug, Clone, Serialize)]
pub struct VersionMismatch {
    /// Object ID (hex string)
    pub object_id: String,
    /// Type of mismatch
    pub mismatch_type: VersionMismatchType,
    /// Expected version (from on-chain or input)
    pub expected: Option<u64>,
    /// Actual version (from local execution)
    pub actual: Option<u64>,
}

/// Type of version mismatch.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum VersionMismatchType {
    /// Input version doesn't match expected
    InputVersion,
    /// Output version doesn't match expected (input + 1)
    OutputVersion,
    /// Object was created but expected version isn't 1
    CreatedVersion,
}

impl EffectsComparison {
    /// Create a comparison between local and on-chain effects.
    ///
    /// Note: On-chain effects always include gas object mutations (from gas consumption).
    /// Our local execution doesn't track gas, so we adjust for this when comparing
    /// mutated object counts.
    pub fn compare(
        on_chain: &TransactionEffectsSummary,
        local_success: bool,
        local_created: usize,
        local_mutated: usize,
        local_deleted: usize,
    ) -> Self {
        let mut notes = Vec::new();
        let mut match_points = 0.0;
        let total_points = 4.0;

        // Compare status
        let status_match = matches!(
            (&on_chain.status, local_success),
            (TransactionStatus::Success, true) | (TransactionStatus::Failure { .. }, false)
        );
        if status_match {
            match_points += 1.0;
        } else {
            notes.push(format!(
                "Status mismatch: on-chain={:?}, local={}",
                on_chain.status,
                if local_success { "success" } else { "failure" }
            ));
        }

        // Compare created count
        let created_count_match = on_chain.created.len() == local_created;
        if created_count_match {
            match_points += 1.0;
        } else {
            notes.push(format!(
                "Created count mismatch: on-chain={}, local={}",
                on_chain.created.len(),
                local_created
            ));
        }

        // Compare mutated count
        // On-chain always includes gas object mutation (1 or more objects for gas).
        // Our local execution doesn't track gas, so we allow for this difference.
        // Consider it a match if:
        // - exact match, OR
        // - on-chain has 1 more (gas object), OR
        // - on-chain has 2 more (gas object + gas payment split scenarios)
        let on_chain_mutated = on_chain.mutated.len();
        let mutated_diff = on_chain_mutated as isize - local_mutated as isize;
        let mutated_count_match = mutated_diff == 0 || mutated_diff == 1 || mutated_diff == 2;
        if mutated_count_match {
            match_points += 1.0;
        } else {
            notes.push(format!(
                "Mutated count mismatch: on-chain={}, local={} (diff={})",
                on_chain_mutated, local_mutated, mutated_diff
            ));
        }

        // Compare deleted count
        let deleted_count_match = on_chain.deleted.len() == local_deleted;
        if deleted_count_match {
            match_points += 1.0;
        } else {
            notes.push(format!(
                "Deleted count mismatch: on-chain={}, local={}",
                on_chain.deleted.len(),
                local_deleted
            ));
        }

        let match_score = match_points / total_points;

        Self {
            status_match,
            created_count_match,
            mutated_count_match,
            deleted_count_match,
            match_score,
            notes,
            created_ids_match: true,
            mutated_ids_match: true,
            deleted_ids_match: true,
            created_ids_missing: Vec::new(),
            created_ids_extra: Vec::new(),
            mutated_ids_missing: Vec::new(),
            mutated_ids_extra: Vec::new(),
            deleted_ids_missing: Vec::new(),
            deleted_ids_extra: Vec::new(),
            // Version tracking fields not populated in basic comparison
            version_tracking_enabled: false,
            input_versions_matched: 0,
            input_versions_total: 0,
            version_increments_valid: 0,
            version_increments_total: 0,
            version_mismatches: Vec::new(),
        }
    }

    /// Apply object-level ID comparison between on-chain and local effects.
    ///
    /// This supplements count-based comparison with ID-level checks.
    /// For mutated objects, extra on-chain IDs are tolerated (gas object mutations).
    pub fn apply_object_id_comparison(
        &mut self,
        on_chain: &TransactionEffectsSummary,
        local: &TransactionEffectsSummary,
    ) {
        use std::collections::HashSet;

        fn normalize_ids(ids: &[String]) -> Vec<String> {
            ids.iter()
                .map(|id| {
                    AccountAddress::from_hex_literal(id)
                        .map(|addr| addr.to_hex_literal())
                        .unwrap_or_else(|_| id.clone())
                })
                .collect()
        }

        let on_chain_created_ids = normalize_ids(&on_chain.created);
        let local_created_ids = normalize_ids(&local.created);
        let on_chain_mutated_ids = normalize_ids(&on_chain.mutated);
        let local_mutated_ids = normalize_ids(&local.mutated);
        let on_chain_deleted_ids = normalize_ids(&on_chain.deleted);
        let local_deleted_ids = normalize_ids(&local.deleted);

        // Created
        let on_chain_created: HashSet<_> = on_chain_created_ids.into_iter().collect();
        let local_created: HashSet<_> = local_created_ids.into_iter().collect();
        let created_missing: Vec<String> = on_chain_created
            .difference(&local_created)
            .cloned()
            .collect();
        let created_extra: Vec<String> = local_created
            .difference(&on_chain_created)
            .cloned()
            .collect();
        self.created_ids_match = created_missing.is_empty() && created_extra.is_empty();
        self.created_ids_missing = created_missing;
        self.created_ids_extra = created_extra;
        if !self.created_ids_match {
            self.notes.push(format!(
                "Created ID mismatch: missing={}, extra={}",
                self.created_ids_missing.len(),
                self.created_ids_extra.len()
            ));
        }

        // Mutated (allow extra on-chain mutations for gas)
        let on_chain_mutated: HashSet<_> = on_chain_mutated_ids.into_iter().collect();
        let local_mutated: HashSet<_> = local_mutated_ids.into_iter().collect();
        let mutated_missing: Vec<String> = local_mutated
            .difference(&on_chain_mutated)
            .cloned()
            .collect();
        let mutated_extra: Vec<String> = on_chain_mutated
            .difference(&local_mutated)
            .cloned()
            .collect();
        self.mutated_ids_match = mutated_missing.is_empty() && mutated_extra.len() <= 2;
        self.mutated_ids_missing = mutated_missing;
        self.mutated_ids_extra = mutated_extra;
        if !self.mutated_ids_match {
            self.notes.push(format!(
                "Mutated ID mismatch: missing={}, extra={}",
                self.mutated_ids_missing.len(),
                self.mutated_ids_extra.len()
            ));
        }

        // Deleted
        let on_chain_deleted: HashSet<_> = on_chain_deleted_ids.into_iter().collect();
        let local_deleted: HashSet<_> = local_deleted_ids.into_iter().collect();
        let deleted_missing: Vec<String> = on_chain_deleted
            .difference(&local_deleted)
            .cloned()
            .collect();
        let deleted_extra: Vec<String> = local_deleted
            .difference(&on_chain_deleted)
            .cloned()
            .collect();
        self.deleted_ids_match = deleted_missing.is_empty() && deleted_extra.is_empty();
        self.deleted_ids_missing = deleted_missing;
        self.deleted_ids_extra = deleted_extra;
        if !self.deleted_ids_match {
            self.notes.push(format!(
                "Deleted ID mismatch: missing={}, extra={}",
                self.deleted_ids_missing.len(),
                self.deleted_ids_extra.len()
            ));
        }
    }

    /// Create a comparison including version tracking validation.
    ///
    /// This method extends the basic comparison with version tracking:
    /// - Validates that input versions match expected versions
    /// - Validates that output versions are input + 1 for mutations
    /// - Validates that created objects have no input version
    ///
    /// # Arguments
    /// * `on_chain` - On-chain transaction effects summary
    /// * `local_success` - Whether local execution succeeded
    /// * `local_created` - Number of locally created objects
    /// * `local_mutated` - Number of locally mutated objects
    /// * `local_deleted` - Number of locally deleted objects
    /// * `local_versions` - Optional version info from local execution
    /// * `expected_input_versions` - Expected input versions (from gRPC response)
    pub fn compare_with_versions(
        on_chain: &TransactionEffectsSummary,
        local_success: bool,
        local_created: usize,
        local_mutated: usize,
        local_deleted: usize,
        local_versions: Option<&HashMap<String, LocalVersionInfo>>,
        expected_input_versions: Option<&HashMap<String, u64>>,
    ) -> Self {
        // Start with basic comparison
        let mut comparison = Self::compare(
            on_chain,
            local_success,
            local_created,
            local_mutated,
            local_deleted,
        );

        // If version info is provided, perform version validation
        if let (Some(local_vers), Some(expected_vers)) = (local_versions, expected_input_versions) {
            comparison.version_tracking_enabled = true;

            let mut input_matched = 0;
            let mut input_total = 0;
            let mut increment_valid = 0;
            let mut increment_total = 0;
            let mut mismatches = Vec::new();

            for (obj_id, local_info) in local_vers {
                // Check input version matches expected
                if let Some(expected_input) = expected_vers.get(obj_id) {
                    input_total += 1;
                    if let Some(local_input) = local_info.input_version {
                        if local_input == *expected_input {
                            input_matched += 1;
                        } else {
                            mismatches.push(VersionMismatch {
                                object_id: obj_id.clone(),
                                mismatch_type: VersionMismatchType::InputVersion,
                                expected: Some(*expected_input),
                                actual: Some(local_input),
                            });
                        }
                    }
                }

                // Check version increment for mutated objects
                // Note: In Sui, all objects in a transaction get the SAME output version
                // (the lamport timestamp), which is max(input_versions) + 1.
                // So output > input is the correct validation, not output == input + 1.
                if let Some(input_v) = local_info.input_version {
                    increment_total += 1;
                    // Valid if output version > input version
                    if local_info.output_version > input_v {
                        increment_valid += 1;
                    } else {
                        mismatches.push(VersionMismatch {
                            object_id: obj_id.clone(),
                            mismatch_type: VersionMismatchType::OutputVersion,
                            expected: Some(input_v + 1), // Minimum expected
                            actual: Some(local_info.output_version),
                        });
                    }
                }
            }

            comparison.input_versions_matched = input_matched;
            comparison.input_versions_total = input_total;
            comparison.version_increments_valid = increment_valid;
            comparison.version_increments_total = increment_total;
            comparison.version_mismatches = mismatches;

            // Adjust match score to include version tracking
            if input_total > 0 || increment_total > 0 {
                let version_score = if input_total + increment_total > 0 {
                    (input_matched + increment_valid) as f64
                        / (input_total + increment_total) as f64
                } else {
                    1.0
                };
                // Weight: 80% original comparison, 20% version tracking
                comparison.match_score = comparison.match_score * 0.8 + version_score * 0.2;
            }
        }

        comparison
    }
}

/// Simplified version info for comparison (without digest).
#[derive(Debug, Clone)]
pub struct LocalVersionInfo {
    /// Input version (None if created)
    pub input_version: Option<u64>,
    /// Output version after execution
    pub output_version: u64,
}

// Note: FetchedObject is now defined in fetched.rs with a richer API.
// The canonical type is crate::fetched::FetchedObject, re-exported from lib.rs.
//
// For legacy compatibility, code that only needs (bcs_bytes, type_string, is_shared,
// is_immutable, version) can construct a FetchedObject with:
//   FetchedObject::new(object_id, version, bcs_bytes)
//       .with_type(type_string)
//       .shared()  // or .immutable()

/// Entry for a dynamic field child (from JSON-RPC `suix_getDynamicFields`).
#[derive(Debug, Clone)]
pub struct DynamicFieldEntry {
    /// Type of the key/name (e.g., "u64", "0x2::object::ID")
    pub name_type: String,
    /// JSON value of the key/name
    pub name_json: Option<serde_json::Value>,
    /// BCS-encoded key bytes
    pub name_bcs: Option<Vec<u8>>,
    /// Object ID of the dynamic field wrapper
    pub object_id: String,
    /// Full type of the stored value
    pub object_type: Option<String>,
    /// Version of the wrapper object
    pub version: Option<u64>,
    /// Digest of the wrapper object
    pub digest: Option<String>,
}

// ============================================================================
// Transaction Cache Types
// ============================================================================

/// Cached transaction data including packages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTransaction {
    /// The fetched transaction
    pub transaction: FetchedTransaction,
    /// Cached package bytecode (package_id -> [(module_name, module_bytes_base64)])
    pub packages: HashMap<String, Vec<(String, String)>>,
    /// Input object data (object_id -> bytes_base64)
    pub objects: HashMap<String, String>,
    /// Object type information (object_id -> type_string)
    #[serde(default)]
    pub object_types: HashMap<String, String>,
    /// Object versions at transaction time (object_id -> version)
    #[serde(default)]
    pub object_versions: HashMap<String, u64>,
    /// Historical object data at transaction-time versions (object_id -> bytes_base64)
    /// These are objects fetched at their specific version from gRPC archive
    #[serde(default)]
    pub historical_objects: HashMap<String, String>,
    /// Dynamic field children (child_id -> CachedDynamicField)
    /// Pre-fetched dynamic field data for replay
    #[serde(default)]
    pub dynamic_field_children: HashMap<String, CachedDynamicField>,
    /// Package upgrade mappings (original_address -> upgraded_address)
    /// Maps original package addresses to their upgraded versions
    #[serde(default)]
    pub package_upgrades: HashMap<String, String>,
    /// Cache timestamp
    pub cached_at: u64,
}

/// Cached dynamic field child data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedDynamicField {
    /// Parent object ID
    pub parent_id: String,
    /// Type string of the dynamic field
    pub type_string: String,
    /// BCS bytes (base64 encoded)
    pub bcs_base64: String,
    /// Version at which this was fetched
    pub version: u64,
}

impl CachedTransaction {
    /// Create a new cached transaction.
    pub fn new(tx: FetchedTransaction) -> Self {
        Self {
            transaction: tx,
            packages: HashMap::new(),
            objects: HashMap::new(),
            object_types: HashMap::new(),
            object_versions: HashMap::new(),
            historical_objects: HashMap::new(),
            dynamic_field_children: HashMap::new(),
            package_upgrades: HashMap::new(),
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    /// Add package bytecode to the cache.
    pub fn add_package(&mut self, package_id: String, modules: Vec<(String, Vec<u8>)>) {
        let encoded: Vec<(String, String)> = modules
            .into_iter()
            .map(|(name, bytes)| {
                (
                    name,
                    base64_encode(&bytes),
                )
            })
            .collect();
        self.packages.insert(package_id, encoded);
    }

    /// Add object data to the cache.
    pub fn add_object(&mut self, object_id: String, bytes: Vec<u8>) {
        self.objects.insert(
            object_id,
            base64_encode(&bytes),
        );
    }

    /// Add object data with type information to the cache.
    pub fn add_object_with_type(
        &mut self,
        object_id: String,
        bytes: Vec<u8>,
        object_type: Option<String>,
    ) {
        self.objects.insert(
            object_id.clone(),
            base64_encode(&bytes),
        );
        if let Some(type_str) = object_type {
            self.object_types.insert(object_id, type_str);
        }
    }

    /// Get decoded package modules.
    pub fn get_package_modules(&self, package_id: &str) -> Option<Vec<(String, Vec<u8>)>> {
        self.packages.get(package_id).map(|modules| {
            modules
                .iter()
                .filter_map(|(name, b64)| {
                    try_base64_decode(b64).map(|bytes| (name.clone(), bytes))
                })
                .collect()
        })
    }

    /// Get decoded object bytes.
    pub fn get_object_bytes(&self, object_id: &str) -> Option<Vec<u8>> {
        self.objects
            .get(object_id)
            .and_then(|b64| try_base64_decode(b64))
    }

    /// Get historical object bytes (at transaction-time version).
    /// Falls back to regular objects if no historical version is cached.
    pub fn get_historical_object_bytes(&self, object_id: &str) -> Option<Vec<u8>> {
        // Try historical first, then fall back to regular objects
        self.historical_objects
            .get(object_id)
            .or_else(|| self.objects.get(object_id))
            .and_then(|b64| try_base64_decode(b64))
    }

    /// Get object version at transaction time.
    pub fn get_object_version(&self, object_id: &str) -> Option<u64> {
        self.object_versions.get(object_id).copied()
    }

    /// Add historical object data to the cache.
    pub fn add_historical_object(&mut self, object_id: String, bytes: Vec<u8>, version: u64) {
        self.historical_objects.insert(
            object_id.clone(),
            base64_encode(&bytes),
        );
        self.object_versions.insert(object_id, version);
    }

    /// Add a dynamic field child to the cache.
    pub fn add_dynamic_field_child(
        &mut self,
        child_id: String,
        parent_id: String,
        type_string: String,
        bytes: Vec<u8>,
        version: u64,
    ) {
        self.dynamic_field_children.insert(
            child_id,
            CachedDynamicField {
                parent_id,
                type_string,
                bcs_base64: base64_encode(&bytes),
                version,
            },
        );
    }

    /// Get decoded dynamic field child data.
    pub fn get_dynamic_field_child(
        &self,
        child_id: &str,
    ) -> Option<(String, String, Vec<u8>, u64)> {
        self.dynamic_field_children.get(child_id).and_then(|df| {
            try_base64_decode(&df.bcs_base64).map(|bytes| {
                (
                    df.parent_id.clone(),
                    df.type_string.clone(),
                    bytes,
                    df.version,
                )
            })
        })
    }

    /// Get all dynamic field children for a parent.
    pub fn get_dynamic_fields_for_parent(
        &self,
        parent_id: &str,
    ) -> Vec<(String, String, Vec<u8>, u64)> {
        self.dynamic_field_children
            .iter()
            .filter(|(_, df)| df.parent_id == parent_id)
            .filter_map(|(child_id, df)| {
                try_base64_decode(&df.bcs_base64)
                    .map(|bytes| (child_id.clone(), df.type_string.clone(), bytes, df.version))
            })
            .collect()
    }

    /// Add a package upgrade mapping.
    pub fn add_package_upgrade(&mut self, original_address: String, upgraded_address: String) {
        self.package_upgrades
            .insert(original_address, upgraded_address);
    }

    /// Get the upgraded package address for an original address.
    pub fn get_upgraded_package(&self, original_address: &str) -> Option<&String> {
        self.package_upgrades.get(original_address)
    }

    /// Get objects map with historical objects merged in.
    /// Historical objects take precedence over regular objects.
    pub fn get_merged_objects(&self) -> HashMap<String, String> {
        let mut merged = self.objects.clone();
        for (id, b64) in &self.historical_objects {
            merged.insert(id.clone(), b64.clone());
        }
        merged
    }
}

/// Transaction cache for storing fetched transactions and their dependencies.
pub struct TransactionCache {
    /// Cache directory path
    cache_dir: std::path::PathBuf,
}

impl TransactionCache {
    /// Create a new transaction cache with the given directory.
    pub fn new(cache_dir: impl Into<std::path::PathBuf>) -> std::io::Result<Self> {
        let cache_dir = cache_dir.into();
        std::fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    /// Get the cache file path for a transaction digest.
    fn cache_path(&self, digest: &str) -> std::path::PathBuf {
        self.cache_dir.join(format!("{}.json", digest))
    }

    /// Check if a transaction is cached.
    pub fn has(&self, digest: &str) -> bool {
        self.cache_path(digest).exists()
    }

    /// Load a cached transaction.
    pub fn load(&self, digest: &str) -> std::io::Result<CachedTransaction> {
        let path = self.cache_path(digest);
        let content = std::fs::read_to_string(&path)?;
        let cached: CachedTransaction = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(cached)
    }

    /// Save a transaction to the cache.
    pub fn save(&self, cached: &CachedTransaction) -> std::io::Result<()> {
        let path = self.cache_path(&cached.transaction.digest.0);
        let content = serde_json::to_string_pretty(cached)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// List all cached transaction digests.
    pub fn list(&self) -> std::io::Result<Vec<String>> {
        let mut digests = Vec::new();
        for entry in std::fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem() {
                    digests.push(stem.to_string_lossy().to_string());
                }
            }
        }
        Ok(digests)
    }

    /// Get the number of cached transactions.
    pub fn count(&self) -> usize {
        self.list().map(|l| l.len()).unwrap_or(0)
    }

    /// Clear all cached transactions.
    pub fn clear(&self) -> std::io::Result<()> {
        for entry in std::fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    /// Get cache directory path.
    pub fn cache_dir(&self) -> &std::path::Path {
        &self.cache_dir
    }
}

// ============================================================================
// Serde helpers
// ============================================================================

/// Serde helper for base64 encoding/decoding Vec<u8>
pub mod base64_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_digest_new() {
        let digest = TransactionDigest::new("abc123");
        assert_eq!(digest.0, "abc123");
    }

    #[test]
    fn test_cached_transaction_new() {
        let tx = FetchedTransaction {
            digest: TransactionDigest::new("test"),
            sender: AccountAddress::ZERO,
            gas_budget: 1000,
            gas_price: 1,
            commands: vec![],
            inputs: vec![],
            effects: None,
            timestamp_ms: None,
            checkpoint: None,
        };

        let cached = CachedTransaction::new(tx);
        assert!(cached.packages.is_empty());
        assert!(cached.objects.is_empty());
        assert!(cached.cached_at > 0);
    }

    #[test]
    fn test_cached_transaction_add_package() {
        let tx = FetchedTransaction {
            digest: TransactionDigest::new("test"),
            sender: AccountAddress::ZERO,
            gas_budget: 1000,
            gas_price: 1,
            commands: vec![],
            inputs: vec![],
            effects: None,
            timestamp_ms: None,
            checkpoint: None,
        };

        let mut cached = CachedTransaction::new(tx);
        cached.add_package(
            "0x2".to_string(),
            vec![("module1".to_string(), vec![1, 2, 3])],
        );

        let modules = cached.get_package_modules("0x2").unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].0, "module1");
        assert_eq!(modules[0].1, vec![1, 2, 3]);
    }

    #[test]
    fn test_cached_transaction_add_object() {
        let tx = FetchedTransaction {
            digest: TransactionDigest::new("test"),
            sender: AccountAddress::ZERO,
            gas_budget: 1000,
            gas_price: 1,
            commands: vec![],
            inputs: vec![],
            effects: None,
            timestamp_ms: None,
            checkpoint: None,
        };

        let mut cached = CachedTransaction::new(tx);
        cached.add_object("0x123".to_string(), vec![4, 5, 6]);

        let bytes = cached.get_object_bytes("0x123").unwrap();
        assert_eq!(bytes, vec![4, 5, 6]);
    }

    #[test]
    fn test_effects_comparison_all_match() {
        let effects = TransactionEffectsSummary {
            status: TransactionStatus::Success,
            created: vec!["0x1".to_string()],
            mutated: vec!["0x2".to_string(), "0x3".to_string()], // +1 for gas
            deleted: vec![],
            wrapped: vec![],
            unwrapped: vec![],
            gas_used: GasSummary::default(),
            events_count: 0,
            shared_object_versions: HashMap::new(),
        };

        let comparison = EffectsComparison::compare(&effects, true, 1, 1, 0);
        assert!(comparison.status_match);
        assert!(comparison.created_count_match);
        assert!(comparison.mutated_count_match); // allows +1 for gas
        assert!(comparison.deleted_count_match);
        assert_eq!(comparison.match_score, 1.0);
    }

    #[test]
    fn test_effects_comparison_status_mismatch() {
        let effects = TransactionEffectsSummary {
            status: TransactionStatus::Success,
            created: vec![],
            mutated: vec![],
            deleted: vec![],
            wrapped: vec![],
            unwrapped: vec![],
            gas_used: GasSummary::default(),
            events_count: 0,
            shared_object_versions: HashMap::new(),
        };

        let comparison = EffectsComparison::compare(&effects, false, 0, 0, 0);
        assert!(!comparison.status_match);
        assert!(comparison
            .notes
            .iter()
            .any(|n| n.contains("Status mismatch")));
    }
}
