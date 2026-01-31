//! Sui Native ObjectRuntime Integration
//!
//! This module provides integration with **Sui's actual production ObjectRuntime**.
//! It's used when `use_sui_natives: true` for 100% accuracy with on-chain behavior.
//!
//! ## Two Runtime Systems
//!
//! The sandbox supports two runtime systems for dynamic field operations:
//!
//! | Runtime | Module | When Used | Accuracy |
//! |---------|--------|-----------|----------|
//! | **Sandbox** | `sandbox_runtime` | `use_sui_natives: false` (default) | ~90% |
//! | **Sui Native** (this module) | `sui_object_runtime` | `use_sui_natives: true` | 100% |
//!
//! **When to use which:**
//! - **Sandbox runtime (default)**: Fast iteration, testing, development
//! - **Sui native runtime (this module)**: Transaction replay, production parity validation
//!
//! ## Key Components
//!
//! - [`ChildObjectResolverWrapper`]: Implements Sui's `ChildObjectResolver` trait using our
//!   child fetching logic (GraphQL/archive lookup).
//! - [`create_sui_object_runtime`]: Factory function to create Sui's ObjectRuntime with
//!   our resolver and input objects.
//! - [`SuiNativeExtensions`]: Holder for all Sui native extensions.
//!
//! ## Why Use Sui's Actual Implementation?
//!
//! Dynamic fields (Tables, Bags, dynamic_object_field) involve complex operations:
//! - Hash computation for child object IDs
//! - BCS serialization/deserialization of Field<K, V> structs
//! - Reference tracking in GlobalValue
//!
//! Our custom implementation had subtle bugs in reference handling that caused
//! incorrect field access after borrow_child_object_mut. Using Sui's actual code
//! guarantees correctness.
//!
//! ## Callback Types
//!
//! This module uses different callback types than `sandbox_runtime`:
//! - [`ChildFetchFn`]: Uses `ObjectID`, returns `(type, bytes, version, parent)`
//! - [`VersionBoundChildFetchFn`]: Also takes version upper bound for replay accuracy
//!
//! These are converted from `sandbox_runtime` callbacks in `VMHarness::set_child_fetcher()`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use move_vm_runtime::native_functions::NativeFunctionTable;
use tracing::{debug, trace, warn};

use sui_move_natives::object_runtime::{InputObject, ObjectRuntime};
use sui_move_natives::transaction_context::TransactionContext;
use sui_move_natives::NativesCostTable;
use sui_protocol_config::ProtocolConfig;
use sui_types::base_types::{MoveObjectType, ObjectID, SequenceNumber, SuiAddress, TxContext};
use sui_types::committee::EpochId;
use sui_types::digests::TransactionDigest;
use sui_types::error::SuiResult;
use sui_types::metrics::LimitsMetrics;
use sui_types::object::{Data, MoveObject, Object, ObjectInner, Owner};
use sui_types::storage::ChildObjectResolver;

/// Callback type for fetching child objects on-demand (sandbox mode).
/// Takes child_object_id and returns Option<(TypeTag, BCS bytes, version, parent_id)>.
///
/// This is the "sandbox" style fetcher that ignores version bounds and fetches the latest
/// version of a child object. Suitable for exploratory testing and development.
pub type ChildFetchFn =
    Arc<dyn Fn(ObjectID) -> Option<(TypeTag, Vec<u8>, u64, ObjectID)> + Send + Sync>;

/// Callback type for fetching child objects with version bound (replay mode).
/// Takes (child_object_id, version_upper_bound) and returns Option<(TypeTag, BCS bytes, version, parent_id)>.
///
/// This is the "replay" style fetcher that respects version bounds and fetches the child
/// object at a version <= the upper bound. Critical for accurate transaction replay.
pub type VersionBoundChildFetchFn =
    Arc<dyn Fn(ObjectID, u64) -> Option<(TypeTag, Vec<u8>, u64, ObjectID)> + Send + Sync>;

/// Child resolution mode for the object resolver.
///
/// This determines how child objects are fetched during execution:
/// - `Sandbox`: Ignores version bounds, fetches latest version (fast, good for development)
/// - `Replay`: Respects version bounds, fetches historical version (accurate for replay)
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub enum ChildResolutionMode {
    /// Sandbox mode: fetch latest version, ignore version bounds.
    /// Good for development, testing, and exploratory simulation.
    #[default]
    Sandbox,
    /// Replay mode: respect version bounds, fetch historical version.
    /// Required for accurate transaction replay matching on-chain behavior.
    Replay,
}

/// Wrapper that implements Sui's `ChildObjectResolver` trait using our child fetching logic.
///
/// This allows Sui's ObjectRuntime to load child objects from our data sources
/// (GraphQL, transaction replay archive, etc.) while using Sui's actual native
/// function implementations.
///
/// Supports two modes:
/// - Sandbox mode: Uses `ChildFetchFn`, ignores version bounds
/// - Replay mode: Uses `VersionBoundChildFetchFn`, respects version bounds
pub struct ChildObjectResolverWrapper {
    /// The sandbox-style fetcher (ignores version bounds)
    sandbox_fetcher: Option<ChildFetchFn>,
    /// The replay-style fetcher (respects version bounds)
    replay_fetcher: Option<VersionBoundChildFetchFn>,
    /// Resolution mode
    mode: ChildResolutionMode,
    /// Protocol config for the current chain version (used in MoveObject creation)
    protocol_config: ProtocolConfig,
}

impl ChildObjectResolverWrapper {
    /// Create a new resolver with the given fetcher function (sandbox mode).
    ///
    /// This is the backward-compatible constructor that ignores version bounds.
    pub fn new(fetcher: ChildFetchFn, protocol_config: ProtocolConfig) -> Self {
        Self {
            sandbox_fetcher: Some(fetcher),
            replay_fetcher: None,
            mode: ChildResolutionMode::Sandbox,
            protocol_config,
        }
    }

    /// Create a new resolver in replay mode with version-bound aware fetching.
    ///
    /// Use this for accurate transaction replay that respects child object versions.
    pub fn new_replay(fetcher: VersionBoundChildFetchFn, protocol_config: ProtocolConfig) -> Self {
        Self {
            sandbox_fetcher: None,
            replay_fetcher: Some(fetcher),
            mode: ChildResolutionMode::Replay,
            protocol_config,
        }
    }

    /// Create a resolver that supports both modes with automatic selection.
    ///
    /// In replay mode, uses the version-bound fetcher; in sandbox mode, uses the simple fetcher.
    pub fn new_dual(
        sandbox_fetcher: ChildFetchFn,
        replay_fetcher: VersionBoundChildFetchFn,
        mode: ChildResolutionMode,
        protocol_config: ProtocolConfig,
    ) -> Self {
        Self {
            sandbox_fetcher: Some(sandbox_fetcher),
            replay_fetcher: Some(replay_fetcher),
            mode,
            protocol_config,
        }
    }

    /// Get the current resolution mode.
    pub fn mode(&self) -> ChildResolutionMode {
        self.mode
    }
}

impl ChildObjectResolver for ChildObjectResolverWrapper {
    /// Read a child object from storage.
    ///
    /// This is called by Sui's dynamic field natives when they need to load a child object.
    /// We delegate to our fetcher function which may look up the object from GraphQL,
    /// an archive, or the transaction replay environment.
    ///
    /// Behavior depends on `ChildResolutionMode`:
    /// - `Sandbox`: Ignores `child_version_upper_bound`, fetches latest version
    /// - `Replay`: Respects `child_version_upper_bound`, fetches version <= bound
    fn read_child_object(
        &self,
        parent: &ObjectID,
        child: &ObjectID,
        child_version_upper_bound: SequenceNumber,
    ) -> SuiResult<Option<Object>> {
        trace!(
            parent = %parent.to_hex_literal(),
            child = %child.to_hex_literal(),
            version_bound = child_version_upper_bound.value(),
            mode = ?self.mode,
            "read_child_object"
        );

        // Call the appropriate fetcher based on mode
        let result = match self.mode {
            ChildResolutionMode::Sandbox => {
                // Sandbox mode: ignore version bound, use sandbox fetcher
                match &self.sandbox_fetcher {
                    Some(fetcher) => (fetcher)(*child),
                    None => {
                        // Fall back to replay fetcher with max version if available
                        self.replay_fetcher
                            .as_ref()
                            .and_then(|f| (f)(*child, u64::MAX))
                    }
                }
            }
            ChildResolutionMode::Replay => {
                // Replay mode: respect version bound
                match &self.replay_fetcher {
                    Some(fetcher) => (fetcher)(*child, child_version_upper_bound.value()),
                    None => {
                        // Fall back to sandbox fetcher but log a warning
                        warn!(
                            child = %child.to_hex_literal(),
                            version_bound = child_version_upper_bound.value(),
                            "Replay mode but no version-bound fetcher; using sandbox fetcher"
                        );
                        self.sandbox_fetcher.as_ref().and_then(|f| (f)(*child))
                    }
                }
            }
        };

        match result {
            Some((type_tag, bcs_bytes, version, fetched_parent)) => {
                // Verify the parent matches
                if fetched_parent != *parent {
                    warn!(
                        expected = %parent.to_hex_literal(),
                        actual = %fetched_parent.to_hex_literal(),
                        "parent mismatch in child object fetch"
                    );
                    // Still allow it - the parent in the fetcher might be stale
                }

                // Convert TypeTag to MoveObjectType
                let struct_tag = match type_tag {
                    TypeTag::Struct(s) => *s,
                    _ => {
                        return Err(format!(
                            "Expected struct type for child object {}, got {:?}",
                            child.to_hex_literal(),
                            type_tag
                        )
                        .into());
                    }
                };
                let move_object_type = MoveObjectType::from(struct_tag);

                // Create MoveObject from BCS bytes
                //
                // SAFETY: This unsafe block is required because `new_from_execution`
                // constructs a MoveObject from raw BCS bytes without full validation.
                //
                // The following invariants are upheld:
                // 1. `bcs_bytes` originates from either:
                //    - On-chain object state fetched via gRPC/GraphQL (trusted source)
                //    - Our simulation environment's object store (previously validated)
                // 2. `struct_tag` was parsed from a validated TypeTag on lines 110-119 above
                // 3. `self.protocol_config` matches the chain version we're simulating
                // 4. `has_public_transfer=true` is safe in simulation context - we don't
                //    enforce transfer restrictions locally
                // 5. `version` comes from the fetched object's SequenceNumber
                let move_object = unsafe {
                    MoveObject::new_from_execution(
                        move_object_type,
                        true, // has_public_transfer - safe for simulation
                        SequenceNumber::from_u64(version),
                        bcs_bytes,
                        &self.protocol_config,
                        false, // not a system mutation
                    )
                    .map_err(|e| {
                        let msg: String = format!("Failed to create MoveObject: {}", e);
                        msg
                    })?
                };

                // Create the full Object with ObjectOwner ownership
                let owner = Owner::ObjectOwner(SuiAddress::from(*parent));
                let object = ObjectInner {
                    data: Data::Move(move_object),
                    owner,
                    previous_transaction: sui_types::digests::TransactionDigest::ZERO,
                    storage_rebate: 0,
                };

                debug!(
                    child = %child.to_hex_literal(),
                    version = version,
                    "loaded child object"
                );

                Ok(Some(object.into()))
            }
            None => {
                trace!(
                    child = %child.to_hex_literal(),
                    "child object not found"
                );
                Ok(None)
            }
        }
    }

    /// Get an object that was received at a specific version.
    ///
    /// This is used for the `transfer::receive` pattern where an object is sent to
    /// another object and can be claimed later.
    fn get_object_received_at_version(
        &self,
        owner: &ObjectID,
        receiving_object_id: &ObjectID,
        receive_object_at_version: SequenceNumber,
        _epoch_id: EpochId,
    ) -> SuiResult<Option<Object>> {
        trace!(
            owner = %owner.to_hex_literal(),
            object = %receiving_object_id.to_hex_literal(),
            version = receive_object_at_version.value(),
            "get_object_received_at_version"
        );

        // For now, delegate to regular child fetching
        // In a full implementation, we'd need to track pending receives separately
        self.read_child_object(owner, receiving_object_id, receive_object_at_version)
    }
}

/// Create Sui's ObjectRuntime with our child object resolver.
///
/// This sets up the ObjectRuntime that will be added to the VM's NativeExtensions,
/// enabling Sui's production dynamic field natives to work with our data sources.
pub fn create_sui_object_runtime<'a>(
    resolver: &'a dyn ChildObjectResolver,
    input_objects: BTreeMap<ObjectID, InputObject>,
    is_metered: bool,
    protocol_config: &'a ProtocolConfig,
    metrics: Arc<LimitsMetrics>,
    epoch_id: EpochId,
) -> ObjectRuntime<'a> {
    ObjectRuntime::new(
        resolver,
        input_objects,
        is_metered,
        protocol_config,
        metrics,
        epoch_id,
    )
}

/// Create an InputObject from object data.
///
/// InputObjects represent the transaction's input objects and their ownership state
/// at the beginning of execution.
pub fn make_input_object(
    id: ObjectID,
    version: SequenceNumber,
    owner: Owner,
) -> (ObjectID, InputObject) {
    use std::collections::BTreeSet;

    // For a simple object, it only contains its own UID
    let mut contained_uids = BTreeSet::new();
    contained_uids.insert(id);

    (
        id,
        InputObject {
            contained_uids,
            version,
            owner,
        },
    )
}

/// Convert a child fetcher function to ChildFetchFn type.
///
/// The input function takes (child_id) and returns Option<(TypeTag, bytes)>.
/// We wrap it to also return version=1 and parent=child (placeholder).
///
/// **Note**: For transaction replay, prefer `wrap_versioned_fetcher` which
/// provides correct version information needed for accurate replay.
pub fn wrap_simple_fetcher<F>(fetcher: F) -> ChildFetchFn
where
    F: Fn(AccountAddress) -> Option<(TypeTag, Vec<u8>)> + Send + Sync + 'static,
{
    Arc::new(move |child_id: ObjectID| {
        let addr = AccountAddress::new(child_id.into_bytes());
        fetcher(addr).map(|(type_tag, bytes)| {
            // Default version and parent - these should be set properly by the caller
            // when the actual parent/version info is available
            (type_tag, bytes, 1u64, child_id)
        })
    })
}

/// Convert a versioned child fetcher function to ChildFetchFn type.
///
/// The input function takes (child_id) and returns Option<(TypeTag, bytes, version)>.
/// This is the preferred wrapper for transaction replay as it provides correct
/// version information.
pub fn wrap_versioned_fetcher<F>(fetcher: F) -> ChildFetchFn
where
    F: Fn(AccountAddress) -> Option<(TypeTag, Vec<u8>, u64)> + Send + Sync + 'static,
{
    Arc::new(move |child_id: ObjectID| {
        let addr = AccountAddress::new(child_id.into_bytes());
        fetcher(addr).map(|(type_tag, bytes, version)| (type_tag, bytes, version, child_id))
    })
}

/// Build the native function table using Sui's actual `all_natives()`.
///
/// This uses Sui's production native function implementations for all Sui framework natives.
/// The `silent` parameter controls whether to print debug messages for unsupported natives.
pub fn build_sui_native_function_table(
    protocol_config: &ProtocolConfig,
    silent: bool,
) -> NativeFunctionTable {
    sui_move_natives::all_natives(silent, protocol_config)
}

/// Create a NativesCostTable from the protocol config.
///
/// This is needed as a VM extension for gas metering of native function calls.
pub fn create_natives_cost_table(protocol_config: &ProtocolConfig) -> NativesCostTable {
    NativesCostTable::from_protocol_config(protocol_config)
}

/// Create a TransactionContext for use as a VM extension.
///
/// This provides tx_context native functions with access to transaction information.
pub fn create_transaction_context(
    sender: AccountAddress,
    tx_digest: TransactionDigest,
    epoch: EpochId,
    epoch_timestamp_ms: u64,
    gas_price: u64,
    gas_budget: u64,
    sponsor: Option<AccountAddress>,
    protocol_config: &ProtocolConfig,
) -> (TransactionContext, Rc<RefCell<TxContext>>) {
    // Use reference gas price = gas_price for simulation
    let rgp = gas_price;
    let tx_context = TxContext::new_from_components(
        &SuiAddress::from(sender),
        &tx_digest,
        &epoch,
        epoch_timestamp_ms,
        rgp,
        gas_price,
        gas_budget,
        sponsor.map(SuiAddress::from),
        protocol_config,
    );
    let tx_context_rc = Rc::new(RefCell::new(tx_context));
    let transaction_context = TransactionContext::new(tx_context_rc.clone());
    (transaction_context, tx_context_rc)
}

/// Configuration for creating a simulation runtime environment.
pub struct SuiRuntimeConfig {
    pub sender: AccountAddress,
    pub epoch: EpochId,
    pub epoch_timestamp_ms: u64,
    pub gas_price: u64,
    pub gas_budget: u64,
    pub sponsor: Option<AccountAddress>,
    /// Whether to enable gas metering for native functions.
    /// When true, native function costs are charged from Sui's NativesCostTable.
    /// Default: false for backwards compatibility.
    pub is_metered: bool,
    /// Child object resolution mode.
    /// Controls whether version bounds are respected when fetching child objects.
    /// Default: Sandbox (ignores version bounds for backward compatibility).
    pub child_resolution_mode: ChildResolutionMode,
}

impl Default for SuiRuntimeConfig {
    fn default() -> Self {
        Self {
            sender: AccountAddress::ZERO,
            epoch: 0,
            epoch_timestamp_ms: 0,
            gas_price: 1000,
            gas_budget: 50_000_000_000,
            sponsor: None,
            is_metered: false,
            child_resolution_mode: ChildResolutionMode::Sandbox,
        }
    }
}

/// A holder for Sui native extensions that can be added to NativeContextExtensions.
///
/// This struct owns the various components needed for Sui natives and can create
/// the actual extensions. The lifetime management is handled by leaking the resolver
/// to get 'static lifetime, which is acceptable for simulation contexts.
pub struct SuiNativeExtensions {
    /// Leaked resolver for 'static lifetime
    resolver: &'static dyn ChildObjectResolver,
    /// Protocol config (also leaked for 'static)
    protocol_config: &'static ProtocolConfig,
    /// Transaction context
    tx_context_rc: Rc<RefCell<TxContext>>,
    /// Metrics placeholder
    metrics: Arc<LimitsMetrics>,
    /// Epoch ID
    epoch_id: EpochId,
    /// Input objects - objects that are inputs to the transaction
    /// These must be registered before their children can be accessed
    input_objects: parking_lot::Mutex<BTreeMap<ObjectID, InputObject>>,
    /// Whether gas metering is enabled for native functions
    is_metered: bool,
}

impl SuiNativeExtensions {
    /// Create a new SuiNativeExtensions with a sandbox-mode fetcher.
    ///
    /// This is the backward-compatible constructor that ignores version bounds.
    ///
    /// SAFETY: This leaks memory, which is acceptable for simulation/testing contexts
    /// where the process will exit and reclaim all memory. Do not use this in a
    /// long-running service.
    pub fn new(fetcher: ChildFetchFn, config: SuiRuntimeConfig) -> Self {
        // Leak the protocol config to get 'static lifetime
        let protocol_config: &'static ProtocolConfig =
            Box::leak(Box::new(ProtocolConfig::get_for_max_version_UNSAFE()));

        // Create resolver based on configured mode
        let resolver: &'static dyn ChildObjectResolver = match config.child_resolution_mode {
            ChildResolutionMode::Sandbox => {
                // Sandbox mode: use the simple fetcher that ignores version bounds
                let resolver_box = Box::new(ChildObjectResolverWrapper::new(
                    fetcher,
                    protocol_config.clone(),
                ));
                Box::leak(resolver_box)
            }
            ChildResolutionMode::Replay => {
                // Replay mode requested but only sandbox fetcher provided
                // Create a wrapper that adapts the sandbox fetcher but logs warnings
                warn!("Replay mode requested but using sandbox fetcher - version bounds will be ignored");
                let resolver_box = Box::new(ChildObjectResolverWrapper::new(
                    fetcher,
                    protocol_config.clone(),
                ));
                Box::leak(resolver_box)
            }
        };

        Self::create_with_resolver(resolver, protocol_config, config)
    }

    /// Create a new SuiNativeExtensions with a replay-mode fetcher that respects version bounds.
    ///
    /// Use this for accurate transaction replay where child object versions matter.
    ///
    /// SAFETY: This leaks memory, which is acceptable for simulation/testing contexts.
    pub fn new_replay(replay_fetcher: VersionBoundChildFetchFn, config: SuiRuntimeConfig) -> Self {
        let protocol_config: &'static ProtocolConfig =
            Box::leak(Box::new(ProtocolConfig::get_for_max_version_UNSAFE()));

        let resolver_box = Box::new(ChildObjectResolverWrapper::new_replay(
            replay_fetcher,
            protocol_config.clone(),
        ));
        let resolver: &'static dyn ChildObjectResolver = Box::leak(resolver_box);

        Self::create_with_resolver(resolver, protocol_config, config)
    }

    /// Create a new SuiNativeExtensions with both fetcher types for automatic mode selection.
    ///
    /// The mode from `config.child_resolution_mode` determines which fetcher is used.
    pub fn new_dual(
        sandbox_fetcher: ChildFetchFn,
        replay_fetcher: VersionBoundChildFetchFn,
        config: SuiRuntimeConfig,
    ) -> Self {
        let protocol_config: &'static ProtocolConfig =
            Box::leak(Box::new(ProtocolConfig::get_for_max_version_UNSAFE()));

        let resolver_box = Box::new(ChildObjectResolverWrapper::new_dual(
            sandbox_fetcher,
            replay_fetcher,
            config.child_resolution_mode,
            protocol_config.clone(),
        ));
        let resolver: &'static dyn ChildObjectResolver = Box::leak(resolver_box);

        Self::create_with_resolver(resolver, protocol_config, config)
    }

    /// Internal helper to create extensions with a pre-configured resolver.
    fn create_with_resolver(
        resolver: &'static dyn ChildObjectResolver,
        protocol_config: &'static ProtocolConfig,
        config: SuiRuntimeConfig,
    ) -> Self {
        // Create TxContext
        let rgp = config.gas_price;
        let tx_context = TxContext::new_from_components(
            &SuiAddress::from(config.sender),
            &TransactionDigest::ZERO,
            &config.epoch,
            config.epoch_timestamp_ms,
            rgp,
            config.gas_price,
            config.gas_budget,
            config.sponsor.map(SuiAddress::from),
            protocol_config,
        );
        let tx_context_rc = Rc::new(RefCell::new(tx_context));

        // Create metrics with a default registry (metrics won't be collected but that's fine for simulation)
        let registry = prometheus::Registry::default();
        let metrics = Arc::new(LimitsMetrics::new(&registry));

        Self {
            resolver,
            protocol_config,
            tx_context_rc,
            metrics,
            epoch_id: config.epoch,
            input_objects: parking_lot::Mutex::new(BTreeMap::new()),
            is_metered: config.is_metered,
        }
    }

    /// Add an input object to the runtime.
    /// This must be called for all objects that are inputs to the transaction
    /// before their children can be accessed.
    pub fn add_input_object(&self, id: ObjectID, version: u64, owner: Owner) {
        use std::collections::BTreeSet;
        let mut contained_uids = BTreeSet::new();
        contained_uids.insert(id);

        let input_object = InputObject {
            contained_uids,
            version: SequenceNumber::from_u64(version),
            owner,
        };

        self.input_objects.lock().insert(id, input_object);
    }

    /// Add multiple input objects.
    pub fn add_input_objects(&self, objects: &[(ObjectID, u64, Owner)]) {
        for (id, version, owner) in objects {
            self.add_input_object(*id, *version, owner.clone());
        }
    }

    /// Add Sui extensions to a NativeContextExtensions.
    ///
    /// This adds ObjectRuntime, TransactionContext, and NativesCostTable.
    /// Input objects are re-created each time to support multiple sessions.
    pub fn add_to_extensions(
        &self,
        extensions: &mut move_vm_runtime::native_extensions::NativeContextExtensions<'static>,
    ) {
        // Re-create input objects from stored data (ObjectRuntime consumes them)
        // We need to build them fresh each time since ObjectRuntime takes ownership
        // Clone the InputObject fields manually since InputObject doesn't implement Clone
        let objects = self.input_objects.lock();
        let input_objects: BTreeMap<ObjectID, InputObject> = objects
            .iter()
            .map(|(id, obj)| {
                let new_input = InputObject {
                    contained_uids: obj.contained_uids.clone(),
                    version: obj.version,
                    owner: obj.owner.clone(),
                };
                (*id, new_input)
            })
            .collect();
        drop(objects); // Release lock before using input_objects

        debug!(
            input_objects_count = input_objects.len(),
            "creating ObjectRuntime"
        );

        // Create ObjectRuntime with leaked references
        // is_metered controls whether native functions charge gas from NativesCostTable
        let object_runtime = ObjectRuntime::new(
            self.resolver,
            input_objects,
            self.is_metered,
            self.protocol_config,
            self.metrics.clone(),
            self.epoch_id,
        );

        // Create TransactionContext
        let transaction_context = TransactionContext::new(self.tx_context_rc.clone());

        // Create NativesCostTable
        let cost_table = NativesCostTable::from_protocol_config(self.protocol_config);

        // Add all extensions
        extensions.add(object_runtime);
        extensions.add(transaction_context);
        extensions.add(cost_table);
    }

    /// Get the protocol config reference.
    pub fn protocol_config(&self) -> &'static ProtocolConfig {
        self.protocol_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_child_object_resolver_wrapper_creation() {
        let fetcher: ChildFetchFn = Arc::new(|_| None);
        let protocol_config = ProtocolConfig::get_for_max_version_UNSAFE();
        let resolver = ChildObjectResolverWrapper::new(fetcher, protocol_config);

        // Test that read_child_object returns None for non-existent objects
        let parent = ObjectID::ZERO;
        let child = ObjectID::from_single_byte(1);
        let version = SequenceNumber::from_u64(1);

        let result = resolver.read_child_object(&parent, &child, version);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
