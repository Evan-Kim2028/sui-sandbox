//! VM setup, fetcher installation, and PTB execution.
//!
//! Handles the configuration and execution of PTBs in the SimulationEnvironment,
//! including:
//! - Environment configuration (sender, timestamp, version tracking)
//! - Address alias setup for package upgrades
//! - Child-fetcher installation for dynamic field lookups
//! - Gas coin mutation patching
//!
//! Note: Some types may appear unused until full migration is complete.

#![allow(dead_code)]

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sui_sandbox_core::ptb::{Command, InputValue, TransactionEffects};
use sui_sandbox_core::simulation::SimulationEnvironment;

// Note: ObjectEntry and TieredObjectStore will be used when full integration is complete

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for transaction execution.
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    /// Enable Sui native functions
    pub use_sui_natives: bool,
    /// Track object versions for parity comparison
    pub track_versions: bool,
    /// Install child-fetcher for dynamic field resolution
    pub enable_child_fetcher: bool,
    /// Install key-based fetcher for dynamic field by key
    pub enable_key_based_fetcher: bool,
    /// Gas budget override (if any)
    pub gas_budget: Option<u64>,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            use_sui_natives: true,
            track_versions: true,
            enable_child_fetcher: false,
            enable_key_based_fetcher: false,
            gas_budget: None,
        }
    }
}

impl ExecutionConfig {
    /// Create config for WalrusOnly attempt (no child fetcher).
    pub fn walrus_only() -> Self {
        Self::default()
    }

    /// Create config with child fetcher enabled.
    pub fn with_child_fetcher() -> Self {
        Self {
            enable_child_fetcher: true,
            enable_key_based_fetcher: true,
            ..Default::default()
        }
    }

    /// Create config with all features enabled.
    pub fn full() -> Self {
        Self {
            use_sui_natives: true,
            track_versions: true,
            enable_child_fetcher: true,
            enable_key_based_fetcher: true,
            gas_budget: None,
        }
    }
}

// ============================================================================
// Execution Result
// ============================================================================

/// Result of PTB execution.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Whether execution completed successfully
    pub success: bool,
    /// Transaction effects (if successful)
    pub effects: Option<TransactionEffects>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// Raw error for classification
    pub raw_error: Option<String>,
    /// Simulation error type
    pub simulation_error: Option<SimulationErrorKind>,
    /// Gas used
    pub gas_used: u64,
}

/// Simplified simulation error kinds for external use.
#[derive(Debug, Clone)]
pub enum SimulationErrorKind {
    MissingPackage { address: String },
    MissingObject { id: String },
    ExecutionFailure { message: String },
    Other { message: String },
}

impl ExecutionResult {
    /// Create a successful result.
    pub fn success(effects: TransactionEffects, gas_used: u64) -> Self {
        Self {
            success: true,
            effects: Some(effects),
            error: None,
            raw_error: None,
            simulation_error: None,
            gas_used,
        }
    }

    /// Create a failed result.
    pub fn failure(error: impl Into<String>, raw_error: Option<String>) -> Self {
        let error_str = error.into();
        Self {
            success: false,
            effects: None,
            error: Some(error_str.clone()),
            raw_error,
            simulation_error: Some(SimulationErrorKind::Other { message: error_str }),
            gas_used: 0,
        }
    }

    /// Create from simulation error.
    pub fn from_simulation_error(
        err: &sui_sandbox_core::simulation::SimulationError,
        raw: Option<String>,
    ) -> Self {
        use sui_sandbox_core::simulation::SimulationError;

        let kind = match err {
            SimulationError::MissingPackage { address, .. } => {
                SimulationErrorKind::MissingPackage {
                    address: address.to_string(),
                }
            }
            SimulationError::MissingObject { id, .. } => {
                SimulationErrorKind::MissingObject { id: id.to_string() }
            }
            _ => SimulationErrorKind::ExecutionFailure {
                message: format!("{:?}", err),
            },
        };

        Self {
            success: false,
            effects: None,
            error: Some(format!("{:?}", err)),
            raw_error: raw,
            simulation_error: Some(kind),
            gas_used: 0,
        }
    }
}

// ============================================================================
// Fetcher Context
// ============================================================================

/// Context for building child/key-based fetchers.
///
/// Contains the version maps and deny-lists needed for safe object resolution.
#[derive(Clone, Default)]
pub struct FetcherContext {
    /// Known object versions from batch pre-scan
    pub version_map: Arc<HashMap<AccountAddress, u64>>,
    /// Objects created in this transaction (skip fetching)
    pub created_objects: Arc<HashSet<AccountAddress>>,
    /// Objects in Walrus output (authoritative, don't override)
    pub output_objects: Arc<HashSet<AccountAddress>>,
    /// Child objects to deny (due to conflicts)
    pub deny_children: Arc<RwLock<HashSet<AccountAddress>>>,
    /// Parent objects to deny
    pub deny_parents: Arc<RwLock<HashSet<AccountAddress>>>,
}

impl FetcherContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with version map.
    pub fn with_versions(mut self, versions: HashMap<AccountAddress, u64>) -> Self {
        self.version_map = Arc::new(versions);
        self
    }

    /// Create with created objects set.
    pub fn with_created(mut self, created: HashSet<AccountAddress>) -> Self {
        self.created_objects = Arc::new(created);
        self
    }

    /// Create with output objects set.
    pub fn with_outputs(mut self, outputs: HashSet<AccountAddress>) -> Self {
        self.output_objects = Arc::new(outputs);
        self
    }

    /// Check if a child should be denied.
    pub fn is_child_denied(&self, id: &AccountAddress) -> bool {
        self.deny_children.read().contains(id)
    }

    /// Check if a parent should be denied.
    pub fn is_parent_denied(&self, id: &AccountAddress) -> bool {
        self.deny_parents.read().contains(id)
    }

    /// Add a child to the deny list.
    pub fn deny_child(&self, id: AccountAddress) {
        self.deny_children.write().insert(id);
    }

    /// Add a parent to the deny list.
    pub fn deny_parent(&self, id: AccountAddress) {
        self.deny_parents.write().insert(id);
    }

    /// Get version for an object, if known.
    pub fn get_version(&self, id: &AccountAddress) -> Option<u64> {
        self.version_map.get(id).copied()
    }
}

// ============================================================================
// Fetcher Types
// ============================================================================

/// Child object fetcher callback type.
///
/// Given parent ID and child ID, returns (type_tag, bcs_bytes, version) if found.
pub type ChildFetcher =
    Box<dyn Fn(AccountAddress, AccountAddress) -> Option<(TypeTag, Vec<u8>, u64)> + Send + Sync>;

/// Key-based child fetcher for dynamic fields.
///
/// Given parent ID, child ID, key type, and key bytes, returns (type_tag, bcs_bytes).
pub type KeyBasedFetcher = Box<
    dyn Fn(AccountAddress, AccountAddress, &TypeTag, &[u8]) -> Option<(TypeTag, Vec<u8>)>
        + Send
        + Sync,
>;

// ============================================================================
// PTB Executor Trait
// ============================================================================

/// Trait for configuring and running PTB execution.
pub trait PtbExecutor: Send + Sync {
    /// Configure environment for execution.
    fn configure_env(
        &self,
        env: &mut SimulationEnvironment,
        sender: AccountAddress,
        timestamp_ms: Option<u64>,
        lamport_version: Option<u64>,
        config: &ExecutionConfig,
    ) -> Result<()>;

    /// Set address aliases for package resolution.
    fn set_address_aliases(
        &self,
        env: &mut SimulationEnvironment,
        aliases: &HashMap<AccountAddress, AccountAddress>,
        versions: &HashMap<AccountAddress, u64>,
    );

    /// Preload objects into the environment.
    fn preload_objects(&self, env: &mut SimulationEnvironment, inputs: &[InputValue])
        -> Result<()>;

    /// Install child fetcher for dynamic object lookup.
    fn install_child_fetcher(&self, env: &mut SimulationEnvironment, fetcher: ChildFetcher);

    /// Install key-based fetcher for dynamic field lookup.
    fn install_key_based_fetcher(&self, env: &mut SimulationEnvironment, fetcher: KeyBasedFetcher);

    /// Execute PTB commands.
    fn execute(
        &self,
        env: &mut SimulationEnvironment,
        inputs: Vec<InputValue>,
        commands: Vec<Command>,
        gas_budget: Option<u64>,
    ) -> ExecutionResult;
}

// ============================================================================
// Address Alias Builder
// ============================================================================

/// Builds address aliases for package upgrades.
pub struct AliasBuilder {
    /// Runtime address -> Storage address
    aliases: HashMap<AccountAddress, AccountAddress>,
    /// Address -> Version
    versions: HashMap<AccountAddress, u64>,
}

impl AliasBuilder {
    pub fn new() -> Self {
        Self {
            aliases: HashMap::new(),
            versions: HashMap::new(),
        }
    }

    /// Add an alias from runtime to storage address.
    pub fn add_alias(&mut self, runtime: AccountAddress, storage: AccountAddress, version: u64) {
        self.aliases.insert(runtime, storage);
        self.versions.insert(storage, version);
    }

    /// Build from linkage info.
    pub fn from_linkage(linkage: &[(AccountAddress, AccountAddress, u64)]) -> Self {
        let mut builder = Self::new();
        for (runtime, storage, version) in linkage {
            builder.add_alias(*runtime, *storage, *version);
        }
        builder
    }

    /// Get the aliases map.
    pub fn aliases(&self) -> &HashMap<AccountAddress, AccountAddress> {
        &self.aliases
    }

    /// Get the versions map.
    pub fn versions(&self) -> &HashMap<AccountAddress, u64> {
        &self.versions
    }
}

impl Default for AliasBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Gas Coin Utilities
// ============================================================================

/// Extract gas coin ID from transaction JSON.
pub fn extract_gas_coin_id(tx_json: &serde_json::Value) -> Option<AccountAddress> {
    let payment = tx_json.pointer("/transaction/V1/txn_data/V1/gas_data/payment")?;
    let first = payment.as_array()?.first()?;
    let id_str = first.get("object_id")?.as_str()?;
    AccountAddress::from_hex_literal(id_str).ok()
}

/// Extract gas budget from transaction JSON.
pub fn extract_gas_budget(tx_json: &serde_json::Value) -> Option<u64> {
    tx_json
        .pointer("/transaction/V1/txn_data/V1/gas_data/budget")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_config_defaults() {
        let config = ExecutionConfig::default();
        assert!(config.use_sui_natives);
        assert!(config.track_versions);
        assert!(!config.enable_child_fetcher);
        assert!(!config.enable_key_based_fetcher);
    }

    #[test]
    fn test_execution_config_with_child_fetcher() {
        let config = ExecutionConfig::with_child_fetcher();
        assert!(config.enable_child_fetcher);
        assert!(config.enable_key_based_fetcher);
    }

    #[test]
    fn test_fetcher_context() {
        let ctx = FetcherContext::new().with_versions(HashMap::from([(
            AccountAddress::from_hex_literal("0x1").unwrap(),
            5,
        )]));

        let id = AccountAddress::from_hex_literal("0x1").unwrap();
        assert_eq!(ctx.get_version(&id), Some(5));

        let other = AccountAddress::from_hex_literal("0x2").unwrap();
        assert_eq!(ctx.get_version(&other), None);
    }

    #[test]
    fn test_fetcher_context_deny_lists() {
        let ctx = FetcherContext::new();
        let id = AccountAddress::from_hex_literal("0x1").unwrap();

        assert!(!ctx.is_child_denied(&id));
        ctx.deny_child(id);
        assert!(ctx.is_child_denied(&id));

        let parent = AccountAddress::from_hex_literal("0x2").unwrap();
        assert!(!ctx.is_parent_denied(&parent));
        ctx.deny_parent(parent);
        assert!(ctx.is_parent_denied(&parent));
    }

    #[test]
    fn test_alias_builder() {
        let mut builder = AliasBuilder::new();
        let runtime = AccountAddress::from_hex_literal("0x100").unwrap();
        let storage = AccountAddress::from_hex_literal("0x200").unwrap();

        builder.add_alias(runtime, storage, 5);

        assert_eq!(builder.aliases().get(&runtime), Some(&storage));
        assert_eq!(builder.versions().get(&storage), Some(&5));
    }

    #[test]
    fn test_execution_result_success() {
        let effects = TransactionEffects::default();
        let result = ExecutionResult::success(effects, 1000);

        assert!(result.success);
        assert!(result.effects.is_some());
        assert!(result.error.is_none());
        assert_eq!(result.gas_used, 1000);
    }

    #[test]
    fn test_execution_result_failure() {
        let result =
            ExecutionResult::failure("something went wrong", Some("raw error".to_string()));

        assert!(!result.success);
        assert!(result.effects.is_none());
        assert!(result.error.is_some());
        assert_eq!(result.raw_error, Some("raw error".to_string()));
    }
}
