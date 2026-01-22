//! Transaction Replay Module
//!
//! This module provides types and utilities for replaying Sui transactions
//! in the local Move VM sandbox. This enables:
//!
//! 1. **Validation**: Compare local execution with on-chain effects
//! 2. **Training Data**: Generate input/output pairs for LLM training
//! 3. **Testing**: Use real transaction patterns for testing
//!
//! ## Architecture
//!
//! Transactions are fetched via GraphQL (see `DataFetcher`) and cached locally.
//! The cached transactions can then be replayed using the `FetchedTransaction::replay()` method.
//!
//! ```text
//! GraphQL → CachedTransaction → PTBCommands → LocalExecution → CompareEffects
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! // Load a cached transaction
//! let cache = TransactionCache::new(".tx-cache")?;
//! let cached = cache.load("digest_here")?;
//!
//! // Replay it locally
//! let result = cached.transaction.replay(&mut harness)?;
//! ```
//!
//! ## Known Limitations: Dynamic Field Traversal
//!
//! Some DeFi protocols (Cetus, Turbos) use `skip_list` data structures that store
//! tick data as dynamic fields. These present a replay challenge:
//!
//! 1. The skip_list computes tick indices at runtime during traversal
//! 2. Each computed index becomes a dynamic field lookup via `derive_dynamic_field_id()`
//! 3. We can pre-fetch known dynamic fields, but not indices computed during execution
//!
//! **Example**: A Cetus swap traverses: `head(0) → 481316 → 512756 → tail(887272)`.
//! If the swap needs tick `500000`, this is computed at runtime and we can't know
//! to pre-fetch it without simulating the entire traversal.
//!
//! **Workarounds**:
//! - Cache all dynamic field children at transaction time
//! - Use synthetic/mocked transactions for testing (see `synthetic_ptb_case_study.rs`)
//! - Pre-fetch all known tick indices for a pool

use anyhow::{anyhow, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use serde::{Deserialize, Serialize};

use crate::benchmark::ptb::{Argument, Command, InputValue, ObjectInput};
use crate::benchmark::vm::VMHarness;

// Re-export type parsing functions from the canonical location (types module)
// This maintains backwards compatibility while centralizing the implementation.
pub use crate::benchmark::types::{
    clear_type_cache as clear_type_tag_cache, parse_type_tag,
    type_cache_size as type_tag_cache_size,
};

/// Object ID type (32-byte address)
pub type ObjectID = AccountAddress;

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
    pub shared_object_versions: std::collections::HashMap<String, u64>,
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
        }
    }
}

/// Full object data returned from RPC.
#[derive(Debug, Clone)]
pub struct FetchedObject {
    /// BCS bytes of the object content.
    pub bcs_bytes: Vec<u8>,
    /// Type string (e.g., "0x2::coin::Coin<0x2::sui::SUI>").
    pub type_string: Option<String>,
    /// Whether the object is shared.
    pub is_shared: bool,
    /// Whether the object is immutable.
    pub is_immutable: bool,
    /// Object version.
    pub version: u64,
}

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
// Dynamic Field ID Derivation
// ============================================================================

/// Derive the object ID for a dynamic field given the parent UID, key type, and key value.
///
/// This implements the same formula as Sui's `dynamic_field::derive_dynamic_field_id`:
/// ```text
/// Blake2b256(0xf0 || parent || len(key_bytes) || key_bytes || bcs(key_type_tag))
/// ```
///
/// Where:
/// - `0xf0` is the `HashingIntentScope::ChildObjectId` prefix
/// - `parent` is the 32-byte parent UID address
/// - `len(key_bytes)` is the length as 8-byte little-endian (usize)
/// - `key_bytes` is the BCS-serialized key value
/// - `bcs(key_type_tag)` is the BCS-serialized TypeTag of the key
///
/// # Arguments
/// * `parent` - The parent object's UID address (32 bytes)
/// * `key_type_tag` - The Move TypeTag of the key (e.g., TypeTag::U64)
/// * `key_bytes` - The BCS-serialized key value
///
/// # Returns
/// The derived ObjectID (32 bytes) as an AccountAddress
///
/// # Example
/// ```ignore
/// use move_core_types::language_storage::TypeTag;
///
/// let parent = AccountAddress::from_hex_literal("0x6dd50d...").unwrap();
/// let key: u64 = 481316;
/// let key_bytes = bcs::to_bytes(&key).unwrap();
/// let obj_id = derive_dynamic_field_id(parent, &TypeTag::U64, &key_bytes).unwrap();
/// ```
pub fn derive_dynamic_field_id(
    parent: AccountAddress,
    key_type_tag: &TypeTag,
    key_bytes: &[u8],
) -> Result<AccountAddress> {
    use fastcrypto::hash::{Blake2b256, HashFunction};

    // HashingIntentScope::ChildObjectId = 0xf0
    const CHILD_OBJECT_ID_SCOPE: u8 = 0xf0;

    // BCS-serialize the type tag
    let type_tag_bytes = bcs::to_bytes(key_type_tag)
        .map_err(|e| anyhow!("Failed to BCS-serialize type tag: {}", e))?;

    // Build the input: scope || parent || len(key) || key || type_tag
    let mut input = Vec::with_capacity(1 + 32 + 8 + key_bytes.len() + type_tag_bytes.len());
    input.push(CHILD_OBJECT_ID_SCOPE);
    input.extend_from_slice(parent.as_ref());
    input.extend_from_slice(&(key_bytes.len() as u64).to_le_bytes());
    input.extend_from_slice(key_bytes);
    input.extend_from_slice(&type_tag_bytes);

    // Hash with Blake2b-256
    let hash = Blake2b256::digest(&input);

    // Convert to AccountAddress (hash.digest is [u8; 32])
    Ok(AccountAddress::new(hash.digest))
}

/// Derive the object ID for a dynamic field with a u64 key.
///
/// Convenience wrapper around `derive_dynamic_field_id` for the common case
/// of u64 keys (used by skip_list, table, etc.).
///
/// # Arguments
/// * `parent` - The parent object's UID address
/// * `key` - The u64 key value
///
/// # Returns
/// The derived ObjectID as an AccountAddress
pub fn derive_dynamic_field_id_u64(parent: AccountAddress, key: u64) -> Result<AccountAddress> {
    let key_bytes =
        bcs::to_bytes(&key).map_err(|e| anyhow!("Failed to BCS-serialize u64 key: {}", e))?;
    derive_dynamic_field_id(parent, &TypeTag::U64, &key_bytes)
}

// ============================================================================
// Transaction Cache
// ============================================================================

/// Cached transaction data including packages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTransaction {
    /// The fetched transaction
    pub transaction: FetchedTransaction,
    /// Cached package bytecode (package_id -> [(module_name, module_bytes_base64)])
    pub packages: std::collections::HashMap<String, Vec<(String, String)>>,
    /// Input object data (object_id -> bytes_base64)
    pub objects: std::collections::HashMap<String, String>,
    /// Object type information (object_id -> type_string)
    #[serde(default)]
    pub object_types: std::collections::HashMap<String, String>,
    /// Object versions at transaction time (object_id -> version)
    #[serde(default)]
    pub object_versions: std::collections::HashMap<String, u64>,
    /// Historical object data at transaction-time versions (object_id -> bytes_base64)
    /// These are objects fetched at their specific version from gRPC archive
    #[serde(default)]
    pub historical_objects: std::collections::HashMap<String, String>,
    /// Dynamic field children (child_id -> CachedDynamicField)
    /// Pre-fetched dynamic field data for replay
    #[serde(default)]
    pub dynamic_field_children: std::collections::HashMap<String, CachedDynamicField>,
    /// Package upgrade mappings (original_address -> upgraded_address)
    /// Maps original package addresses to their upgraded versions
    #[serde(default)]
    pub package_upgrades: std::collections::HashMap<String, String>,
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
            packages: std::collections::HashMap::new(),
            objects: std::collections::HashMap::new(),
            object_types: std::collections::HashMap::new(),
            object_versions: std::collections::HashMap::new(),
            historical_objects: std::collections::HashMap::new(),
            dynamic_field_children: std::collections::HashMap::new(),
            package_upgrades: std::collections::HashMap::new(),
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    /// Add package bytecode to the cache.
    pub fn add_package(&mut self, package_id: String, modules: Vec<(String, Vec<u8>)>) {
        use base64::Engine;
        let encoded: Vec<(String, String)> = modules
            .into_iter()
            .map(|(name, bytes)| {
                (
                    name,
                    base64::engine::general_purpose::STANDARD.encode(&bytes),
                )
            })
            .collect();
        self.packages.insert(package_id, encoded);
    }

    /// Add object data to the cache.
    pub fn add_object(&mut self, object_id: String, bytes: Vec<u8>) {
        use base64::Engine;
        self.objects.insert(
            object_id,
            base64::engine::general_purpose::STANDARD.encode(&bytes),
        );
    }

    /// Add object data with type information to the cache.
    pub fn add_object_with_type(
        &mut self,
        object_id: String,
        bytes: Vec<u8>,
        object_type: Option<String>,
    ) {
        use base64::Engine;
        self.objects.insert(
            object_id.clone(),
            base64::engine::general_purpose::STANDARD.encode(&bytes),
        );
        if let Some(type_str) = object_type {
            self.object_types.insert(object_id, type_str);
        }
    }

    /// Get decoded package modules.
    pub fn get_package_modules(&self, package_id: &str) -> Option<Vec<(String, Vec<u8>)>> {
        use base64::Engine;
        self.packages.get(package_id).map(|modules| {
            modules
                .iter()
                .filter_map(|(name, b64)| {
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .ok()
                        .map(|bytes| (name.clone(), bytes))
                })
                .collect()
        })
    }

    /// Get decoded object bytes.
    pub fn get_object_bytes(&self, object_id: &str) -> Option<Vec<u8>> {
        use base64::Engine;
        self.objects
            .get(object_id)
            .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
    }

    /// Get historical object bytes (at transaction-time version).
    /// Falls back to regular objects if no historical version is cached.
    pub fn get_historical_object_bytes(&self, object_id: &str) -> Option<Vec<u8>> {
        use base64::Engine;
        // Try historical first, then fall back to regular objects
        self.historical_objects
            .get(object_id)
            .or_else(|| self.objects.get(object_id))
            .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
    }

    /// Get object version at transaction time.
    pub fn get_object_version(&self, object_id: &str) -> Option<u64> {
        self.object_versions.get(object_id).copied()
    }

    /// Add historical object data to the cache.
    pub fn add_historical_object(&mut self, object_id: String, bytes: Vec<u8>, version: u64) {
        use base64::Engine;
        self.historical_objects.insert(
            object_id.clone(),
            base64::engine::general_purpose::STANDARD.encode(&bytes),
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
        use base64::Engine;
        self.dynamic_field_children.insert(
            child_id,
            CachedDynamicField {
                parent_id,
                type_string,
                bcs_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                version,
            },
        );
    }

    /// Get decoded dynamic field child data.
    pub fn get_dynamic_field_child(
        &self,
        child_id: &str,
    ) -> Option<(String, String, Vec<u8>, u64)> {
        use base64::Engine;
        self.dynamic_field_children.get(child_id).and_then(|df| {
            base64::engine::general_purpose::STANDARD
                .decode(&df.bcs_base64)
                .ok()
                .map(|bytes| {
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
        use base64::Engine;
        self.dynamic_field_children
            .iter()
            .filter(|(_, df)| df.parent_id == parent_id)
            .filter_map(|(child_id, df)| {
                base64::engine::general_purpose::STANDARD
                    .decode(&df.bcs_base64)
                    .ok()
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
    pub fn get_merged_objects(&self) -> std::collections::HashMap<String, String> {
        let mut merged = self.objects.clone();
        for (id, b64) in &self.historical_objects {
            merged.insert(id.clone(), b64.clone());
        }
        merged
    }

    /// Convert to PTB commands using cached object data.
    pub fn to_ptb_commands(&self) -> Result<(Vec<InputValue>, Vec<Command>)> {
        self.transaction.to_ptb_commands_with_objects(&self.objects)
    }
}

/// Transaction cache for storing fetched transactions and their dependencies.
pub struct TransactionCache {
    /// Cache directory path
    cache_dir: std::path::PathBuf,
}

impl TransactionCache {
    /// Create a new transaction cache with the given directory.
    pub fn new(cache_dir: impl Into<std::path::PathBuf>) -> Result<Self> {
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
    pub fn load(&self, digest: &str) -> Result<CachedTransaction> {
        let path = self.cache_path(digest);
        let content = std::fs::read_to_string(&path)?;
        let cached: CachedTransaction = serde_json::from_str(&content)?;
        Ok(cached)
    }

    /// Save a transaction to the cache.
    pub fn save(&self, cached: &CachedTransaction) -> Result<()> {
        let path = self.cache_path(&cached.transaction.digest.0);
        let content = serde_json::to_string_pretty(cached)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// List all cached transaction digests.
    pub fn list(&self) -> Result<Vec<String>> {
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
    pub fn clear(&self) -> Result<usize> {
        let digests = self.list()?;
        let count = digests.len();
        for digest in digests {
            let path = self.cache_path(&digest);
            std::fs::remove_file(path)?;
        }
        Ok(count)
    }
}

// ============================================================================
// Parallel Replay
// ============================================================================

/// Result of a parallel replay operation.
#[derive(Debug, Clone, Serialize)]
pub struct ParallelReplayResult {
    /// Total transactions processed
    pub total: usize,
    /// Successfully executed locally
    pub successful: usize,
    /// Status matched with on-chain
    pub status_matched: usize,
    /// Individual results
    pub results: Vec<ReplayResult>,
    /// Processing time in milliseconds
    pub elapsed_ms: u64,
    /// Transactions per second
    pub tps: f64,
}

/// Build address alias map by examining the bytecode self-addresses.
/// Returns a map: on-chain package ID (runtime) -> bytecode self-address
/// This allows module resolution: when looking for runtime address, fall back to bytecode.
///
/// Note: For hash rewriting (bytecode -> runtime), callers should build an inverted map.
fn build_address_aliases(
    cached: &CachedTransaction,
) -> std::collections::HashMap<AccountAddress, AccountAddress> {
    use move_binary_format::file_format::CompiledModule;

    let mut aliases = std::collections::HashMap::new();

    for pkg_id in cached.packages.keys() {
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            // Get the runtime address (on-chain package ID)
            let runtime_addr = match AccountAddress::from_hex_literal(pkg_id) {
                Ok(addr) => addr,
                Err(_) => continue,
            };

            // Find the bytecode address from the module
            for (_name, bytes) in &modules {
                if bytes.is_empty() {
                    continue;
                }
                if let Ok(module) = CompiledModule::deserialize_with_defaults(bytes) {
                    let bytecode_addr = *module.self_id().address();
                    if bytecode_addr != runtime_addr {
                        // Map runtime address -> bytecode address (for module resolution)
                        aliases.insert(runtime_addr, bytecode_addr);
                    }
                    break; // All modules in a package have the same address
                }
            }
        }
    }

    aliases
}

/// Public wrapper for testing - builds address aliases for a cached transaction.
pub fn build_address_aliases_for_test(
    cached: &CachedTransaction,
) -> std::collections::HashMap<AccountAddress, AccountAddress> {
    build_address_aliases(cached)
}

/// Replay multiple transactions in parallel.
///
/// This function uses rayon for parallel execution, creating a separate
/// VMHarness for each thread to avoid contention.
pub fn replay_parallel(
    transactions: &[CachedTransaction],
    resolver: &crate::benchmark::resolver::LocalModuleResolver,
    num_threads: Option<usize>,
) -> Result<ParallelReplayResult> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    // Configure thread pool
    if let Some(threads) = num_threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .ok(); // Ignore if already configured
    }

    let start = Instant::now();
    let total = transactions.len();
    let successful = AtomicUsize::new(0);
    let status_matched = AtomicUsize::new(0);

    // Process transactions in parallel
    let results: Vec<ReplayResult> = transactions
        .par_iter()
        .map(|cached| {
            // Create a resolver with cached packages
            let mut local_resolver = resolver.clone();

            // Build address alias map for this transaction
            let address_aliases = build_address_aliases(cached);

            // Load cached packages into the resolver
            for pkg_id in cached.packages.keys() {
                if let Some(modules) = cached.get_package_modules(pkg_id) {
                    // Don't use target address aliasing - we'll rewrite the transaction instead
                    let _ = local_resolver.add_package_modules(modules);
                }
            }

            // Create harness and replay with address rewriting
            match VMHarness::new(&local_resolver, false) {
                Ok(mut harness) => {
                    match cached.transaction.replay_with_objects_and_aliases(
                        &mut harness,
                        &cached.objects,
                        &address_aliases,
                    ) {
                        Ok(result) => {
                            if result.local_success {
                                successful.fetch_add(1, Ordering::Relaxed);
                            }
                            if result
                                .comparison
                                .as_ref()
                                .map(|c| c.status_match)
                                .unwrap_or(false)
                            {
                                status_matched.fetch_add(1, Ordering::Relaxed);
                            }
                            result
                        }
                        Err(e) => ReplayResult {
                            digest: cached.transaction.digest.clone(),
                            local_success: false,
                            local_error: Some(e.to_string()),
                            comparison: None,
                            commands_executed: 0,
                            commands_failed: cached.transaction.commands.len(),
                        },
                    }
                }
                Err(e) => ReplayResult {
                    digest: cached.transaction.digest.clone(),
                    local_success: false,
                    local_error: Some(format!("Failed to create harness: {}", e)),
                    comparison: None,
                    commands_executed: 0,
                    commands_failed: cached.transaction.commands.len(),
                },
            }
        })
        .collect();

    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    let tps = if elapsed_ms > 0 {
        (total as f64 * 1000.0) / elapsed_ms as f64
    } else {
        0.0
    };

    Ok(ParallelReplayResult {
        total,
        successful: successful.load(Ordering::Relaxed),
        status_matched: status_matched.load(Ordering::Relaxed),
        results,
        elapsed_ms,
        tps,
    })
}

impl FetchedTransaction {
    /// Convert this transaction to PTB commands for local execution.
    pub fn to_ptb_commands(&self) -> Result<(Vec<InputValue>, Vec<Command>)> {
        // Use a large default balance for simulation (1 billion SUI = 10^18 MIST)
        // This ensures SplitCoins won't fail due to insufficient balance
        // The actual gas coin balance on-chain is typically much larger than gas_budget
        const DEFAULT_GAS_BALANCE: u64 = 1_000_000_000_000_000_000; // 1B SUI in MIST
        self.to_ptb_commands_internal(DEFAULT_GAS_BALANCE, &std::collections::HashMap::new())
    }

    /// Convert this transaction to PTB commands using cached object data.
    pub fn to_ptb_commands_with_objects(
        &self,
        cached_objects: &std::collections::HashMap<String, String>,
    ) -> Result<(Vec<InputValue>, Vec<Command>)> {
        const DEFAULT_GAS_BALANCE: u64 = 1_000_000_000_000_000_000;
        self.to_ptb_commands_internal(DEFAULT_GAS_BALANCE, cached_objects)
    }

    /// Convert this transaction to PTB commands with address rewriting.
    /// The aliases map on-chain package addresses to bytecode self-addresses.
    pub fn to_ptb_commands_with_objects_and_aliases(
        &self,
        cached_objects: &std::collections::HashMap<String, String>,
        address_aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
    ) -> Result<(Vec<InputValue>, Vec<Command>)> {
        const DEFAULT_GAS_BALANCE: u64 = 1_000_000_000_000_000_000;
        self.to_ptb_commands_internal_with_aliases(
            DEFAULT_GAS_BALANCE,
            cached_objects,
            address_aliases,
        )
    }

    /// Convert this transaction to PTB commands, providing a gas coin with specified balance.
    pub fn to_ptb_commands_with_gas_budget(
        &self,
        gas_balance: u64,
    ) -> Result<(Vec<InputValue>, Vec<Command>)> {
        self.to_ptb_commands_internal(gas_balance, &std::collections::HashMap::new())
    }

    /// Internal method that converts to PTB commands with gas balance and optional cached objects.
    fn to_ptb_commands_internal(
        &self,
        gas_balance: u64,
        cached_objects: &std::collections::HashMap<String, String>,
    ) -> Result<(Vec<InputValue>, Vec<Command>)> {
        use base64::Engine;
        let mut inputs = Vec::new();
        let mut commands = Vec::new();

        // Helper to get object bytes from cache
        let get_object_bytes = |object_id: &str| -> Vec<u8> {
            cached_objects
                .get(object_id)
                .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
                .unwrap_or_else(|| vec![0u8; 32]) // Fallback placeholder
        };

        // Check if any command uses GasCoin
        let uses_gas_coin = self.commands.iter().any(|cmd| match cmd {
            PtbCommand::SplitCoins { coin, .. } => matches!(coin, PtbArgument::GasCoin),
            PtbCommand::MergeCoins {
                destination,
                sources,
            } => {
                matches!(destination, PtbArgument::GasCoin)
                    || sources.iter().any(|s| matches!(s, PtbArgument::GasCoin))
            }
            PtbCommand::TransferObjects { objects, .. } => {
                objects.iter().any(|o| matches!(o, PtbArgument::GasCoin))
            }
            _ => false,
        });

        // Input index offset: if we prepend GasCoin, all other input indices shift by 1
        let input_offset: u16 = if uses_gas_coin { 1 } else { 0 };

        // If uses GasCoin, prepend a synthetic gas coin object
        if uses_gas_coin {
            // Create a synthetic Coin<SUI> with the gas budget as balance
            // Coin<T> layout: id (UID = 32 bytes) + balance (u64 = 8 bytes) = 40 bytes
            let mut gas_coin_bytes = vec![0u8; 32]; // UID (placeholder)
            gas_coin_bytes.extend_from_slice(&gas_balance.to_le_bytes()); // Balance
            inputs.push(InputValue::Object(ObjectInput::Owned {
                id: AccountAddress::ZERO, // Placeholder gas coin ID
                bytes: gas_coin_bytes,
                type_tag: None, // Gas coin type is known to be Coin<SUI>
            }));
        }

        // Convert inputs, using cached object data when available
        for input in &self.inputs {
            match input {
                TransactionInput::Pure { bytes } => {
                    inputs.push(InputValue::Pure(bytes.clone()));
                }
                TransactionInput::Object { object_id, .. } => {
                    let id =
                        AccountAddress::from_hex_literal(object_id).unwrap_or(AccountAddress::ZERO);
                    let bytes = get_object_bytes(object_id);
                    inputs.push(InputValue::Object(ObjectInput::Owned {
                        id,
                        bytes,
                        type_tag: None,
                    }));
                }
                TransactionInput::SharedObject { object_id, .. } => {
                    let id =
                        AccountAddress::from_hex_literal(object_id).unwrap_or(AccountAddress::ZERO);
                    let bytes = get_object_bytes(object_id);
                    inputs.push(InputValue::Object(ObjectInput::Shared {
                        id,
                        bytes,
                        type_tag: None,
                    }));
                }
                TransactionInput::ImmutableObject { object_id, .. } => {
                    let id =
                        AccountAddress::from_hex_literal(object_id).unwrap_or(AccountAddress::ZERO);
                    let bytes = get_object_bytes(object_id);
                    // Use ImmRef for immutable objects (e.g., packages, Clock)
                    inputs.push(InputValue::Object(ObjectInput::ImmRef {
                        id,
                        bytes,
                        type_tag: None,
                    }));
                }
                TransactionInput::Receiving { object_id, .. } => {
                    let id =
                        AccountAddress::from_hex_literal(object_id).unwrap_or(AccountAddress::ZERO);
                    let bytes = get_object_bytes(object_id);
                    // Receiving objects are treated as owned for replay purposes
                    inputs.push(InputValue::Object(ObjectInput::Owned {
                        id,
                        bytes,
                        type_tag: None,
                    }));
                }
            }
        }

        // Helper to convert arguments with input offset
        let convert_arg = |arg: &PtbArgument| -> Argument {
            match arg {
                PtbArgument::Input { index } => Argument::Input(index + input_offset),
                PtbArgument::Result { index } => Argument::Result(*index),
                PtbArgument::NestedResult {
                    index,
                    result_index,
                } => Argument::NestedResult(*index, *result_index),
                PtbArgument::GasCoin => Argument::Input(0), // GasCoin is always input 0 (prepended)
            }
        };

        // Convert commands (with input offset if using GasCoin)
        for cmd in &self.commands {
            match cmd {
                PtbCommand::MoveCall {
                    package,
                    module,
                    function,
                    type_arguments,
                    arguments,
                } => {
                    let package_addr = AccountAddress::from_hex_literal(package)
                        .map_err(|e| anyhow!("Invalid package address: {}", e))?;
                    let module_id = Identifier::new(module.clone())
                        .map_err(|e| anyhow!("Invalid module name: {}", e))?;
                    let function_id = Identifier::new(function.clone())
                        .map_err(|e| anyhow!("Invalid function name: {}", e))?;

                    // Parse type arguments from RPC strings
                    let type_args: Vec<TypeTag> = type_arguments
                        .iter()
                        .filter_map(|s| parse_type_tag(s).ok())
                        .collect();

                    // Convert arguments
                    let args: Vec<Argument> = arguments.iter().map(&convert_arg).collect();

                    commands.push(Command::MoveCall {
                        package: package_addr,
                        module: module_id,
                        function: function_id,
                        type_args,
                        args,
                    });
                }

                PtbCommand::SplitCoins { coin, amounts } => {
                    let coin_arg = convert_arg(coin);
                    let amount_args: Vec<Argument> = amounts.iter().map(&convert_arg).collect();
                    commands.push(Command::SplitCoins {
                        coin: coin_arg,
                        amounts: amount_args,
                    });
                }

                PtbCommand::MergeCoins {
                    destination,
                    sources,
                } => {
                    let dest_arg = convert_arg(destination);
                    let source_args: Vec<Argument> = sources.iter().map(&convert_arg).collect();
                    commands.push(Command::MergeCoins {
                        destination: dest_arg,
                        sources: source_args,
                    });
                }

                PtbCommand::TransferObjects { objects, address } => {
                    let obj_args: Vec<Argument> = objects.iter().map(&convert_arg).collect();
                    let addr_arg = convert_arg(address);
                    commands.push(Command::TransferObjects {
                        objects: obj_args,
                        address: addr_arg,
                    });
                }

                PtbCommand::MakeMoveVec { type_arg, elements } => {
                    let type_tag = type_arg.as_ref().and_then(|s| parse_type_tag(s).ok());
                    let elem_args: Vec<Argument> = elements.iter().map(&convert_arg).collect();
                    commands.push(Command::MakeMoveVec {
                        type_tag,
                        elements: elem_args,
                    });
                }

                PtbCommand::Publish { .. } | PtbCommand::Upgrade { .. } => {
                    // Skip publish/upgrade for now
                }
            }
        }

        Ok((inputs, commands))
    }

    /// Internal method with address aliasing support for package upgrades.
    fn to_ptb_commands_internal_with_aliases(
        &self,
        gas_balance: u64,
        cached_objects: &std::collections::HashMap<String, String>,
        address_aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
    ) -> Result<(Vec<InputValue>, Vec<Command>)> {
        use base64::Engine;
        let mut inputs = Vec::new();
        let mut commands = Vec::new();

        // Helper to get object bytes from cache
        let get_object_bytes = |object_id: &str| -> Vec<u8> {
            cached_objects
                .get(object_id)
                .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
                .unwrap_or_else(|| vec![0u8; 32])
        };

        // Helper to rewrite address if aliased
        let rewrite_addr = |addr: AccountAddress| -> AccountAddress {
            address_aliases.get(&addr).copied().unwrap_or(addr)
        };

        // Helper to rewrite addresses in type tags
        fn rewrite_type_tag(
            tag: TypeTag,
            aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
        ) -> TypeTag {
            match tag {
                TypeTag::Struct(s) => {
                    let mut s = *s;
                    s.address = aliases.get(&s.address).copied().unwrap_or(s.address);
                    s.type_params = s
                        .type_params
                        .into_iter()
                        .map(|t| rewrite_type_tag(t, aliases))
                        .collect();
                    TypeTag::Struct(Box::new(s))
                }
                TypeTag::Vector(inner) => {
                    TypeTag::Vector(Box::new(rewrite_type_tag(*inner, aliases)))
                }
                other => other,
            }
        }

        // Check if any command uses GasCoin
        let uses_gas_coin = self.commands.iter().any(|cmd| match cmd {
            PtbCommand::SplitCoins { coin, .. } => matches!(coin, PtbArgument::GasCoin),
            PtbCommand::MergeCoins {
                destination,
                sources,
            } => {
                matches!(destination, PtbArgument::GasCoin)
                    || sources.iter().any(|s| matches!(s, PtbArgument::GasCoin))
            }
            PtbCommand::TransferObjects { objects, .. } => {
                objects.iter().any(|o| matches!(o, PtbArgument::GasCoin))
            }
            _ => false,
        });

        let input_offset: u16 = if uses_gas_coin { 1 } else { 0 };

        if uses_gas_coin {
            let mut gas_coin_bytes = vec![0u8; 32];
            gas_coin_bytes.extend_from_slice(&gas_balance.to_le_bytes());
            inputs.push(InputValue::Object(ObjectInput::Owned {
                id: AccountAddress::ZERO,
                bytes: gas_coin_bytes,
                type_tag: None,
            }));
        }

        // Convert inputs
        for input in &self.inputs {
            match input {
                TransactionInput::Pure { bytes } => {
                    inputs.push(InputValue::Pure(bytes.clone()));
                }
                TransactionInput::Object { object_id, .. } => {
                    let id =
                        AccountAddress::from_hex_literal(object_id).unwrap_or(AccountAddress::ZERO);
                    let bytes = get_object_bytes(object_id);
                    inputs.push(InputValue::Object(ObjectInput::Owned {
                        id,
                        bytes,
                        type_tag: None,
                    }));
                }
                TransactionInput::SharedObject { object_id, .. } => {
                    let id =
                        AccountAddress::from_hex_literal(object_id).unwrap_or(AccountAddress::ZERO);
                    let bytes = get_object_bytes(object_id);
                    inputs.push(InputValue::Object(ObjectInput::Shared {
                        id,
                        bytes,
                        type_tag: None,
                    }));
                }
                TransactionInput::ImmutableObject { object_id, .. } => {
                    let id =
                        AccountAddress::from_hex_literal(object_id).unwrap_or(AccountAddress::ZERO);
                    let bytes = get_object_bytes(object_id);
                    inputs.push(InputValue::Object(ObjectInput::ImmRef {
                        id,
                        bytes,
                        type_tag: None,
                    }));
                }
                TransactionInput::Receiving { object_id, .. } => {
                    let id =
                        AccountAddress::from_hex_literal(object_id).unwrap_or(AccountAddress::ZERO);
                    let bytes = get_object_bytes(object_id);
                    inputs.push(InputValue::Object(ObjectInput::Owned {
                        id,
                        bytes,
                        type_tag: None,
                    }));
                }
            }
        }

        let convert_arg = |arg: &PtbArgument| -> Argument {
            match arg {
                PtbArgument::Input { index } => Argument::Input(index + input_offset),
                PtbArgument::Result { index } => Argument::Result(*index),
                PtbArgument::NestedResult {
                    index,
                    result_index,
                } => Argument::NestedResult(*index, *result_index),
                PtbArgument::GasCoin => Argument::Input(0),
            }
        };

        // Convert commands with address rewriting
        for cmd in &self.commands {
            match cmd {
                PtbCommand::MoveCall {
                    package,
                    module,
                    function,
                    type_arguments,
                    arguments,
                } => {
                    let package_addr = AccountAddress::from_hex_literal(package)
                        .map_err(|e| anyhow!("Invalid package address: {}", e))?;
                    // Rewrite package address to bytecode self-address
                    let rewritten_package = rewrite_addr(package_addr);
                    let module_id = Identifier::new(module.clone())
                        .map_err(|e| anyhow!("Invalid module name: {}", e))?;
                    let function_id = Identifier::new(function.clone())
                        .map_err(|e| anyhow!("Invalid function name: {}", e))?;

                    // Parse and rewrite type arguments
                    let type_args: Vec<TypeTag> = type_arguments
                        .iter()
                        .filter_map(|s| parse_type_tag(s).ok())
                        .map(|t| rewrite_type_tag(t, address_aliases))
                        .collect();

                    let args: Vec<Argument> = arguments.iter().map(&convert_arg).collect();

                    commands.push(Command::MoveCall {
                        package: rewritten_package,
                        module: module_id,
                        function: function_id,
                        type_args,
                        args,
                    });
                }

                PtbCommand::SplitCoins { coin, amounts } => {
                    commands.push(Command::SplitCoins {
                        coin: convert_arg(coin),
                        amounts: amounts.iter().map(&convert_arg).collect(),
                    });
                }

                PtbCommand::MergeCoins {
                    destination,
                    sources,
                } => {
                    commands.push(Command::MergeCoins {
                        destination: convert_arg(destination),
                        sources: sources.iter().map(&convert_arg).collect(),
                    });
                }

                PtbCommand::TransferObjects { objects, address } => {
                    commands.push(Command::TransferObjects {
                        objects: objects.iter().map(&convert_arg).collect(),
                        address: convert_arg(address),
                    });
                }

                PtbCommand::MakeMoveVec { type_arg, elements } => {
                    let type_tag = type_arg
                        .as_ref()
                        .and_then(|s| parse_type_tag(s).ok())
                        .map(|t| rewrite_type_tag(t, address_aliases));
                    commands.push(Command::MakeMoveVec {
                        type_tag,
                        elements: elements.iter().map(&convert_arg).collect(),
                    });
                }

                PtbCommand::Publish { .. } | PtbCommand::Upgrade { .. } => {
                    // Skip publish/upgrade
                }
            }
        }

        Ok((inputs, commands))
    }

    /// Replay this transaction in the local VM.
    ///
    /// This method executes the transaction commands using PTBExecutor and compares
    /// the results with on-chain effects.
    pub fn replay(&self, harness: &mut VMHarness) -> Result<ReplayResult> {
        self.replay_with_objects(harness, &std::collections::HashMap::new())
    }

    /// Replay this transaction using cached object data.
    pub fn replay_with_objects(
        &self,
        harness: &mut VMHarness,
        cached_objects: &std::collections::HashMap<String, String>,
    ) -> Result<ReplayResult> {
        self.replay_with_objects_and_aliases(
            harness,
            cached_objects,
            &std::collections::HashMap::new(),
        )
    }

    /// Replay this transaction using cached object data and address aliases.
    /// The aliases map on-chain package addresses to bytecode self-addresses.
    pub fn replay_with_objects_and_aliases(
        &self,
        harness: &mut VMHarness,
        cached_objects: &std::collections::HashMap<String, String>,
        address_aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
    ) -> Result<ReplayResult> {
        use crate::benchmark::ptb::PTBExecutor;

        let (inputs, commands) =
            self.to_ptb_commands_with_objects_and_aliases(cached_objects, address_aliases)?;
        let commands_count = commands.len();

        // Execute using PTBExecutor
        let mut executor = PTBExecutor::new(harness);

        // Add inputs to executor
        for input in &inputs {
            executor.add_input(input.clone());
        }

        // Execute commands
        let effects = match executor.execute_commands(&commands) {
            Ok(effects) => effects,
            Err(e) => {
                return Ok(ReplayResult {
                    digest: self.digest.clone(),
                    local_success: false,
                    local_error: Some(e.to_string()),
                    comparison: None,
                    commands_executed: 0,
                    commands_failed: commands_count,
                });
            }
        };

        // Compare with on-chain effects using the new comparison method
        let comparison = self.effects.as_ref().map(|on_chain| {
            EffectsComparison::compare(
                on_chain,
                effects.success,
                effects.created.len(),
                effects.mutated.len(),
                effects.deleted.len(),
            )
        });

        Ok(ReplayResult {
            digest: self.digest.clone(),
            local_success: effects.success,
            local_error: effects.error,
            comparison,
            commands_executed: if effects.success { commands_count } else { 0 },
            commands_failed: if effects.success { 0 } else { commands_count },
        })
    }

    /// Check if this transaction uses only framework packages (0x1, 0x2, 0x3).
    pub fn uses_only_framework(&self) -> bool {
        let framework_addrs = [
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            "0x0000000000000000000000000000000000000000000000000000000000000003",
            "0x1",
            "0x2",
            "0x3",
        ];

        for cmd in &self.commands {
            if let PtbCommand::MoveCall { package, .. } = cmd {
                let is_framework = framework_addrs
                    .iter()
                    .any(|addr| package == *addr || package.to_lowercase() == addr.to_lowercase());
                if !is_framework {
                    return false;
                }
            }
        }
        true
    }

    /// Get a list of third-party packages used by this transaction.
    pub fn third_party_packages(&self) -> Vec<String> {
        let framework_addrs = [
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            "0x0000000000000000000000000000000000000000000000000000000000000003",
            "0x1",
            "0x2",
            "0x3",
        ];

        let mut packages = std::collections::BTreeSet::new();
        for cmd in &self.commands {
            if let PtbCommand::MoveCall { package, .. } = cmd {
                let is_framework = framework_addrs
                    .iter()
                    .any(|addr| package == *addr || package.to_lowercase() == addr.to_lowercase());
                if !is_framework {
                    packages.insert(package.clone());
                }
            }
        }
        packages.into_iter().collect()
    }

    /// Get a summary of this transaction for display.
    pub fn summary(&self) -> String {
        let status = self
            .effects
            .as_ref()
            .map(|e| match &e.status {
                TransactionStatus::Success => "success".to_string(),
                TransactionStatus::Failure { error } => format!("failed: {}", error),
            })
            .unwrap_or_else(|| "unknown".to_string());

        format!(
            "Transaction {} from {} ({} commands, status: {})",
            self.digest.0,
            self.sender.to_hex_literal(),
            self.commands.len(),
            status
        )
    }
}

/// Helper module for base64 serialization.
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use base64::Engine;
        let s = base64::engine::general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        use base64::Engine;
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
    fn test_transaction_digest() {
        let digest = TransactionDigest::new("abc123");
        assert_eq!(digest.0, "abc123");
    }

    /// Convert a PtbArgument to an Argument (test helper).
    fn convert_ptb_argument(arg: &PtbArgument) -> Argument {
        match arg {
            PtbArgument::Input { index } => Argument::Input(*index),
            PtbArgument::Result { index } => Argument::Result(*index),
            PtbArgument::NestedResult {
                index,
                result_index,
            } => Argument::NestedResult(*index, *result_index),
            PtbArgument::GasCoin => Argument::Input(0), // Gas coin is typically input 0
        }
    }

    #[test]
    fn test_ptb_argument_conversion() {
        let input = PtbArgument::Input { index: 5 };
        let arg = convert_ptb_argument(&input);
        assert!(matches!(arg, Argument::Input(5)));

        let result = PtbArgument::Result { index: 3 };
        let arg = convert_ptb_argument(&result);
        assert!(matches!(arg, Argument::Result(3)));

        let nested = PtbArgument::NestedResult {
            index: 2,
            result_index: 1,
        };
        let arg = convert_ptb_argument(&nested);
        assert!(matches!(arg, Argument::NestedResult(2, 1)));
    }

    #[test]
    fn test_transaction_status_serialization() {
        let success = TransactionStatus::Success;
        let json = serde_json::to_string(&success).unwrap();
        assert_eq!(json, "\"Success\"");

        let failure = TransactionStatus::Failure {
            error: "test error".to_string(),
        };
        let json = serde_json::to_string(&failure).unwrap();
        assert!(json.contains("test error"));
    }

    #[test]
    fn test_gas_summary_default() {
        let gas = GasSummary::default();
        assert_eq!(gas.computation_cost, 0);
        assert_eq!(gas.storage_cost, 0);
    }

    #[test]
    fn test_derive_dynamic_field_id() {
        // Test case from Cetus Pool's skip_list:
        // Parent UID: 0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3
        // Key type: u64
        // Key value: 481316
        // Expected object ID: 0x01aff7f7c58ba303e1d23df4ef9ccc1562d9bdcee7aeed813a3edb3a7f2b3704

        let parent = AccountAddress::from_hex_literal(
            "0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3",
        )
        .unwrap();

        let key: u64 = 481316;

        let derived = super::derive_dynamic_field_id_u64(parent, key).unwrap();

        let expected = AccountAddress::from_hex_literal(
            "0x01aff7f7c58ba303e1d23df4ef9ccc1562d9bdcee7aeed813a3edb3a7f2b3704",
        )
        .unwrap();

        assert_eq!(
            derived,
            expected,
            "Derived ID mismatch:\n  got:      {}\n  expected: {}",
            derived.to_hex_literal(),
            expected.to_hex_literal()
        );

        // Test another key value (key=0 for historical skip_list head)
        let key_0_derived = super::derive_dynamic_field_id_u64(parent, 0).unwrap();
        let key_0_expected = AccountAddress::from_hex_literal(
            "0x364f5bc3735b4aadfe4ff299163c407c8058ab7f014308ec62550a5306a1fb7f",
        )
        .unwrap();

        assert_eq!(
            key_0_derived,
            key_0_expected,
            "Derived ID for key=0 mismatch:\n  got:      {}\n  expected: {}",
            key_0_derived.to_hex_literal(),
            key_0_expected.to_hex_literal()
        );
    }
}

// ============================================================================
// GraphQL to FetchedTransaction Conversion
// ============================================================================

/// Convert a GraphQL transaction to the internal FetchedTransaction format.
///
/// This enables using transactions fetched via DataFetcher with the CachedTransaction
/// and replay infrastructure.
pub fn graphql_to_fetched_transaction(
    tx: &crate::graphql::GraphQLTransaction,
) -> Result<FetchedTransaction> {
    use crate::graphql::GraphQLTransactionInput;
    use base64::Engine;
    use move_core_types::account_address::AccountAddress;

    // Parse sender address
    let sender_hex = tx.sender.strip_prefix("0x").unwrap_or(&tx.sender);
    let sender = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))
        .map_err(|e| anyhow::anyhow!("Invalid sender address: {}", e))?;

    // Convert inputs
    let inputs: Vec<TransactionInput> = tx
        .inputs
        .iter()
        .map(|input| match input {
            GraphQLTransactionInput::Pure { bytes_base64 } => {
                // Decode base64 to bytes for Pure input
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(bytes_base64)
                    .unwrap_or_default();
                TransactionInput::Pure { bytes }
            }
            GraphQLTransactionInput::OwnedObject {
                address,
                version,
                digest,
            } => TransactionInput::Object {
                object_id: address.clone(),
                version: *version,
                digest: digest.clone(),
            },
            GraphQLTransactionInput::SharedObject {
                address,
                initial_shared_version,
                mutable,
            } => TransactionInput::SharedObject {
                object_id: address.clone(),
                initial_shared_version: *initial_shared_version,
                mutable: *mutable,
            },
            GraphQLTransactionInput::Receiving {
                address,
                version,
                digest,
            } => TransactionInput::Receiving {
                object_id: address.clone(),
                version: *version,
                digest: digest.clone(),
            },
        })
        .collect();

    // Convert commands
    let commands: Vec<PtbCommand> = tx
        .commands
        .iter()
        .filter_map(convert_graphql_command)
        .collect();

    // Convert effects
    let effects = tx.effects.as_ref().map(convert_graphql_effects);

    Ok(FetchedTransaction {
        digest: TransactionDigest(tx.digest.clone()),
        sender,
        gas_budget: tx.gas_budget.unwrap_or(0),
        gas_price: tx.gas_price.unwrap_or(0),
        commands,
        inputs,
        effects,
        timestamp_ms: tx.timestamp_ms,
        checkpoint: tx.checkpoint,
    })
}

/// Convert a GraphQL command to PtbCommand
fn convert_graphql_command(cmd: &crate::graphql::GraphQLCommand) -> Option<PtbCommand> {
    use crate::graphql::GraphQLCommand;

    match cmd {
        GraphQLCommand::MoveCall {
            package,
            module,
            function,
            type_arguments,
            arguments,
        } => Some(PtbCommand::MoveCall {
            package: package.clone(),
            module: module.clone(),
            function: function.clone(),
            type_arguments: type_arguments.clone(),
            arguments: arguments.iter().map(convert_graphql_argument).collect(),
        }),
        GraphQLCommand::TransferObjects { objects, address } => Some(PtbCommand::TransferObjects {
            objects: objects.iter().map(convert_graphql_argument).collect(),
            address: convert_graphql_argument(address),
        }),
        GraphQLCommand::SplitCoins { coin, amounts } => Some(PtbCommand::SplitCoins {
            coin: convert_graphql_argument(coin),
            amounts: amounts.iter().map(convert_graphql_argument).collect(),
        }),
        GraphQLCommand::MergeCoins {
            destination,
            sources,
        } => Some(PtbCommand::MergeCoins {
            destination: convert_graphql_argument(destination),
            sources: sources.iter().map(convert_graphql_argument).collect(),
        }),
        GraphQLCommand::MakeMoveVec { type_arg, elements } => Some(PtbCommand::MakeMoveVec {
            type_arg: type_arg.clone(),
            elements: elements.iter().map(convert_graphql_argument).collect(),
        }),
        GraphQLCommand::Publish {
            modules,
            dependencies,
        } => Some(PtbCommand::Publish {
            modules: modules.clone(),
            dependencies: dependencies.clone(),
        }),
        GraphQLCommand::Upgrade {
            package, ticket, ..
        } => Some(PtbCommand::Upgrade {
            modules: Vec::new(), // Upgrade modules not available in GraphQL response
            package: package.clone(),
            ticket: convert_graphql_argument(ticket),
        }),
        GraphQLCommand::Other { .. } => None, // Skip unknown command types
    }
}

/// Convert a GraphQL argument to PtbArgument
fn convert_graphql_argument(arg: &crate::graphql::GraphQLArgument) -> PtbArgument {
    use crate::graphql::GraphQLArgument;

    match arg {
        GraphQLArgument::GasCoin => PtbArgument::GasCoin,
        GraphQLArgument::Input(index) => PtbArgument::Input { index: *index },
        GraphQLArgument::Result(index) => PtbArgument::Result { index: *index },
        GraphQLArgument::NestedResult(index, result_idx) => PtbArgument::NestedResult {
            index: *index,
            result_index: *result_idx,
        },
    }
}

/// Convert GraphQL effects to TransactionEffectsSummary
fn convert_graphql_effects(effects: &crate::graphql::GraphQLEffects) -> TransactionEffectsSummary {
    let status = if effects.status == "SUCCESS" {
        TransactionStatus::Success
    } else {
        TransactionStatus::Failure {
            error: effects.status.clone(),
        }
    };

    TransactionEffectsSummary {
        status,
        created: effects.created.iter().map(|o| o.address.clone()).collect(),
        mutated: effects.mutated.iter().map(|o| o.address.clone()).collect(),
        deleted: effects.deleted.clone(),
        wrapped: Vec::new(),
        unwrapped: Vec::new(),
        gas_used: GasSummary::default(),
        events_count: 0,
        shared_object_versions: std::collections::HashMap::new(),
    }
}

// ============================================================================
// gRPC to FetchedTransaction Conversion
// ============================================================================

/// Convert a gRPC transaction to the internal FetchedTransaction format.
///
/// This enables using transactions fetched directly via gRPC (e.g., from Surflux)
/// with the CachedTransaction and replay infrastructure.
///
/// gRPC transactions provide additional historical data that GraphQL doesn't:
/// - `unchanged_loaded_runtime_objects`: Objects read but not modified (with exact versions)
/// - `unchanged_consensus_objects`: Actual consensus versions for shared objects
/// - `changed_objects`: Objects modified with their INPUT versions
pub fn grpc_to_fetched_transaction(
    tx: &crate::grpc::GrpcTransaction,
) -> Result<FetchedTransaction> {
    use crate::grpc::GrpcInput;
    use move_core_types::account_address::AccountAddress;

    // Parse sender address
    let sender_hex = tx.sender.strip_prefix("0x").unwrap_or(&tx.sender);
    let sender = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))
        .map_err(|e| anyhow::anyhow!("Invalid sender address: {}", e))?;

    // Convert inputs
    let inputs: Vec<TransactionInput> = tx
        .inputs
        .iter()
        .map(|input| match input {
            GrpcInput::Pure { bytes } => TransactionInput::Pure {
                bytes: bytes.clone(),
            },
            GrpcInput::Object {
                object_id,
                version,
                digest,
            } => TransactionInput::Object {
                object_id: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                mutable,
            } => TransactionInput::SharedObject {
                object_id: object_id.clone(),
                initial_shared_version: *initial_version,
                mutable: *mutable,
            },
            GrpcInput::Receiving {
                object_id,
                version,
                digest,
            } => TransactionInput::Receiving {
                object_id: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
        })
        .collect();

    // Convert commands
    let commands: Vec<PtbCommand> = tx
        .commands
        .iter()
        .filter_map(convert_grpc_command)
        .collect();

    // Convert effects status
    let effects = tx.status.as_ref().map(|status| {
        let tx_status = if status == "success" {
            TransactionStatus::Success
        } else {
            TransactionStatus::Failure {
                error: tx
                    .execution_error
                    .as_ref()
                    .and_then(|e| e.description.clone())
                    .unwrap_or_else(|| status.clone()),
            }
        };

        TransactionEffectsSummary {
            status: tx_status,
            created: tx
                .created_objects
                .iter()
                .map(|(id, _)| id.clone())
                .collect(),
            mutated: tx
                .changed_objects
                .iter()
                .map(|(id, _)| id.clone())
                .collect(),
            deleted: Vec::new(),
            wrapped: Vec::new(),
            unwrapped: Vec::new(),
            gas_used: GasSummary::default(),
            events_count: 0,
            shared_object_versions: std::collections::HashMap::new(),
        }
    });

    Ok(FetchedTransaction {
        digest: TransactionDigest(tx.digest.clone()),
        sender,
        gas_budget: tx.gas_budget.unwrap_or(0),
        gas_price: tx.gas_price.unwrap_or(0),
        commands,
        inputs,
        effects,
        timestamp_ms: tx.timestamp_ms,
        checkpoint: tx.checkpoint,
    })
}

/// Convert a gRPC command to PtbCommand
fn convert_grpc_command(cmd: &crate::grpc::GrpcCommand) -> Option<PtbCommand> {
    use crate::grpc::GrpcCommand;

    match cmd {
        GrpcCommand::MoveCall {
            package,
            module,
            function,
            type_arguments,
            arguments,
        } => Some(PtbCommand::MoveCall {
            package: package.clone(),
            module: module.clone(),
            function: function.clone(),
            type_arguments: type_arguments.clone(),
            arguments: arguments.iter().map(convert_grpc_argument).collect(),
        }),
        GrpcCommand::TransferObjects { objects, address } => Some(PtbCommand::TransferObjects {
            objects: objects.iter().map(convert_grpc_argument).collect(),
            address: convert_grpc_argument(address),
        }),
        GrpcCommand::SplitCoins { coin, amounts } => Some(PtbCommand::SplitCoins {
            coin: convert_grpc_argument(coin),
            amounts: amounts.iter().map(convert_grpc_argument).collect(),
        }),
        GrpcCommand::MergeCoins { coin, sources } => Some(PtbCommand::MergeCoins {
            destination: convert_grpc_argument(coin),
            sources: sources.iter().map(convert_grpc_argument).collect(),
        }),
        GrpcCommand::MakeMoveVec {
            element_type,
            elements,
        } => Some(PtbCommand::MakeMoveVec {
            type_arg: element_type.clone(),
            elements: elements.iter().map(convert_grpc_argument).collect(),
        }),
        GrpcCommand::Publish {
            modules,
            dependencies,
        } => Some(PtbCommand::Publish {
            modules: modules
                .iter()
                .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                .collect(),
            dependencies: dependencies.clone(),
        }),
        GrpcCommand::Upgrade {
            modules,
            package,
            ticket,
            ..
        } => Some(PtbCommand::Upgrade {
            modules: modules
                .iter()
                .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                .collect(),
            package: package.clone(),
            ticket: convert_grpc_argument(ticket),
        }),
    }
}

/// Convert a gRPC argument to PtbArgument
fn convert_grpc_argument(arg: &crate::grpc::GrpcArgument) -> PtbArgument {
    use crate::grpc::GrpcArgument;

    match arg {
        GrpcArgument::GasCoin => PtbArgument::GasCoin,
        GrpcArgument::Input(index) => PtbArgument::Input {
            index: *index as u16,
        },
        GrpcArgument::Result(index) => PtbArgument::Result {
            index: *index as u16,
        },
        GrpcArgument::NestedResult(index, result_idx) => PtbArgument::NestedResult {
            index: *index as u16,
            result_index: *result_idx as u16,
        },
    }
}

// ============================================================================
// Auto-Fetch and Cache Functionality
// ============================================================================

/// Extract package addresses that a module depends on from its bytecode.
///
/// This parses the CompiledModule to find all module_handles, which reference
/// other modules that this module depends on.
fn extract_dependencies_from_bytecode(bytecode: &[u8]) -> Vec<AccountAddress> {
    use move_binary_format::CompiledModule;
    use std::collections::BTreeSet;

    // Framework addresses to skip
    let framework_addrs: BTreeSet<AccountAddress> = [
        AccountAddress::from_hex_literal("0x1").unwrap(),
        AccountAddress::from_hex_literal("0x2").unwrap(),
        AccountAddress::from_hex_literal("0x3").unwrap(),
    ]
    .into_iter()
    .collect();

    let mut deps = Vec::new();

    if let Ok(module) = CompiledModule::deserialize_with_defaults(bytecode) {
        for handle in &module.module_handles {
            let addr = *module.address_identifier_at(handle.address);
            // Skip framework modules
            if !framework_addrs.contains(&addr) {
                deps.push(addr);
            }
        }
    }

    deps
}

/// Extract all unique dependency addresses from a set of packages.
/// packages is HashMap<String, Vec<(module_name, bytecode_base64)>>
fn extract_all_dependencies(
    packages: &std::collections::HashMap<String, Vec<(String, String)>>,
) -> std::collections::BTreeSet<String> {
    use base64::Engine;
    use std::collections::BTreeSet;

    let mut all_deps: BTreeSet<String> = BTreeSet::new();

    for modules in packages.values() {
        for (_name, bytecode_base64) in modules {
            if let Ok(bytecode) = base64::engine::general_purpose::STANDARD.decode(bytecode_base64)
            {
                for dep_addr in extract_dependencies_from_bytecode(&bytecode) {
                    let addr_str = format!("0x{}", hex::encode(dep_addr.as_ref()));
                    all_deps.insert(addr_str);
                }
            }
        }
    }

    all_deps
}

/// Fetch a transaction and all its dependencies, returning a fully populated CachedTransaction.
///
/// This function automatically:
/// 1. Fetches the transaction from GraphQL
/// 2. Fetches all referenced packages
/// 3. **Recursively fetches transitive package dependencies** (up to max_depth)
/// 4. Fetches all input objects
/// 5. Optionally fetches historical object versions via gRPC
/// 6. Optionally fetches dynamic field children
///
/// # Arguments
/// * `fetcher` - DataFetcher configured for mainnet
/// * `digest` - Transaction digest to fetch
/// * `fetch_historical` - Whether to fetch historical object versions (requires gRPC)
/// * `fetch_dynamic_fields` - Whether to fetch dynamic field children
pub fn fetch_and_cache_transaction(
    fetcher: &crate::data_fetcher::DataFetcher,
    digest: &str,
    _fetch_historical: bool,
    fetch_dynamic_fields: bool,
) -> Result<CachedTransaction> {
    use crate::graphql::GraphQLTransactionInput;
    use base64::Engine;
    use std::collections::BTreeSet;

    // Maximum depth for transitive dependency resolution
    const MAX_DEPENDENCY_DEPTH: usize = 10;

    // Step 1: Fetch transaction
    eprintln!("[fetch_and_cache] Fetching transaction {}...", digest);
    let graphql_tx = fetcher.fetch_transaction(digest)?;
    let fetched_tx = graphql_to_fetched_transaction(&graphql_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    // Step 2: Extract and fetch all directly referenced packages
    let package_ids = crate::data_fetcher::DataFetcher::extract_package_ids(&graphql_tx);
    eprintln!(
        "[fetch_and_cache] Found {} directly referenced packages",
        package_ids.len()
    );

    let mut fetched_packages: BTreeSet<String> = BTreeSet::new();
    let mut packages_to_fetch: BTreeSet<String> = package_ids.into_iter().collect();

    // Step 3: Recursively fetch transitive dependencies
    for depth in 0..MAX_DEPENDENCY_DEPTH {
        if packages_to_fetch.is_empty() {
            eprintln!(
                "[fetch_and_cache] All dependencies resolved at depth {}",
                depth
            );
            break;
        }

        eprintln!(
            "[fetch_and_cache] Depth {}: fetching {} packages...",
            depth,
            packages_to_fetch.len()
        );

        let mut newly_fetched: Vec<String> = Vec::new();

        for pkg_id in &packages_to_fetch {
            if fetched_packages.contains(pkg_id) {
                continue;
            }

            match fetcher.fetch_package(pkg_id) {
                Ok(pkg) => {
                    let modules: Vec<(String, Vec<u8>)> = pkg
                        .modules
                        .into_iter()
                        .map(|m| (m.name, m.bytecode))
                        .collect();

                    eprintln!(
                        "[fetch_and_cache]   Fetched {}: {} modules",
                        &pkg_id[..20.min(pkg_id.len())],
                        modules.len()
                    );

                    cached.add_package(pkg_id.clone(), modules);
                    newly_fetched.push(pkg_id.clone());
                    fetched_packages.insert(pkg_id.clone());
                }
                Err(e) => {
                    eprintln!(
                        "[fetch_and_cache]   Warning: Failed to fetch {}: {}",
                        &pkg_id[..20.min(pkg_id.len())],
                        e
                    );
                    // Mark as "fetched" to avoid infinite retry
                    fetched_packages.insert(pkg_id.clone());
                }
            }
        }

        // Extract dependencies from newly fetched packages
        let all_deps = extract_all_dependencies(&cached.packages);

        // Find packages we haven't fetched yet
        packages_to_fetch = all_deps
            .into_iter()
            .filter(|p| !fetched_packages.contains(p))
            .collect();

        if packages_to_fetch.is_empty() {
            eprintln!("[fetch_and_cache] No more transitive dependencies to fetch");
            break;
        }

        eprintln!(
            "[fetch_and_cache] Found {} new transitive dependencies",
            packages_to_fetch.len()
        );
    }

    eprintln!(
        "[fetch_and_cache] Total packages fetched: {}",
        cached.packages.len()
    );

    // Build a map of object address -> input version from effects
    // For mutated objects: input_version = output_version - 1
    let mut input_versions: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();
    if let Some(effects) = &graphql_tx.effects {
        for change in &effects.mutated {
            if let Some(output_version) = change.version {
                // Input version is output version minus 1
                let input_version = output_version.saturating_sub(1);
                input_versions.insert(change.address.clone(), input_version);
                eprintln!(
                    "[fetch_and_cache] Object {} mutated: output_version={}, input_version={}",
                    &change.address[..20.min(change.address.len())],
                    output_version,
                    input_version
                );
            }
        }
    }

    // Step 4: Fetch input objects
    for input in &graphql_tx.inputs {
        match input {
            GraphQLTransactionInput::OwnedObject {
                address, version, ..
            } => match fetcher.fetch_object_at_version(address, *version) {
                Ok(obj) => {
                    if let Some(bcs) = obj.bcs_bytes {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&bcs);
                        cached.objects.insert(address.clone(), encoded);
                        cached.object_versions.insert(address.clone(), *version);
                        if let Some(type_str) = obj.type_string {
                            cached.object_types.insert(address.clone(), type_str);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Failed to fetch object {}: {}", address, e);
                }
            },
            GraphQLTransactionInput::SharedObject {
                address,
                initial_shared_version,
                ..
            } => {
                // For shared objects, try to use the computed input version from effects
                // This is more accurate than initial_shared_version which is when the object was first shared
                let version_to_fetch = input_versions.get(address).copied();

                let fetch_result = if let Some(version) = version_to_fetch {
                    eprintln!("[fetch_and_cache] Fetching shared object {} at input version {} (initial_shared_version={})",
                        &address[..20.min(address.len())], version, initial_shared_version);
                    fetcher.fetch_object_at_version(address, version)
                } else {
                    // No version info from effects (read-only object not in mutations)
                    eprintln!("[fetch_and_cache] Fetching shared object {} at initial_shared_version={} (no mutation)",
                        &address[..20.min(address.len())], initial_shared_version);
                    fetcher.fetch_object_at_version(address, *initial_shared_version)
                };

                match fetch_result {
                    Ok(obj) => {
                        if let Some(bcs) = obj.bcs_bytes {
                            let encoded = base64::engine::general_purpose::STANDARD.encode(&bcs);
                            cached.objects.insert(address.clone(), encoded);
                            cached.object_versions.insert(address.clone(), obj.version);
                            if let Some(type_str) = obj.type_string {
                                cached.object_types.insert(address.clone(), type_str);
                            }
                            eprintln!("[fetch_and_cache]   SUCCESS: got version {}", obj.version);
                        }
                    }
                    Err(e) => {
                        // Historical version not available - fall back to current version
                        // Note: This may cause replay differences for objects that changed since the tx
                        eprintln!(
                            "[fetch_and_cache] WARNING: Historical version unavailable for {}: {}",
                            &address[..20.min(address.len())],
                            e
                        );
                        eprintln!("[fetch_and_cache]   Falling back to CURRENT version (may cause replay differences)");

                        if let Ok(obj) = fetcher.fetch_object(address) {
                            if let Some(bcs) = obj.bcs_bytes {
                                let encoded =
                                    base64::engine::general_purpose::STANDARD.encode(&bcs);
                                cached.objects.insert(address.clone(), encoded);
                                cached.object_versions.insert(address.clone(), obj.version);
                                if let Some(type_str) = obj.type_string {
                                    cached.object_types.insert(address.clone(), type_str);
                                }
                                eprintln!("[fetch_and_cache]   Fallback SUCCESS: got version {} (wanted {})",
                                    obj.version, version_to_fetch.unwrap_or(*initial_shared_version));
                            }
                        } else {
                            eprintln!("[fetch_and_cache]   ERROR: Could not fetch object at all");
                        }
                    }
                }
            }
            // Receiving and Pure inputs don't need special fetching
            _ => {}
        }
    }

    // Step 5: Fetch dynamic field children if requested
    if fetch_dynamic_fields {
        // Fetch dynamic fields for all shared objects (they often contain important state)
        for input in &graphql_tx.inputs {
            if let GraphQLTransactionInput::SharedObject { address, .. } = input {
                match fetcher.fetch_dynamic_fields_recursive(address, 2, 100) {
                    Ok(children) => {
                        for child in children {
                            if let (Some(_name_bcs), Some(value_bcs)) =
                                (child.name_bcs, child.value_bcs)
                            {
                                let child_field = CachedDynamicField {
                                    parent_id: child.parent_address.clone(),
                                    type_string: child.value_type.unwrap_or_default(),
                                    bcs_base64: base64::engine::general_purpose::STANDARD
                                        .encode(&value_bcs),
                                    version: child.version.unwrap_or(0),
                                };
                                cached
                                    .dynamic_field_children
                                    .insert(child.child_address, child_field);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to fetch dynamic fields for {}: {}",
                            address, e
                        );
                    }
                }
            }
        }
    }

    Ok(cached)
}

/// Load a cached transaction from disk, or fetch and cache it if not present.
///
/// This is the main entry point for auto-caching behavior.
///
/// # Arguments
/// * `cache_dir` - Directory to store/load cached transactions
/// * `digest` - Transaction digest
/// * `fetcher` - Optional DataFetcher (created if None and fetch needed)
/// * `fetch_historical` - Whether to fetch historical versions
/// * `fetch_dynamic_fields` - Whether to fetch dynamic fields
pub fn load_or_fetch_transaction(
    cache_dir: &str,
    digest: &str,
    fetcher: Option<&crate::data_fetcher::DataFetcher>,
    fetch_historical: bool,
    fetch_dynamic_fields: bool,
) -> Result<CachedTransaction> {
    let cache_path = std::path::Path::new(cache_dir).join(format!("{}.json", digest));

    // Try to load from cache first
    if cache_path.exists() {
        let data = std::fs::read_to_string(&cache_path)?;
        let cached: CachedTransaction = serde_json::from_str(&data)?;
        return Ok(cached);
    }

    // Create cache directory if needed
    std::fs::create_dir_all(cache_dir)?;

    // Fetch the transaction - create a new fetcher if none provided
    let owned_fetcher;
    let fetcher_ref = match fetcher {
        Some(f) => f,
        None => {
            owned_fetcher = crate::data_fetcher::DataFetcher::mainnet();
            &owned_fetcher
        }
    };

    let cached =
        fetch_and_cache_transaction(fetcher_ref, digest, fetch_historical, fetch_dynamic_fields)?;

    // Save to cache
    let json = serde_json::to_string_pretty(&cached)?;
    std::fs::write(&cache_path, json)?;

    Ok(cached)
}
