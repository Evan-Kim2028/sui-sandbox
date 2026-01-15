//! # State Layering for Simulation Environment
//!
//! This module provides a layered state management abstraction for the simulation
//! environment. The layering model supports:
//!
//! - **Immutable Layer**: Base state (Sui framework, system objects) that never changes
//! - **Session Layer**: Modifications that persist across transactions within a session
//! - **Transaction Layer**: Changes within a single transaction (can be committed or rolled back)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Transaction Layer                         │
//! │  (uncommitted changes, can rollback on abort)               │
//! ├─────────────────────────────────────────────────────────────┤
//! │                     Session Layer                            │
//! │  (user-created objects, published modules, session state)   │
//! ├─────────────────────────────────────────────────────────────┤
//! │                    Immutable Layer                           │
//! │  (Sui framework, Clock, Random, stdlib)                     │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! The state layering is designed to be used with the existing SimulationEnvironment
//! by wrapping state modifications in transaction contexts:
//!
//! ```ignore
//! let mut env = SimulationEnvironment::new()?;
//!
//! // Start a transaction
//! let mut tx = env.begin_transaction();
//!
//! // Make changes (these go to transaction layer)
//! tx.create_object(...)?;
//! tx.call_function(...)?;
//!
//! // Commit or rollback
//! if successful {
//!     tx.commit()?;  // Merges into session layer
//! } else {
//!     tx.rollback(); // Discards transaction layer changes
//! }
//! ```

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// =============================================================================
// State Layer Types
// =============================================================================

/// Identifies which layer state belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LayerType {
    /// Base layer - Sui framework, system objects, never changes
    Immutable,
    /// Session layer - persists across transactions
    Session,
    /// Transaction layer - uncommitted changes
    Transaction,
}

impl LayerType {
    /// Check if this layer can be modified
    pub fn is_mutable(&self) -> bool {
        !matches!(self, LayerType::Immutable)
    }

    /// Get human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            LayerType::Immutable => "immutable",
            LayerType::Session => "session",
            LayerType::Transaction => "transaction",
        }
    }
}

/// Tracks which layer an object belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectLayerInfo {
    /// Object ID
    pub object_id: AccountAddress,
    /// Which layer owns this object
    pub layer: LayerType,
    /// Version when added to this layer
    pub version: u64,
    /// Whether object was modified in current transaction
    pub modified_in_tx: bool,
}

// =============================================================================
// State Snapshot for Rollback
// =============================================================================

/// A snapshot of state that can be restored on rollback.
///
/// This captures the delta between layers, not the full state,
/// making snapshots efficient even for large object stores.
#[derive(Debug, Clone, Default)]
pub struct StateSnapshot {
    /// Objects that existed before the transaction started (for rollback)
    pub objects_before: BTreeMap<AccountAddress, Vec<u8>>,
    /// Objects created during this transaction
    pub objects_created: BTreeSet<AccountAddress>,
    /// Objects deleted during this transaction
    pub objects_deleted: BTreeSet<AccountAddress>,
    /// Dynamic fields added: (parent_id, child_id)
    pub dynamic_fields_added: BTreeSet<(AccountAddress, AccountAddress)>,
    /// Dynamic fields removed: (parent_id, child_id) -> (type, bytes)
    pub dynamic_fields_removed: BTreeMap<(AccountAddress, AccountAddress), (TypeTag, Vec<u8>)>,
    /// Counter values before transaction
    pub counters_before: SnapshotCounters,
}

/// Counter values to restore on rollback
#[derive(Debug, Clone, Default)]
pub struct SnapshotCounters {
    pub id_counter: u64,
    pub tx_counter: u64,
    pub lamport_clock: u64,
    pub consensus_sequence: u64,
}

impl StateSnapshot {
    /// Create a new empty snapshot
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that an object was created (will be deleted on rollback)
    pub fn record_create(&mut self, object_id: AccountAddress) {
        self.objects_created.insert(object_id);
    }

    /// Record that an object was modified (save original for rollback)
    pub fn record_modify(&mut self, object_id: AccountAddress, original_bytes: Vec<u8>) {
        // Only record the first modification (original state)
        self.objects_before
            .entry(object_id)
            .or_insert(original_bytes);
    }

    /// Record that an object was deleted (save for restoration on rollback)
    pub fn record_delete(&mut self, object_id: AccountAddress, original_bytes: Vec<u8>) {
        self.objects_deleted.insert(object_id);
        self.objects_before
            .entry(object_id)
            .or_insert(original_bytes);
    }

    /// Record that a dynamic field was added
    pub fn record_field_add(&mut self, parent_id: AccountAddress, child_id: AccountAddress) {
        self.dynamic_fields_added.insert((parent_id, child_id));
    }

    /// Record that a dynamic field was removed
    pub fn record_field_remove(
        &mut self,
        parent_id: AccountAddress,
        child_id: AccountAddress,
        type_tag: TypeTag,
        bytes: Vec<u8>,
    ) {
        self.dynamic_fields_removed
            .insert((parent_id, child_id), (type_tag, bytes));
    }

    /// Check if snapshot has any changes
    pub fn has_changes(&self) -> bool {
        !self.objects_created.is_empty()
            || !self.objects_deleted.is_empty()
            || !self.objects_before.is_empty()
            || !self.dynamic_fields_added.is_empty()
            || !self.dynamic_fields_removed.is_empty()
    }

    /// Get count of changes for logging
    pub fn change_count(&self) -> usize {
        self.objects_created.len()
            + self.objects_deleted.len()
            + self.objects_before.len()
            + self.dynamic_fields_added.len()
            + self.dynamic_fields_removed.len()
    }
}

// =============================================================================
// Transaction Context
// =============================================================================

/// A transaction context that tracks changes for commit/rollback.
///
/// This is designed to wrap operations on SimulationEnvironment,
/// capturing changes that can be rolled back on abort.
#[derive(Debug)]
pub struct TransactionContext {
    /// Unique ID for this transaction
    pub tx_id: String,
    /// Snapshot of state before transaction started
    snapshot: StateSnapshot,
    /// Whether transaction is still active
    active: bool,
    /// Whether transaction was committed
    committed: bool,
}

impl TransactionContext {
    /// Create a new transaction context
    pub fn new(tx_id: impl Into<String>) -> Self {
        Self {
            tx_id: tx_id.into(),
            snapshot: StateSnapshot::new(),
            active: true,
            committed: false,
        }
    }

    /// Create with pre-initialized counters for rollback
    pub fn with_counters(tx_id: impl Into<String>, counters: SnapshotCounters) -> Self {
        let mut ctx = Self::new(tx_id);
        ctx.snapshot.counters_before = counters;
        ctx
    }

    /// Check if transaction is still active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Check if transaction was committed
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// Get mutable reference to snapshot for recording changes
    pub fn snapshot_mut(&mut self) -> &mut StateSnapshot {
        &mut self.snapshot
    }

    /// Get reference to snapshot
    pub fn snapshot(&self) -> &StateSnapshot {
        &self.snapshot
    }

    /// Mark transaction as committed
    pub fn mark_committed(&mut self) {
        self.active = false;
        self.committed = true;
    }

    /// Mark transaction as rolled back
    pub fn mark_rolled_back(&mut self) {
        self.active = false;
        self.committed = false;
    }
}

// =============================================================================
// Layer Statistics
// =============================================================================

/// Statistics about state layers for debugging and monitoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayerStats {
    /// Number of objects in immutable layer
    pub immutable_objects: usize,
    /// Number of modules in immutable layer
    pub immutable_modules: usize,
    /// Number of objects in session layer
    pub session_objects: usize,
    /// Number of modules in session layer
    pub session_modules: usize,
    /// Number of objects in transaction layer
    pub transaction_objects: usize,
    /// Number of dynamic fields
    pub dynamic_fields: usize,
    /// Number of pending receives
    pub pending_receives: usize,
}

impl LayerStats {
    /// Total object count across all layers
    pub fn total_objects(&self) -> usize {
        self.immutable_objects + self.session_objects + self.transaction_objects
    }

    /// Total module count across all layers
    pub fn total_modules(&self) -> usize {
        self.immutable_modules + self.session_modules
    }
}

// =============================================================================
// Immutable Object Registry
// =============================================================================

/// Registry of well-known immutable objects.
///
/// These objects are part of the immutable layer and should never
/// be modified during simulation.
#[derive(Debug, Clone, Default)]
pub struct ImmutableObjectRegistry {
    /// Set of immutable object IDs
    object_ids: BTreeSet<AccountAddress>,
}

impl ImmutableObjectRegistry {
    /// Create a new registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with Sui system objects registered
    pub fn with_sui_system() -> Self {
        let mut registry = Self::new();

        // Clock object (0x6)
        if let Ok(addr) = AccountAddress::from_hex_literal(
            "0x0000000000000000000000000000000000000000000000000000000000000006",
        ) {
            registry.register(addr);
        }

        // Random object (0x8)
        if let Ok(addr) = AccountAddress::from_hex_literal(
            "0x0000000000000000000000000000000000000000000000000000000000000008",
        ) {
            registry.register(addr);
        }

        registry
    }

    /// Register an object as immutable
    pub fn register(&mut self, object_id: AccountAddress) {
        self.object_ids.insert(object_id);
    }

    /// Check if an object is immutable
    pub fn is_immutable(&self, object_id: &AccountAddress) -> bool {
        self.object_ids.contains(object_id)
    }

    /// Get count of registered immutable objects
    pub fn count(&self) -> usize {
        self.object_ids.len()
    }

    /// Iterate over immutable object IDs
    pub fn iter(&self) -> impl Iterator<Item = &AccountAddress> {
        self.object_ids.iter()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_type() {
        assert!(!LayerType::Immutable.is_mutable());
        assert!(LayerType::Session.is_mutable());
        assert!(LayerType::Transaction.is_mutable());
    }

    #[test]
    fn test_state_snapshot_create() {
        let mut snapshot = StateSnapshot::new();
        assert!(!snapshot.has_changes());

        let obj_id = AccountAddress::from_hex_literal("0x1").unwrap();
        snapshot.record_create(obj_id);

        assert!(snapshot.has_changes());
        assert_eq!(snapshot.change_count(), 1);
        assert!(snapshot.objects_created.contains(&obj_id));
    }

    #[test]
    fn test_state_snapshot_modify() {
        let mut snapshot = StateSnapshot::new();
        let obj_id = AccountAddress::from_hex_literal("0x1").unwrap();

        // Record first modification
        snapshot.record_modify(obj_id, vec![1, 2, 3]);
        // Record second modification (should not overwrite)
        snapshot.record_modify(obj_id, vec![4, 5, 6]);

        assert_eq!(snapshot.objects_before.get(&obj_id), Some(&vec![1, 2, 3]));
    }

    #[test]
    fn test_state_snapshot_delete() {
        let mut snapshot = StateSnapshot::new();
        let obj_id = AccountAddress::from_hex_literal("0x1").unwrap();

        snapshot.record_delete(obj_id, vec![1, 2, 3]);

        assert!(snapshot.objects_deleted.contains(&obj_id));
        assert!(snapshot.objects_before.contains_key(&obj_id));
    }

    #[test]
    fn test_state_snapshot_dynamic_fields() {
        let mut snapshot = StateSnapshot::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child = AccountAddress::from_hex_literal("0x2").unwrap();

        snapshot.record_field_add(parent, child);
        assert!(snapshot.dynamic_fields_added.contains(&(parent, child)));

        let type_tag = TypeTag::Bool;
        snapshot.record_field_remove(parent, child, type_tag.clone(), vec![1]);
        assert!(snapshot
            .dynamic_fields_removed
            .contains_key(&(parent, child)));
    }

    #[test]
    fn test_transaction_context() {
        let mut ctx = TransactionContext::new("test-tx-1");
        assert!(ctx.is_active());
        assert!(!ctx.is_committed());

        let obj_id = AccountAddress::from_hex_literal("0x1").unwrap();
        ctx.snapshot_mut().record_create(obj_id);

        assert!(ctx.snapshot().has_changes());

        ctx.mark_committed();
        assert!(!ctx.is_active());
        assert!(ctx.is_committed());
    }

    #[test]
    fn test_transaction_context_rollback() {
        let mut ctx = TransactionContext::new("test-tx-2");

        ctx.mark_rolled_back();
        assert!(!ctx.is_active());
        assert!(!ctx.is_committed());
    }

    #[test]
    fn test_immutable_registry() {
        let mut registry = ImmutableObjectRegistry::new();
        assert_eq!(registry.count(), 0);

        let obj_id = AccountAddress::from_hex_literal("0x6").unwrap();
        registry.register(obj_id);

        assert!(registry.is_immutable(&obj_id));
        assert!(!registry.is_immutable(&AccountAddress::from_hex_literal("0x123").unwrap()));
    }

    #[test]
    fn test_immutable_registry_with_sui_system() {
        let registry = ImmutableObjectRegistry::with_sui_system();

        // Clock (0x6)
        let clock = AccountAddress::from_hex_literal(
            "0x0000000000000000000000000000000000000000000000000000000000000006",
        )
        .unwrap();
        assert!(registry.is_immutable(&clock));

        // Random (0x8)
        let random = AccountAddress::from_hex_literal(
            "0x0000000000000000000000000000000000000000000000000000000000000008",
        )
        .unwrap();
        assert!(registry.is_immutable(&random));
    }

    #[test]
    fn test_layer_stats() {
        let stats = LayerStats {
            immutable_objects: 5,
            immutable_modules: 10,
            session_objects: 3,
            session_modules: 2,
            transaction_objects: 1,
            dynamic_fields: 4,
            pending_receives: 0,
        };

        assert_eq!(stats.total_objects(), 9);
        assert_eq!(stats.total_modules(), 12);
    }

    #[test]
    fn test_snapshot_counters() {
        let counters = SnapshotCounters {
            id_counter: 100,
            tx_counter: 50,
            lamport_clock: 25,
            consensus_sequence: 10,
        };

        let ctx = TransactionContext::with_counters("tx-1", counters);
        assert_eq!(ctx.snapshot().counters_before.id_counter, 100);
        assert_eq!(ctx.snapshot().counters_before.tx_counter, 50);
    }
}
