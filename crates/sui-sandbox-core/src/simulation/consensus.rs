//! Shared object locking and consensus simulation types.
//!
//! This module provides types for simulating Sui's shared object
//! consensus ordering and lock management.

use move_core_types::account_address::AccountAddress;
use std::collections::BTreeMap;

/// Shared object lock entry tracking access mode and version.
#[derive(Debug, Clone)]
pub struct SharedObjectLock {
    /// Object ID of the shared object.
    pub object_id: AccountAddress,
    /// Version at which the lock was acquired.
    pub version: u64,
    /// Whether the lock is for mutable access.
    pub is_mutable: bool,
    /// Transaction ID that holds the lock (for diagnostics).
    pub transaction_id: Option<String>,
}

/// Consensus ordering entry for tracking transaction serialization.
///
/// In Sui's consensus model, shared object transactions are serialized.
/// Each transaction has:
/// - A sequence number (global ordering)
/// - Read versions for each object it reads
/// - Write versions for each object it writes
///
/// Two transactions conflict (serialization violation) if:
/// - Tx B reads an object that Tx A writes, and B.seq > A.seq but B.read_version < A.write_version
/// - Tx B writes an object that Tx A reads, and B.seq > A.seq but B.write_version <= A.read_version
#[derive(Debug, Clone)]
pub struct ConsensusOrderEntry {
    /// Transaction sequence number (global ordering from consensus).
    pub sequence: u64,
    /// Transaction ID for diagnostics.
    pub transaction_id: String,
    /// Object ID -> version read by this transaction.
    pub read_versions: BTreeMap<AccountAddress, u64>,
    /// Object ID -> version written by this transaction.
    pub write_versions: BTreeMap<AccountAddress, u64>,
    /// Timestamp when transaction was ordered.
    pub timestamp_ms: u64,
}

/// Result of consensus validation.
#[derive(Debug, Clone)]
pub enum ConsensusValidation {
    /// Transaction can proceed - no serialization conflicts.
    Valid,
    /// Serialization conflict detected.
    SerializationConflict {
        /// The conflicting object ID.
        object_id: AccountAddress,
        /// Our transaction's intended version.
        our_version: u64,
        /// The conflicting transaction's version.
        their_version: u64,
        /// The conflicting transaction ID.
        conflicting_tx: String,
        /// Description of the conflict.
        reason: String,
    },
    /// Stale read detected (read version is behind current object version).
    StaleRead {
        /// The object with stale read.
        object_id: AccountAddress,
        /// Version we tried to read.
        read_version: u64,
        /// Current object version.
        current_version: u64,
    },
}

/// Result of attempting to acquire shared object locks.
#[derive(Debug, Clone)]
pub enum LockResult {
    /// All locks acquired successfully.
    Success {
        /// The locks that were acquired.
        locks: Vec<SharedObjectLock>,
    },
    /// Lock conflict detected.
    Conflict {
        /// The object ID that has a conflict.
        object_id: AccountAddress,
        /// The conflicting lock.
        existing_lock: SharedObjectLock,
        /// Description of the conflict.
        reason: String,
    },
}
