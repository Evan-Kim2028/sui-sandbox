//! Transaction Replay Module
//!
//! This module provides functionality to fetch and replay mainnet Sui transactions
//! in the local Move VM sandbox. This enables:
//!
//! 1. **Validation**: Compare local execution with on-chain effects
//! 2. **Training Data**: Generate input/output pairs for LLM training
//! 3. **Testing**: Use real transaction patterns for testing
//!
//! ## Architecture
//!
//! ```text
//! Mainnet RPC → FetchedTransaction → PTBCommands → LocalExecution → CompareEffects
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! let fetcher = TransactionFetcher::new("https://fullnode.mainnet.sui.io:443")?;
//! let tx = fetcher.fetch_transaction("digest_here").await?;
//! let replay_result = tx.replay(&mut harness)?;
//! ```

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::benchmark::ptb::{Argument, Command, InputValue, ObjectInput};
use crate::benchmark::vm::VMHarness;
use crate::grpc::{GrpcClient, GrpcOwner};

// Re-export type parsing functions from the canonical location (types module)
// This maintains backwards compatibility while centralizing the implementation.
pub use crate::benchmark::types::{
    clear_type_cache as clear_type_tag_cache, parse_type_tag,
    type_cache_size as type_tag_cache_size,
};

// =============================================================================
// RPC Helper Functions
// =============================================================================

/// Execute a JSON-RPC request to a Sui endpoint.
///
/// This is the canonical way to make RPC calls throughout the codebase.
/// Handles request formatting, error checking, and result extraction.
///
/// # Arguments
/// * `endpoint` - The RPC endpoint URL
/// * `method` - The JSON-RPC method name (e.g., "sui_getObject")
/// * `params` - The method parameters as a JSON value
///
/// # Returns
/// The "result" field from the RPC response, or an error if the request failed.
fn rpc_request(
    endpoint: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let request_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });

    let response: serde_json::Value = ureq::post(endpoint)
        .set("Content-Type", "application/json")
        .send_json(&request_body)
        .map_err(|e| anyhow!("RPC request failed: {}", e))?
        .into_json()
        .map_err(|e| anyhow!("Failed to parse RPC response: {}", e))?;

    // Check for RPC error
    if let Some(error) = response.get("error") {
        return Err(anyhow!(
            "RPC error: {}",
            error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown")
        ));
    }

    response
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("No result in RPC response"))
}

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

/// Fetches transactions from a Sui RPC endpoint.
pub struct TransactionFetcher {
    /// RPC endpoint URL
    endpoint: String,
    /// Archive endpoint for historical object lookups via JSON-RPC (optional, legacy)
    archive_endpoint: Option<String>,
    /// gRPC archive endpoint URL for historical lookups (preferred)
    grpc_archive_endpoint: Option<String>,
    /// Lazily initialized gRPC client for archive lookups
    grpc_client: OnceLock<GrpcClient>,
    /// Tokio runtime for executing async gRPC calls in sync context
    runtime: OnceLock<tokio::runtime::Runtime>,
}

impl TransactionFetcher {
    /// Create a new fetcher with the given RPC endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            archive_endpoint: None,
            grpc_archive_endpoint: None,
            grpc_client: OnceLock::new(),
            runtime: OnceLock::new(),
        }
    }

    /// Create a fetcher for Sui mainnet.
    pub fn mainnet() -> Self {
        Self::new("https://fullnode.mainnet.sui.io:443")
    }

    /// Create a fetcher for Sui mainnet with gRPC archive support.
    ///
    /// Uses `archive.mainnet.sui.io:443` for historical object lookups via gRPC.
    /// This is the recommended way to fetch historical objects as the JSON-RPC
    /// archive endpoints are unreliable.
    pub fn mainnet_with_archive() -> Self {
        Self {
            endpoint: "https://fullnode.mainnet.sui.io:443".to_string(),
            archive_endpoint: None,
            grpc_archive_endpoint: Some("https://archive.mainnet.sui.io:443".to_string()),
            grpc_client: OnceLock::new(),
            runtime: OnceLock::new(),
        }
    }

    /// Create a fetcher for Sui testnet.
    pub fn testnet() -> Self {
        Self::new("https://fullnode.testnet.sui.io:443")
    }

    /// Set the JSON-RPC archive endpoint for historical lookups (legacy).
    /// Prefer `with_grpc_archive_endpoint` for more reliable historical lookups.
    pub fn with_archive_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.archive_endpoint = Some(endpoint.into());
        self
    }

    /// Set the gRPC archive endpoint for historical lookups (recommended).
    ///
    /// The gRPC archive at `archive.mainnet.sui.io:443` provides reliable
    /// historical object lookups via the LedgerService.
    pub fn with_grpc_archive_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.grpc_archive_endpoint = Some(endpoint.into());
        self
    }

    /// Get or initialize the tokio runtime for async operations.
    fn get_runtime(&self) -> Result<&tokio::runtime::Runtime> {
        self.runtime.get_or_init(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
        });
        self.runtime
            .get()
            .ok_or_else(|| anyhow!("Runtime not initialized"))
    }

    /// Get or initialize the gRPC client for archive lookups.
    fn get_grpc_client(&self) -> Result<&GrpcClient> {
        let endpoint = self
            .grpc_archive_endpoint
            .as_ref()
            .ok_or_else(|| anyhow!("No gRPC archive endpoint configured"))?;

        // Ensure runtime is initialized first
        let runtime = self.get_runtime()?;

        // Initialize client if needed
        if self.grpc_client.get().is_none() {
            let client = runtime.block_on(async { GrpcClient::new(endpoint).await })?;
            // Ignore if another thread beat us to it
            let _ = self.grpc_client.set(client);
        }

        self.grpc_client
            .get()
            .ok_or_else(|| anyhow!("gRPC client not initialized"))
    }

    /// Get the endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Fetch a transaction by digest.
    ///
    /// This is an async operation that makes an HTTP request to the RPC endpoint.
    /// For now, returns a placeholder; actual implementation requires async runtime.
    pub fn fetch_transaction_sync(&self, digest: &str) -> Result<FetchedTransaction> {
        let result = rpc_request(
            &self.endpoint,
            "sui_getTransactionBlock",
            serde_json::json!([
                digest,
                {
                    "showInput": true,
                    "showEffects": true,
                    "showEvents": false,
                    "showObjectChanges": true,
                    "showBalanceChanges": false
                }
            ]),
        )?;

        parse_transaction_response(digest, &result)
    }

    /// Fetch recent transactions from a checkpoint.
    pub fn fetch_recent_transactions(&self, limit: usize) -> Result<Vec<TransactionDigest>> {
        // Get the latest checkpoint
        let checkpoint_result = rpc_request(
            &self.endpoint,
            "sui_getLatestCheckpointSequenceNumber",
            serde_json::json!([]),
        )?;

        let checkpoint = checkpoint_result
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| anyhow!("Failed to get latest checkpoint"))?;

        // Get transactions from recent checkpoints
        let mut digests = Vec::new();
        let mut current_checkpoint = checkpoint;

        while digests.len() < limit && current_checkpoint > 0 {
            let result = rpc_request(
                &self.endpoint,
                "sui_getCheckpoint",
                serde_json::json!([current_checkpoint.to_string()]),
            )?;

            if let Some(txs) = result.get("transactions").and_then(|t| t.as_array()) {
                for tx in txs {
                    if let Some(digest) = tx.as_str() {
                        digests.push(TransactionDigest::new(digest));
                        if digests.len() >= limit {
                            break;
                        }
                    }
                }
            }

            current_checkpoint = current_checkpoint.saturating_sub(1);
        }

        Ok(digests)
    }

    /// Fetch an object's BCS data by ID.
    /// Returns the raw BCS bytes of the object content.
    pub fn fetch_object_bcs(&self, object_id: &str) -> Result<Vec<u8>> {
        let fetched = self.fetch_object_full(object_id)?;
        Ok(fetched.bcs_bytes)
    }

    /// Fetch full object data including type, ownership, and BCS bytes.
    pub fn fetch_object_full(&self, object_id: &str) -> Result<FetchedObject> {
        let result = rpc_request(
            &self.endpoint,
            "sui_getObject",
            serde_json::json!([
                object_id,
                {
                    "showType": true,
                    "showOwner": true,
                    "showContent": true,
                    "showBcs": true
                }
            ]),
        )?;

        // Check if object exists
        if result.get("error").is_some() {
            return Err(anyhow!("ObjectNotFound: {}", object_id));
        }

        let data = result
            .get("data")
            .ok_or_else(|| anyhow!("No data in object response"))?;

        // Get BCS bytes (base64 encoded)
        let bcs_base64 = data
            .get("bcs")
            .and_then(|b| b.get("bcsBytes"))
            .and_then(|b| b.as_str())
            .ok_or_else(|| anyhow!("No BCS data in object"))?;

        use base64::Engine;
        let bcs_bytes = base64::engine::general_purpose::STANDARD
            .decode(bcs_base64)
            .map_err(|e| anyhow!("Failed to decode BCS: {}", e))?;

        // Get type string
        let type_string = data
            .get("type")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());

        // Get ownership info
        let owner = data.get("owner");
        let is_shared = owner.and_then(|o| o.get("Shared")).is_some();
        let is_immutable = owner
            .and_then(|o| o.as_str())
            .map(|s| s == "Immutable")
            .unwrap_or(false);

        // Get version
        let version = data
            .get("version")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1);

        Ok(FetchedObject {
            bcs_bytes,
            type_string,
            is_shared,
            is_immutable,
            version,
        })
    }

    /// Fetch object data at a specific version (for historical replay).
    /// Uses the archive endpoint if available, otherwise falls back to regular endpoint.
    pub fn fetch_object_at_version(&self, object_id: &str, version: u64) -> Result<Vec<u8>> {
        let fetched = self.fetch_object_at_version_full(object_id, version)?;
        Ok(fetched.bcs_bytes)
    }

    /// Fetch full object data at a specific version (for historical replay).
    ///
    /// Tries methods in order of reliability:
    /// 1. gRPC archive (most reliable for historical data)
    /// 2. JSON-RPC archive endpoint (legacy)
    /// 3. Regular JSON-RPC endpoint (fallback)
    pub fn fetch_object_at_version_full(
        &self,
        object_id: &str,
        version: u64,
    ) -> Result<FetchedObject> {
        // Try gRPC archive first (most reliable)
        if self.grpc_archive_endpoint.is_some() {
            match self.fetch_object_at_version_grpc(object_id, Some(version)) {
                Ok(obj) => return Ok(obj),
                Err(e) => {
                    // Log but continue to fallbacks
                    eprintln!(
                        "gRPC archive lookup failed for {}@{}: {}",
                        object_id, version, e
                    );
                }
            }
        }

        // Try JSON-RPC archive endpoint if available
        let endpoint = self.archive_endpoint.as_ref().unwrap_or(&self.endpoint);

        let result = rpc_request(
            endpoint,
            "sui_tryGetPastObject",
            serde_json::json!([
                object_id,
                version,
                {
                    "showType": true,
                    "showOwner": true,
                    "showContent": true,
                    "showBcs": true
                }
            ]),
        );

        // If archive failed and we have a fallback, try regular endpoint
        let result = match result {
            Ok(r) => r,
            Err(e) if self.archive_endpoint.is_some() => {
                // Try regular endpoint as fallback
                rpc_request(
                    &self.endpoint,
                    "sui_tryGetPastObject",
                    serde_json::json!([
                        object_id,
                        version,
                        {
                            "showType": true,
                            "showOwner": true,
                            "showContent": true,
                            "showBcs": true
                        }
                    ]),
                )
                .map_err(|_| e)?
            }
            Err(e) => return Err(e),
        };

        // Check status
        let status = result
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");

        if status != "VersionFound" {
            return Err(anyhow!("Object version not found: status={}", status));
        }

        let details = result
            .get("details")
            .ok_or_else(|| anyhow!("No details in past object response"))?;

        // Get BCS bytes
        let bcs_base64 = details
            .get("bcs")
            .and_then(|b| b.get("bcsBytes"))
            .and_then(|b| b.as_str())
            .ok_or_else(|| anyhow!("No BCS data in past object"))?;

        use base64::Engine;
        let bcs_bytes = base64::engine::general_purpose::STANDARD
            .decode(bcs_base64)
            .map_err(|e| anyhow!("Failed to decode BCS: {}", e))?;

        // Get type string
        let type_string = details
            .get("type")
            .or_else(|| details.get("bcs").and_then(|b| b.get("type")))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());

        // Get ownership info
        let owner = details.get("owner");
        let is_shared = owner.and_then(|o| o.get("Shared")).is_some();
        let is_immutable = owner
            .and_then(|o| o.as_str())
            .map(|s| s == "Immutable")
            .unwrap_or(false);

        Ok(FetchedObject {
            bcs_bytes,
            type_string,
            is_shared,
            is_immutable,
            version,
        })
    }

    /// Fetch object at version using gRPC archive.
    ///
    /// This is the most reliable method for historical object lookups as
    /// the archive at `archive.mainnet.sui.io:443` has full history.
    fn fetch_object_at_version_grpc(
        &self,
        object_id: &str,
        version: Option<u64>,
    ) -> Result<FetchedObject> {
        let client = self.get_grpc_client()?;
        let runtime = self.get_runtime()?;

        let grpc_obj: Option<crate::grpc::GrpcObject> =
            runtime.block_on(async { client.get_object_at_version(object_id, version).await })?;

        let obj = grpc_obj.ok_or_else(|| anyhow!("Object not found via gRPC: {}", object_id))?;

        // Extract BCS bytes
        let bcs_bytes = obj
            .bcs
            .ok_or_else(|| anyhow!("No BCS data in gRPC response for {}", object_id))?;

        // Determine ownership
        let (is_shared, is_immutable) = match &obj.owner {
            GrpcOwner::Shared { .. } => (true, false),
            GrpcOwner::Immutable => (false, true),
            _ => (false, false),
        };

        Ok(FetchedObject {
            bcs_bytes,
            type_string: obj.type_string,
            is_shared,
            is_immutable,
            version: obj.version,
        })
    }

    /// Fetch all input objects for a transaction.
    /// Returns a map from object ID to BCS bytes.
    pub fn fetch_transaction_inputs(
        &self,
        tx: &FetchedTransaction,
    ) -> Result<std::collections::HashMap<String, Vec<u8>>> {
        let mut objects = std::collections::HashMap::new();

        for input in &tx.inputs {
            match input {
                TransactionInput::Object {
                    object_id, version, ..
                } => {
                    // Try to fetch at specific version first, fall back to current
                    let bytes = if *version > 0 {
                        self.fetch_object_at_version(object_id, *version)
                            .or_else(|_| self.fetch_object_bcs(object_id))
                    } else {
                        self.fetch_object_bcs(object_id)
                    };

                    match bytes {
                        Ok(b) => {
                            objects.insert(object_id.clone(), b);
                        }
                        Err(e) => {
                            eprintln!("Warning: Failed to fetch object {}: {}", object_id, e);
                        }
                    }
                }
                TransactionInput::SharedObject {
                    object_id,
                    initial_shared_version,
                    ..
                } => {
                    // For shared objects, try to get at initial version or current
                    let bytes = self
                        .fetch_object_at_version(object_id, *initial_shared_version)
                        .or_else(|_| self.fetch_object_bcs(object_id));

                    match bytes {
                        Ok(b) => {
                            objects.insert(object_id.clone(), b);
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: Failed to fetch shared object {}: {}",
                                object_id, e
                            );
                        }
                    }
                }
                TransactionInput::ImmutableObject {
                    object_id, version, ..
                } => {
                    let bytes = if *version > 0 {
                        self.fetch_object_at_version(object_id, *version)
                            .or_else(|_| self.fetch_object_bcs(object_id))
                    } else {
                        self.fetch_object_bcs(object_id)
                    };

                    match bytes {
                        Ok(b) => {
                            objects.insert(object_id.clone(), b);
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: Failed to fetch immutable object {}: {}",
                                object_id, e
                            );
                        }
                    }
                }
                TransactionInput::Receiving {
                    object_id, version, ..
                } => {
                    let bytes = if *version > 0 {
                        self.fetch_object_at_version(object_id, *version)
                            .or_else(|_| self.fetch_object_bcs(object_id))
                    } else {
                        self.fetch_object_bcs(object_id)
                    };

                    match bytes {
                        Ok(b) => {
                            objects.insert(object_id.clone(), b);
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: Failed to fetch receiving object {}: {}",
                                object_id, e
                            );
                        }
                    }
                }
                TransactionInput::Pure { .. } => {
                    // Pure values don't need fetching
                }
            }
        }

        Ok(objects)
    }

    /// Fetch all input objects for a transaction with type information.
    /// Returns a map from object ID to (bytes, type_string).
    pub fn fetch_transaction_inputs_with_types(
        &self,
        tx: &FetchedTransaction,
    ) -> Result<std::collections::HashMap<String, (Vec<u8>, Option<String>)>> {
        let mut objects = std::collections::HashMap::new();

        for input in &tx.inputs {
            match input {
                TransactionInput::Object { object_id, .. }
                | TransactionInput::SharedObject { object_id, .. }
                | TransactionInput::ImmutableObject { object_id, .. }
                | TransactionInput::Receiving { object_id, .. } => {
                    // Use full fetch to get both bytes and type
                    match self.fetch_object_full(object_id) {
                        Ok(fetched) => {
                            objects.insert(
                                object_id.clone(),
                                (fetched.bcs_bytes, fetched.type_string),
                            );
                        }
                        Err(e) => {
                            eprintln!("Warning: Failed to fetch object {}: {}", object_id, e);
                        }
                    }
                }
                TransactionInput::Pure { .. } => {
                    // Pure values don't need fetching
                }
            }
        }

        Ok(objects)
    }

    /// Fetch package bytecode modules from the RPC.
    /// Returns a vector of (module_name, module_bytes) pairs.
    pub fn fetch_package_modules(&self, package_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let result = rpc_request(
            &self.endpoint,
            "sui_getObject",
            serde_json::json!([
                package_id,
                {
                    "showBcs": true,
                    "showContent": true
                }
            ]),
        )?;

        let data = result
            .get("data")
            .ok_or_else(|| anyhow!("No data in object response"))?;

        // Verify this is a package - check both "type" and "content.dataType"
        let obj_type = data
            .get("type")
            .and_then(|t| t.as_str())
            .or_else(|| {
                data.get("content")
                    .and_then(|c| c.get("dataType"))
                    .and_then(|t| t.as_str())
            })
            .unwrap_or("");

        if obj_type != "package" {
            return Err(anyhow!(
                "Object {} is not a package (type: {})",
                package_id,
                obj_type
            ));
        }

        // Get BCS module map - this is the primary source for bytecode
        let bcs_data = data.get("bcs");

        let mut modules = Vec::new();

        // Method 1: Try to get from BCS moduleMap (top-level)
        if let Some(bcs) = bcs_data {
            if let Some(module_map) = bcs.get("moduleMap").and_then(|m| m.as_object()) {
                use base64::Engine;
                for (name, bytes_b64) in module_map {
                    if let Some(b64_str) = bytes_b64.as_str() {
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64_str)
                        {
                            modules.push((name.clone(), bytes));
                        }
                    }
                }
            }
        }

        // Method 2: If no BCS modules found, try content.disassembled for module names
        // (informational only - we won't have executable bytecode)
        if modules.is_empty() {
            if let Some(content) = data.get("content") {
                if let Some(disasm) = content.get("disassembled").and_then(|d| d.as_object()) {
                    for name in disasm.keys() {
                        // Return empty bytes to indicate we know the module exists but don't have bytecode
                        modules.push((name.clone(), vec![]));
                    }
                }
            }
        }

        if modules.is_empty() {
            return Err(anyhow!("No modules found in package {}", package_id));
        }

        Ok(modules)
    }

    /// Extract all unique package IDs from a transaction's commands.
    pub fn extract_package_ids(tx: &FetchedTransaction) -> Vec<String> {
        let mut packages = std::collections::BTreeSet::new();

        for cmd in &tx.commands {
            if let PtbCommand::MoveCall { package, .. } = cmd {
                packages.insert(package.clone());
            }
        }

        packages.into_iter().collect()
    }

    /// Fetch all packages used by a transaction.
    /// Returns a map from package ID to list of (module_name, module_bytes).
    pub fn fetch_transaction_packages(
        &self,
        tx: &FetchedTransaction,
    ) -> Result<std::collections::HashMap<String, Vec<(String, Vec<u8>)>>> {
        let package_ids = Self::extract_package_ids(tx);
        let mut packages = std::collections::HashMap::new();

        // Skip framework packages (0x1, 0x2, 0x3) - we have those bundled
        let framework_prefixes = [
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            "0x0000000000000000000000000000000000000000000000000000000000000003",
            "0x1",
            "0x2",
            "0x3",
        ];

        for pkg_id in package_ids {
            // Check if it's a framework package
            let is_framework = framework_prefixes
                .iter()
                .any(|prefix| pkg_id == *prefix || pkg_id.to_lowercase() == prefix.to_lowercase());

            if is_framework {
                continue;
            }

            match self.fetch_package_modules(&pkg_id) {
                Ok(modules) => {
                    packages.insert(pkg_id, modules);
                }
                Err(e) => {
                    eprintln!("Warning: Failed to fetch package {}: {}", pkg_id, e);
                }
            }
        }

        Ok(packages)
    }

    /// Fetch dynamic field children of an object (or wrapped UID) via JSON-RPC.
    ///
    /// This uses the `suix_getDynamicFields` RPC method which works for both:
    /// - Top-level objects (like Tables)
    /// - Wrapped UIDs (like skip_list nodes inside a Pool)
    ///
    /// Returns a list of dynamic field info including object IDs and types.
    pub fn fetch_dynamic_fields(&self, parent_id: &str) -> Result<Vec<DynamicFieldEntry>> {
        let mut all_fields = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = match &cursor {
                Some(c) => serde_json::json!([parent_id, c, 50]),
                None => serde_json::json!([parent_id, null, 50]),
            };

            let result = rpc_request(&self.endpoint, "suix_getDynamicFields", params)?;

            let data = result
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();

            for entry in data {
                // Parse the dynamic field entry
                let name_value = entry.get("name");
                let name_type = name_value
                    .and_then(|n| n.get("type"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default();

                let name_json = name_value.and_then(|n| n.get("value")).cloned();

                // BCS-encoded name
                let name_bcs = entry.get("bcsName").and_then(|b| b.as_str()).and_then(|s| {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.decode(s).ok()
                });

                let object_id = entry
                    .get("objectId")
                    .and_then(|o| o.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default();

                let object_type = entry
                    .get("objectType")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());

                let version = entry.get("version").and_then(|v| v.as_u64());

                let digest = entry
                    .get("digest")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string());

                all_fields.push(DynamicFieldEntry {
                    name_type,
                    name_json,
                    name_bcs,
                    object_id,
                    object_type,
                    version,
                    digest,
                });
            }

            // Check for more pages
            let has_next = result
                .get("hasNextPage")
                .and_then(|h| h.as_bool())
                .unwrap_or(false);

            if has_next {
                cursor = result
                    .get("nextCursor")
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(all_fields)
    }

    /// Recursively fetch dynamic fields and all their nested children.
    /// This is useful for structures like skip_lists that have nested UIDs.
    ///
    /// # Arguments
    /// * `parent_id` - The parent object/UID to start from
    /// * `max_depth` - Maximum recursion depth (0 = just direct children)
    /// * `max_total` - Maximum total entries to fetch
    pub fn fetch_dynamic_fields_recursive(
        &self,
        parent_id: &str,
        max_depth: usize,
        max_total: usize,
    ) -> Result<Vec<DynamicFieldEntry>> {
        let mut all_fields = Vec::new();
        let mut queue = vec![(parent_id.to_string(), 0usize)];
        let mut visited = std::collections::HashSet::new();
        visited.insert(parent_id.to_string());

        while let Some((current_id, depth)) = queue.pop() {
            if all_fields.len() >= max_total {
                break;
            }

            // Fetch children of this ID
            match self.fetch_dynamic_fields(&current_id) {
                Ok(fields) => {
                    for field in fields {
                        if all_fields.len() >= max_total {
                            break;
                        }

                        all_fields.push(field.clone());

                        // If we haven't reached max depth, queue child objects for scanning
                        if depth < max_depth && !visited.contains(&field.object_id) {
                            visited.insert(field.object_id.clone());
                            queue.push((field.object_id.clone(), depth + 1));
                        }
                    }
                }
                Err(e) => {
                    // Log but continue - some UIDs may not have children
                    eprintln!(
                        "Warning: Failed to fetch dynamic fields for {}: {}",
                        current_id, e
                    );
                }
            }
        }

        Ok(all_fields)
    }

    /// Fetch a dynamic field object at a specific version.
    /// This is useful for historical replay where we need the dynamic field
    /// state at a specific transaction version.
    ///
    /// # Arguments
    /// * `object_id` - The dynamic field object ID
    /// * `version` - The version to fetch (usually from DynamicFieldEntry.version)
    ///
    /// # Returns
    /// The BCS bytes of the dynamic field wrapper object
    pub fn fetch_dynamic_field_at_version(
        &self,
        object_id: &str,
        version: u64,
    ) -> Result<FetchedObject> {
        self.fetch_object_at_version_full(object_id, version)
    }

    /// Fetch multiple dynamic field objects at their historical versions.
    /// Uses the archive endpoint for historical lookups.
    ///
    /// # Arguments
    /// * `entries` - List of (object_id, version) pairs to fetch
    ///
    /// # Returns
    /// Map from object_id to FetchedObject
    pub fn fetch_dynamic_fields_at_versions(
        &self,
        entries: &[(String, u64)],
    ) -> Result<std::collections::HashMap<String, FetchedObject>> {
        let mut result = std::collections::HashMap::new();

        for (object_id, version) in entries {
            match self.fetch_dynamic_field_at_version(object_id, *version) {
                Ok(fetched) => {
                    result.insert(object_id.clone(), fetched);
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to fetch dynamic field {} at version {}: {}",
                        object_id, version, e
                    );
                }
            }
        }

        Ok(result)
    }
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
                .map(|bytes| (df.parent_id.clone(), df.type_string.clone(), bytes, df.version))
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
/// Returns a map: on-chain package ID -> bytecode self-address
fn build_address_aliases(
    cached: &CachedTransaction,
) -> std::collections::HashMap<AccountAddress, AccountAddress> {
    use move_binary_format::file_format::CompiledModule;

    let mut aliases = std::collections::HashMap::new();

    for pkg_id in cached.packages.keys() {
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            // Get the target address (on-chain package ID)
            let target_addr = match AccountAddress::from_hex_literal(pkg_id) {
                Ok(addr) => addr,
                Err(_) => continue,
            };

            // Find the source address from bytecode
            for (_name, bytes) in &modules {
                if bytes.is_empty() {
                    continue;
                }
                if let Ok(module) = CompiledModule::deserialize_with_defaults(bytes) {
                    let source_addr = *module.self_id().address();
                    if source_addr != target_addr {
                        aliases.insert(target_addr, source_addr);
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

/// Download and cache a single transaction by digest.
///
/// Returns Ok(true) if the transaction was fetched and cached, Ok(false) if already cached.
pub fn download_single_transaction(
    fetcher: &TransactionFetcher,
    cache: &TransactionCache,
    digest: &str,
    fetch_packages: bool,
    fetch_objects: bool,
    verbose: bool,
) -> Result<bool> {
    use std::io::Write;

    // Skip if already cached
    if cache.has(digest) {
        if verbose {
            eprintln!("Transaction {} already cached", digest);
        }
        return Ok(false);
    }

    if verbose {
        eprint!("Fetching transaction {}...", digest);
        std::io::stderr().flush().ok();
    }

    // Fetch the transaction
    let tx = fetcher.fetch_transaction_sync(digest)?;
    let mut cached = CachedTransaction::new(tx);

    // Fetch packages if requested
    if fetch_packages {
        if let Ok(packages) = fetcher.fetch_transaction_packages(&cached.transaction) {
            for (pkg_id, modules) in packages {
                cached.add_package(pkg_id, modules);
            }
        }

        // Fetch transitive dependencies (up to 3 levels deep)
        for _depth in 0..3 {
            let missing = find_missing_dependencies(&cached);
            if missing.is_empty() {
                break;
            }

            if verbose {
                eprint!(" (+{} deps)", missing.len());
                std::io::stderr().flush().ok();
            }

            for pkg_addr in missing {
                let pkg_hex = format!("0x{}", pkg_addr.to_hex());
                match fetcher.fetch_package_modules(&pkg_hex) {
                    Ok(modules) => {
                        cached.add_package(pkg_hex, modules);
                    }
                    Err(_e) => {
                        // Dependency not found - might be a deleted/old package
                    }
                }
            }
        }
    }

    // Fetch objects if requested (with type info)
    if fetch_objects {
        if let Ok(objects) = fetcher.fetch_transaction_inputs_with_types(&cached.transaction) {
            for (obj_id, (bytes, type_str)) in objects {
                cached.add_object_with_type(obj_id, bytes, type_str);
            }
        }
    }

    // Fetch packages referenced in type arguments (coin types, etc.)
    // This runs after objects are fetched so we can also check object_types
    if fetch_packages {
        let type_arg_packages = extract_type_argument_packages(&cached);
        if !type_arg_packages.is_empty() {
            if verbose {
                eprint!(" (+{} type pkgs)", type_arg_packages.len());
                std::io::stderr().flush().ok();
            }

            for pkg_addr in type_arg_packages {
                let pkg_hex = format!("0x{}", pkg_addr.to_hex());
                match fetcher.fetch_package_modules(&pkg_hex) {
                    Ok(modules) => {
                        cached.add_package(pkg_hex, modules);
                    }
                    Err(_e) => {
                        // Type package not found - might be a deleted/old package
                    }
                }
            }
        }
    }

    // Save to cache
    cache.save(&cached)?;

    if verbose {
        eprintln!(
            " ok ({} cmds, {} pkgs, {} objs)",
            cached.transaction.commands.len(),
            cached.packages.len(),
            cached.objects.len()
        );
    }

    Ok(true)
}

/// Download and cache transactions from mainnet.
///
/// Returns the number of new transactions cached.
pub fn download_transactions(
    fetcher: &TransactionFetcher,
    cache: &TransactionCache,
    count: usize,
    fetch_packages: bool,
    fetch_objects: bool,
    verbose: bool,
) -> Result<usize> {
    use std::io::Write;

    if verbose {
        eprintln!("Fetching {} recent transaction digests...", count);
    }

    let digests = fetcher.fetch_recent_transactions(count)?;
    let mut new_count = 0;

    for (i, digest) in digests.iter().enumerate() {
        // Skip if already cached
        if cache.has(&digest.0) {
            if verbose {
                eprintln!(
                    "  [{}/{}] {} - cached",
                    i + 1,
                    digests.len(),
                    &digest.0[..12]
                );
            }
            continue;
        }

        if verbose {
            eprint!(
                "  [{}/{}] {} - fetching...",
                i + 1,
                digests.len(),
                &digest.0[..12]
            );
            std::io::stderr().flush().ok();
        }

        // Fetch the transaction
        match fetcher.fetch_transaction_sync(&digest.0) {
            Ok(tx) => {
                let mut cached = CachedTransaction::new(tx);

                // Fetch packages if requested
                if fetch_packages {
                    if let Ok(packages) = fetcher.fetch_transaction_packages(&cached.transaction) {
                        for (pkg_id, modules) in packages {
                            cached.add_package(pkg_id, modules);
                        }
                    }

                    // Fetch transitive dependencies (up to 3 levels deep)
                    for _depth in 0..3 {
                        let missing = find_missing_dependencies(&cached);
                        if missing.is_empty() {
                            break;
                        }

                        if verbose {
                            eprint!(" (+{} deps)", missing.len());
                            std::io::stderr().flush().ok();
                        }

                        for pkg_addr in missing {
                            let pkg_hex = format!("0x{}", pkg_addr.to_hex());
                            match fetcher.fetch_package_modules(&pkg_hex) {
                                Ok(modules) => {
                                    cached.add_package(pkg_hex, modules);
                                }
                                Err(_e) => {
                                    // Dependency not found - might be a deleted/old package
                                }
                            }
                        }
                    }
                }

                // Fetch objects if requested (with type info)
                if fetch_objects {
                    if let Ok(objects) =
                        fetcher.fetch_transaction_inputs_with_types(&cached.transaction)
                    {
                        for (obj_id, (bytes, type_str)) in objects {
                            cached.add_object_with_type(obj_id, bytes, type_str);
                        }
                    }
                }

                // Fetch packages referenced in type arguments (coin types, etc.)
                // This runs after objects are fetched so we can also check object_types
                if fetch_packages {
                    let type_arg_packages = extract_type_argument_packages(&cached);
                    if !type_arg_packages.is_empty() {
                        if verbose {
                            eprint!(" (+{} type pkgs)", type_arg_packages.len());
                            std::io::stderr().flush().ok();
                        }

                        for pkg_addr in type_arg_packages {
                            let pkg_hex = format!("0x{}", pkg_addr.to_hex());
                            match fetcher.fetch_package_modules(&pkg_hex) {
                                Ok(modules) => {
                                    cached.add_package(pkg_hex, modules);
                                }
                                Err(_e) => {
                                    // Type package not found - might be a deleted/old package
                                }
                            }
                        }
                    }
                }

                // Save to cache
                if let Err(e) = cache.save(&cached) {
                    if verbose {
                        eprintln!(" error saving: {}", e);
                    }
                } else {
                    new_count += 1;
                    if verbose {
                        eprintln!(
                            " ok ({} cmds, {} pkgs)",
                            cached.transaction.commands.len(),
                            cached.packages.len()
                        );
                    }
                }
            }
            Err(e) => {
                if verbose {
                    eprintln!(" error: {}", e);
                }
            }
        }
    }

    Ok(new_count)
}

/// Find missing package dependencies from the cached transaction's packages.
/// Returns a list of package addresses that are referenced but not present.
fn find_missing_dependencies(cached: &CachedTransaction) -> Vec<AccountAddress> {
    use move_binary_format::file_format::CompiledModule;
    use std::collections::BTreeSet;

    // Framework addresses we always have bundled
    let framework_addrs: BTreeSet<AccountAddress> = [
        AccountAddress::from_hex_literal("0x1").unwrap(),
        AccountAddress::from_hex_literal("0x2").unwrap(),
        AccountAddress::from_hex_literal("0x3").unwrap(),
    ]
    .into_iter()
    .collect();

    // Build set of loaded package addresses
    let mut loaded_addrs: BTreeSet<AccountAddress> = BTreeSet::new();
    for pkg_id in cached.packages.keys() {
        if let Ok(addr) = AccountAddress::from_hex_literal(pkg_id) {
            loaded_addrs.insert(addr);
        }
    }

    // Find all dependencies by parsing modules
    let mut missing = BTreeSet::new();

    for _pkg_id in cached.packages.keys() {
        if let Some(module_bytes_list) = cached.get_package_modules(_pkg_id) {
            for (_name, bytes) in module_bytes_list {
                if bytes.is_empty() {
                    continue;
                }
                if let Ok(module) = CompiledModule::deserialize_with_defaults(&bytes) {
                    // Check all module handles for dependencies
                    for handle in &module.module_handles {
                        let addr = *module.address_identifier_at(handle.address);

                        // Skip framework and already loaded
                        if framework_addrs.contains(&addr) {
                            continue;
                        }
                        if loaded_addrs.contains(&addr) {
                            continue;
                        }

                        missing.insert(addr);
                    }
                }
            }
        }
    }

    missing.into_iter().collect()
}

/// Extract package addresses from type arguments in transaction commands.
///
/// Type arguments like `0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC`
/// contain package addresses that need to be fetched for complete type resolution.
fn extract_type_argument_packages(cached: &CachedTransaction) -> Vec<AccountAddress> {
    use std::collections::BTreeSet;

    // Framework addresses we always have bundled
    let framework_addrs: BTreeSet<AccountAddress> = [
        AccountAddress::from_hex_literal("0x1").unwrap(),
        AccountAddress::from_hex_literal("0x2").unwrap(),
        AccountAddress::from_hex_literal("0x3").unwrap(),
    ]
    .into_iter()
    .collect();

    // Build set of loaded package addresses
    let mut loaded_addrs: BTreeSet<AccountAddress> = BTreeSet::new();
    for pkg_id in cached.packages.keys() {
        if let Ok(addr) = AccountAddress::from_hex_literal(pkg_id) {
            loaded_addrs.insert(addr);
        }
    }

    let mut missing = BTreeSet::new();

    // Helper to extract hex addresses from a type string
    let extract_addrs = |type_str: &str| -> Vec<AccountAddress> {
        let mut addrs = Vec::new();
        let mut i = 0;
        let chars: Vec<char> = type_str.chars().collect();

        while i < chars.len() {
            // Look for "0x" prefix
            if i + 1 < chars.len() && chars[i] == '0' && chars[i + 1] == 'x' {
                // Found potential hex address, collect hex chars
                let start = i;
                i += 2; // Skip "0x"
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }

                // Extract the address string
                let addr_str: String = chars[start..i].iter().collect();
                if let Ok(addr) = AccountAddress::from_hex_literal(&addr_str) {
                    addrs.push(addr);
                }
            } else {
                i += 1;
            }
        }
        addrs
    };

    // Extract from command type arguments
    for cmd in &cached.transaction.commands {
        if let PtbCommand::MoveCall { type_arguments, .. } = cmd {
            for type_arg in type_arguments {
                for addr in extract_addrs(type_arg) {
                    if !framework_addrs.contains(&addr) && !loaded_addrs.contains(&addr) {
                        missing.insert(addr);
                    }
                }
            }
        }
    }

    // Also extract from cached object types
    for type_str in cached.object_types.values() {
        for addr in extract_addrs(type_str) {
            if !framework_addrs.contains(&addr) && !loaded_addrs.contains(&addr) {
                missing.insert(addr);
            }
        }
    }

    missing.into_iter().collect()
}

/// Parse a transaction response from the RPC.
fn parse_transaction_response(
    digest: &str,
    result: &serde_json::Value,
) -> Result<FetchedTransaction> {
    let transaction = result
        .get("transaction")
        .ok_or_else(|| anyhow!("No transaction in result"))?;

    let data = transaction
        .get("data")
        .ok_or_else(|| anyhow!("No data in transaction"))?;

    // Parse sender
    let sender_str = data
        .get("sender")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("No sender in transaction"))?;
    let sender = AccountAddress::from_hex_literal(sender_str)
        .map_err(|e| anyhow!("Invalid sender address: {}", e))?;

    // Parse gas data
    let gas_data = data.get("gasData").unwrap_or(&serde_json::Value::Null);
    let gas_budget = gas_data
        .get("budget")
        .and_then(|b| b.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let gas_price = gas_data
        .get("price")
        .and_then(|p| p.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // Parse PTB transaction - handle both old and new RPC formats
    let tx_data = data.get("transaction");

    let (commands, inputs) = if let Some(tx) = tx_data {
        // Check if this is a ProgrammableTransaction
        let kind = tx.get("kind").and_then(|k| k.as_str());

        if kind == Some("ProgrammableTransaction") {
            // New format: transaction.inputs and transaction.transactions
            let commands = parse_ptb_commands(tx.get("transactions"))?;
            let inputs = parse_ptb_inputs(tx.get("inputs"))?;
            (commands, inputs)
        } else if let Some(ptb) = tx.get("ProgrammableTransaction") {
            // Old format: transaction.ProgrammableTransaction.commands/inputs
            let commands = parse_ptb_commands(ptb.get("commands"))?;
            let inputs = parse_ptb_inputs(ptb.get("inputs"))?;
            (commands, inputs)
        } else {
            (vec![], vec![])
        }
    } else {
        (vec![], vec![])
    };

    // Parse effects
    let effects = result.get("effects").and_then(|e| parse_effects(e).ok());

    // Parse timestamp and checkpoint
    let timestamp_ms = result
        .get("timestampMs")
        .and_then(|t| t.as_str())
        .and_then(|s| s.parse().ok());
    let checkpoint = result
        .get("checkpoint")
        .and_then(|c| c.as_str())
        .and_then(|s| s.parse().ok());

    Ok(FetchedTransaction {
        digest: TransactionDigest::new(digest),
        sender,
        gas_budget,
        gas_price,
        commands,
        inputs,
        effects,
        timestamp_ms,
        checkpoint,
    })
}

/// Parse PTB commands from JSON.
fn parse_ptb_commands(commands: Option<&serde_json::Value>) -> Result<Vec<PtbCommand>> {
    let commands = match commands {
        Some(c) => c
            .as_array()
            .ok_or_else(|| anyhow!("Commands not an array"))?,
        None => return Ok(vec![]),
    };

    let mut result = Vec::new();

    for cmd in commands {
        if let Some(move_call) = cmd.get("MoveCall") {
            let package = move_call
                .get("package")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            let module = move_call
                .get("module")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let function = move_call
                .get("function")
                .and_then(|f| f.as_str())
                .unwrap_or("")
                .to_string();
            let type_arguments = move_call
                .get("type_arguments")
                .and_then(|ta| ta.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let arguments = parse_ptb_arguments(move_call.get("arguments"))?;

            result.push(PtbCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            });
        } else if let Some(split_coins) = cmd.get("SplitCoins") {
            let coin = parse_single_argument(split_coins.get(0))?;
            let amounts = parse_ptb_arguments(split_coins.get(1))?;
            result.push(PtbCommand::SplitCoins { coin, amounts });
        } else if let Some(merge_coins) = cmd.get("MergeCoins") {
            let destination = parse_single_argument(merge_coins.get(0))?;
            let sources = parse_ptb_arguments(merge_coins.get(1))?;
            result.push(PtbCommand::MergeCoins {
                destination,
                sources,
            });
        } else if let Some(transfer) = cmd.get("TransferObjects") {
            let objects = parse_ptb_arguments(transfer.get(0))?;
            let address = parse_single_argument(transfer.get(1))?;
            result.push(PtbCommand::TransferObjects { objects, address });
        } else if let Some(make_vec) = cmd.get("MakeMoveVec") {
            let type_arg = make_vec.get(0).and_then(|t| t.as_str()).map(String::from);
            let elements = parse_ptb_arguments(make_vec.get(1))?;
            result.push(PtbCommand::MakeMoveVec { type_arg, elements });
        }
        // Publish and Upgrade are more complex and less common
    }

    Ok(result)
}

/// Parse a single PTB argument.
fn parse_single_argument(arg: Option<&serde_json::Value>) -> Result<PtbArgument> {
    let arg = arg.ok_or_else(|| anyhow!("Missing argument"))?;

    if let Some(input) = arg.get("Input") {
        let index = input.as_u64().unwrap_or(0) as u16;
        return Ok(PtbArgument::Input { index });
    }

    if let Some(result) = arg.get("Result") {
        let index = result.as_u64().unwrap_or(0) as u16;
        return Ok(PtbArgument::Result { index });
    }

    if let Some(nested) = arg.get("NestedResult") {
        if let Some(arr) = nested.as_array() {
            let index = arr.first().and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            let result_index = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            return Ok(PtbArgument::NestedResult {
                index,
                result_index,
            });
        }
    }

    if arg.as_str() == Some("GasCoin") {
        return Ok(PtbArgument::GasCoin);
    }

    Err(anyhow!("Unknown argument type: {:?}", arg))
}

/// Parse PTB arguments from JSON array.
fn parse_ptb_arguments(args: Option<&serde_json::Value>) -> Result<Vec<PtbArgument>> {
    let args = match args {
        Some(a) => a
            .as_array()
            .ok_or_else(|| anyhow!("Arguments not an array"))?,
        None => return Ok(vec![]),
    };

    args.iter()
        .map(|a| parse_single_argument(Some(a)))
        .collect()
}

/// Parse PTB inputs from JSON.
fn parse_ptb_inputs(inputs: Option<&serde_json::Value>) -> Result<Vec<TransactionInput>> {
    let inputs = match inputs {
        Some(i) => i.as_array().ok_or_else(|| anyhow!("Inputs not an array"))?,
        None => return Ok(vec![]),
    };

    let mut result = Vec::new();

    for input in inputs {
        // Handle the new RPC format with "type" field
        let input_type = input.get("type").and_then(|t| t.as_str());

        match input_type {
            Some("pure") => {
                // New format: {"type": "pure", "valueType": "...", "value": ...}
                let value_type = input.get("valueType").and_then(|t| t.as_str());
                let value = input.get("value");
                let bytes = if let Some(v) = value {
                    // Convert based on valueType
                    match value_type {
                        Some("u8") => {
                            let n: u8 = if let Some(n) = v.as_u64() {
                                n as u8
                            } else if let Some(s) = v.as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                0
                            };
                            vec![n]
                        }
                        Some("u16") => {
                            let n: u16 = if let Some(n) = v.as_u64() {
                                n as u16
                            } else if let Some(s) = v.as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                0
                            };
                            n.to_le_bytes().to_vec()
                        }
                        Some("u32") => {
                            let n: u32 = if let Some(n) = v.as_u64() {
                                n as u32
                            } else if let Some(s) = v.as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                0
                            };
                            n.to_le_bytes().to_vec()
                        }
                        Some("u64") => {
                            let n: u64 = if let Some(n) = v.as_u64() {
                                n
                            } else if let Some(s) = v.as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                0
                            };
                            n.to_le_bytes().to_vec()
                        }
                        Some("u128") => {
                            let n: u128 = if let Some(s) = v.as_str() {
                                s.parse().unwrap_or(0)
                            } else if let Some(n) = v.as_u64() {
                                n as u128
                            } else {
                                0
                            };
                            n.to_le_bytes().to_vec()
                        }
                        Some("u256") => {
                            // u256 comes as a string, convert to 32 bytes LE
                            if let Some(s) = v.as_str() {
                                // Parse as hex or decimal
                                let n = if s.starts_with("0x") || s.starts_with("0X") {
                                    u128::from_str_radix(&s[2..], 16).unwrap_or(0)
                                } else {
                                    s.parse::<u128>().unwrap_or(0)
                                };
                                let mut bytes = n.to_le_bytes().to_vec();
                                bytes.resize(32, 0); // Extend to 32 bytes
                                bytes
                            } else {
                                vec![0u8; 32]
                            }
                        }
                        Some("bool") => {
                            let b = if let Some(b) = v.as_bool() {
                                b
                            } else if let Some(s) = v.as_str() {
                                s == "true"
                            } else {
                                false
                            };
                            vec![if b { 1 } else { 0 }]
                        }
                        Some("address") => {
                            // Address comes as "0x..." hex string
                            if let Some(s) = v.as_str() {
                                let hex_str = s.strip_prefix("0x").unwrap_or(s);
                                hex::decode(hex_str).unwrap_or_else(|_| vec![0u8; 32])
                            } else {
                                vec![0u8; 32]
                            }
                        }
                        Some(vt) if vt.starts_with("vector<u8>") => {
                            // Vector of bytes - could be array or hex string
                            if let Some(arr) = v.as_array() {
                                // BCS vector: length prefix + elements
                                let bytes: Vec<u8> = arr
                                    .iter()
                                    .filter_map(|x| x.as_u64().map(|n| n as u8))
                                    .collect();
                                let mut result = Vec::new();
                                // ULEB128 length prefix
                                let len = bytes.len();
                                let mut len_val = len;
                                loop {
                                    let mut byte = (len_val & 0x7f) as u8;
                                    len_val >>= 7;
                                    if len_val != 0 {
                                        byte |= 0x80;
                                    }
                                    result.push(byte);
                                    if len_val == 0 {
                                        break;
                                    }
                                }
                                result.extend(bytes);
                                result
                            } else if let Some(s) = v.as_str() {
                                // Hex string
                                let hex_str = s.strip_prefix("0x").unwrap_or(s);
                                let bytes = hex::decode(hex_str).unwrap_or_default();
                                let mut result = Vec::new();
                                // ULEB128 length prefix
                                let len = bytes.len();
                                let mut len_val = len;
                                loop {
                                    let mut byte = (len_val & 0x7f) as u8;
                                    len_val >>= 7;
                                    if len_val != 0 {
                                        byte |= 0x80;
                                    }
                                    result.push(byte);
                                    if len_val == 0 {
                                        break;
                                    }
                                }
                                result.extend(bytes);
                                result
                            } else {
                                vec![0] // Empty vector
                            }
                        }
                        _ => {
                            // Fallback: try direct interpretation
                            if let Some(n) = v.as_u64() {
                                n.to_le_bytes().to_vec()
                            } else if let Some(s) = v.as_str() {
                                // Could be an address or other hex value
                                if let Some(hex_str) = s.strip_prefix("0x") {
                                    hex::decode(hex_str).unwrap_or_else(|_| s.as_bytes().to_vec())
                                } else if let Ok(n) = s.parse::<u64>() {
                                    n.to_le_bytes().to_vec()
                                } else {
                                    s.as_bytes().to_vec()
                                }
                            } else if let Some(b) = v.as_bool() {
                                vec![if b { 1 } else { 0 }]
                            } else if let Some(arr) = v.as_array() {
                                arr.iter()
                                    .filter_map(|x| x.as_u64().map(|n| n as u8))
                                    .collect()
                            } else {
                                vec![]
                            }
                        }
                    }
                } else {
                    vec![]
                };
                result.push(TransactionInput::Pure { bytes });
            }
            Some("object") => {
                // New format: {"type": "object", "objectType": "sharedObject"|"immOrOwnedObject", ...}
                let object_type = input.get("objectType").and_then(|t| t.as_str());
                let object_id = input
                    .get("objectId")
                    .and_then(|o| o.as_str())
                    .unwrap_or("")
                    .to_string();

                match object_type {
                    Some("sharedObject") => {
                        let initial_version = input
                            .get("initialSharedVersion")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);
                        let mutable = input
                            .get("mutable")
                            .and_then(|m| m.as_bool())
                            .unwrap_or(true);
                        result.push(TransactionInput::SharedObject {
                            object_id,
                            initial_shared_version: initial_version,
                            mutable,
                        });
                    }
                    _ => {
                        let version = input
                            .get("version")
                            .and_then(|v| v.as_str().or_else(|| v.as_u64().map(|_| "")))
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);
                        let digest = input
                            .get("digest")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string();
                        result.push(TransactionInput::Object {
                            object_id,
                            version,
                            digest,
                        });
                    }
                }
            }
            // Unknown type - skip
            _ => {}
        }
    }

    Ok(result)
}

/// Parse transaction effects.
fn parse_effects(effects: &serde_json::Value) -> Result<TransactionEffectsSummary> {
    let status = if effects
        .get("status")
        .and_then(|s| s.get("status"))
        .and_then(|s| s.as_str())
        == Some("success")
    {
        TransactionStatus::Success
    } else {
        let error = effects
            .get("status")
            .and_then(|s| s.get("error"))
            .and_then(|e| e.as_str())
            .unwrap_or("unknown error")
            .to_string();
        TransactionStatus::Failure { error }
    };

    let created = parse_object_refs(effects.get("created"));
    let mutated = parse_object_refs(effects.get("mutated"));
    let deleted = parse_object_refs(effects.get("deleted"));
    let wrapped = parse_object_refs(effects.get("wrapped"));
    let unwrapped = parse_object_refs(effects.get("unwrapped"));

    let gas_used = effects
        .get("gasUsed")
        .map(|g| GasSummary {
            computation_cost: g
                .get("computationCost")
                .and_then(|c| c.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            storage_cost: g
                .get("storageCost")
                .and_then(|c| c.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            storage_rebate: g
                .get("storageRebate")
                .and_then(|c| c.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            non_refundable_storage_fee: g
                .get("nonRefundableStorageFee")
                .and_then(|c| c.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        })
        .unwrap_or_default();

    // Parse shared object versions from effects.sharedObjects
    // Format: [{ "objectId": "0x...", "version": 123, "digest": "..." }, ...]
    let shared_object_versions = effects
        .get("sharedObjects")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let object_id = item.get("objectId")?.as_str()?.to_string();
                    let version = item
                        .get("version")
                        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))?;
                    Some((object_id, version))
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(TransactionEffectsSummary {
        status,
        created,
        mutated,
        deleted,
        wrapped,
        unwrapped,
        gas_used,
        events_count: 0, // Events not parsed for now
        shared_object_versions,
    })
}

/// Parse object references from effects.
fn parse_object_refs(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    item.get("reference")
                        .or(item.get("objectId"))
                        .and_then(|r| r.get("objectId").or(Some(r)))
                        .and_then(|o| o.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default()
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
    fn test_fetcher_endpoints() {
        let mainnet = TransactionFetcher::mainnet();
        assert!(mainnet.endpoint().contains("mainnet"));

        let testnet = TransactionFetcher::testnet();
        assert!(testnet.endpoint().contains("testnet"));
    }

    #[test]
    fn test_fetcher_with_grpc_archive() {
        let fetcher = TransactionFetcher::mainnet_with_archive();
        assert!(fetcher.endpoint().contains("mainnet"));
        assert!(fetcher.grpc_archive_endpoint.is_some());
        assert!(fetcher
            .grpc_archive_endpoint
            .as_ref()
            .unwrap()
            .contains("archive"));

        // Test builder pattern
        let fetcher2 = TransactionFetcher::mainnet()
            .with_grpc_archive_endpoint("https://custom-archive.example.com");
        assert_eq!(
            fetcher2.grpc_archive_endpoint.as_ref().unwrap(),
            "https://custom-archive.example.com"
        );
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
