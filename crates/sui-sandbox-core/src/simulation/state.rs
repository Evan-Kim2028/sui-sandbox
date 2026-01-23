//! Persistent state types for simulation serialization.
//!
//! This module provides types for saving and loading simulation state,
//! enabling session persistence and state transfer.

use super::types::CoinMetadata;

// ============================================================================
// Persistent State Types (for serialization)
// ============================================================================

/// Serializable version of SimulatedObject for persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedObject {
    /// Object ID as hex string.
    pub id: String,
    /// Move type as string (e.g., "0x2::coin::Coin<0x2::sui::SUI>").
    pub type_tag: String,
    /// BCS-serialized object contents (base64 encoded).
    pub bcs_bytes_b64: String,
    /// Whether this object is shared.
    pub is_shared: bool,
    /// Whether this object is immutable.
    pub is_immutable: bool,
    /// Version number.
    pub version: u64,
}

/// Serializable module for persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedModule {
    /// Module ID (package::module).
    pub id: String,
    /// Module bytecode (base64 encoded).
    pub bytecode_b64: String,
}

/// Serializable dynamic field for persistence.
/// Dynamic fields are used by Table, Bag, and other collection types.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedDynamicField {
    /// Parent object ID (hex string).
    pub parent_id: String,
    /// Child object ID (hex string).
    pub child_id: String,
    /// Type tag as string (e.g., "0x2::dynamic_field::Field<u64, 0x2::coin::Coin<0x2::sui::SUI>>").
    pub type_tag: String,
    /// BCS-serialized field value (base64 encoded).
    pub value_b64: String,
}

/// Serializable pending receive for persistence.
/// Pending receives are objects that have been transferred to another object
/// and are waiting to be received via `transfer::receive`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedPendingReceive {
    /// Recipient object ID (hex string).
    pub recipient_id: String,
    /// Sent object ID (hex string).
    pub sent_id: String,
    /// Object type tag as string.
    pub type_tag: String,
    /// BCS-serialized object bytes (base64 encoded).
    pub object_bytes_b64: String,
}

/// Persistent sandbox state that can be saved to/loaded from a file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistentState {
    /// Version of the state format (for forward compatibility).
    pub version: u32,
    /// Objects in the environment.
    pub objects: Vec<SerializedObject>,
    /// Non-framework modules (user-deployed packages).
    pub modules: Vec<SerializedModule>,
    /// Coin registry.
    pub coin_registry: std::collections::HashMap<String, CoinMetadata>,
    /// Sender address.
    pub sender: String,
    /// ID counter for generating fresh IDs.
    pub id_counter: u64,
    /// Timestamp in milliseconds.
    pub timestamp_ms: Option<u64>,
    /// Dynamic fields (Table/Bag entries) - added in v2.
    #[serde(default)]
    pub dynamic_fields: Vec<SerializedDynamicField>,
    /// Pending receives (send-to-object pattern) - added in v2.
    #[serde(default)]
    pub pending_receives: Vec<SerializedPendingReceive>,
    /// Simulation configuration (epoch, gas, clock, etc.) - added in v3.
    #[serde(default)]
    pub config: Option<crate::vm::SimulationConfig>,
    /// State file metadata - added in v3.
    #[serde(default)]
    pub metadata: Option<StateMetadata>,
    /// Fetcher configuration for mainnet data access - added in v4.
    /// When present, the fetcher will be auto-reconnected on state load.
    #[serde(default)]
    pub fetcher_config: Option<FetcherConfig>,
}

impl PersistentState {
    /// Current state format version.
    /// v1: Initial version (objects, modules, coins, sender, id_counter, timestamp)
    /// v2: Added dynamic_fields and pending_receives for Table/Bag persistence
    /// v3: Added config (SimulationConfig) and metadata (description, timestamps, tags)
    /// v4: Added fetcher_config for persistent mainnet fetching configuration
    pub const CURRENT_VERSION: u32 = 4;
}

/// Metadata for state files - helps organize multiple simulation scenarios.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct StateMetadata {
    /// Human-readable description of this simulation state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When this state was created (ISO 8601 timestamp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// When this state was last modified (ISO 8601 timestamp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    /// Tags for categorization (e.g., ["defi", "cetus", "testnet"]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Configuration for data fetching - persisted separately from TransactionFetcher
/// because the fetcher itself contains non-serializable runtime state (gRPC clients, tokio runtime).
///
/// This allows save/load to remember that mainnet fetching was enabled and auto-reconnect
/// when the state is restored.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct FetcherConfig {
    /// Whether mainnet fetching is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Network to fetch from: "mainnet" or "testnet".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Custom endpoint URL (if not using default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Whether to use archive endpoint for historical data.
    #[serde(default)]
    pub use_archive: bool,
}

impl FetcherConfig {
    /// Create a config for mainnet fetching.
    pub fn mainnet() -> Self {
        Self {
            enabled: true,
            network: Some("mainnet".to_string()),
            endpoint: None,
            use_archive: false,
        }
    }

    /// Create a config for mainnet with archive support.
    pub fn mainnet_with_archive() -> Self {
        Self {
            enabled: true,
            network: Some("mainnet".to_string()),
            endpoint: None,
            use_archive: true,
        }
    }

    /// Create a config for testnet fetching.
    pub fn testnet() -> Self {
        Self {
            enabled: true,
            network: Some("testnet".to_string()),
            endpoint: None,
            use_archive: false,
        }
    }

    /// Create a config with a custom endpoint.
    pub fn custom(endpoint: impl Into<String>) -> Self {
        Self {
            enabled: true,
            network: None,
            endpoint: Some(endpoint.into()),
            use_archive: false,
        }
    }

    /// Check if this config represents no fetching (disabled).
    pub fn is_disabled(&self) -> bool {
        !self.enabled
    }
}
