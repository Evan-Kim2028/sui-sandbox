//! ObjectRuntime - VM extension for dynamic field simulation and object storage.
//!
//! This module provides an in-memory object runtime that integrates with the Move VM
//! via its extension mechanism. It enables:
//! - Full dynamic field support (add, borrow, remove operations)
//! - Object storage with ownership tracking
//! - Shared object support
//! - Object receiving (send-to-object pattern)
//!
//! ## What is a VM Extension?
//!
//! The Move VM has a mechanism for native functions to access external state that:
//! - Persists across multiple native function calls within a session
//! - Can be accessed via `context.extensions().get::<T>()` or `get_mut::<T>()`
//! - Is registered when creating a session via `new_session_with_extensions()`
//!
//! This is how Sui's actual runtime provides object storage to natives - without it,
//! native functions can only work with their arguments and return values.
//!
//! ## Why We Need This for Dynamic Fields
//!
//! Dynamic field operations like `borrow_child_object` must return a **reference**
//! to a value managed by the VM. The challenge:
//!
//! 1. Native functions are stateless - they receive args and return results
//! 2. Returning a reference requires the value to live somewhere stable
//! 3. The VM needs to track references for borrow checking
//!
//! The solution: `GlobalValue` from move-vm-types wraps a value and provides
//! reference semantics via `borrow_global()`. We store GlobalValues in this
//! extension, so they persist for the session duration.
//!
//! ## Object Store
//!
//! In addition to dynamic fields, this module provides a general object store that:
//! - Tracks all objects created during execution
//! - Records ownership (address-owned, shared, immutable)
//! - Supports object receiving via `pending_receives`
//! - Can mark objects as deleted
//!
//! ## Implementation
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │ VMHarness::execute_function()                                    │
//! │   │                                                              │
//! │   ├─► Create ObjectRuntime extension                             │
//! │   ├─► vm.new_session_with_extensions(storage, extensions)        │
//! │   │                                                              │
//! │   │   ┌──────────────────────────────────────────────────────┐   │
//! │   │   │ Native: add_child_object                             │   │
//! │   │   │   ctx.extensions_mut().get_mut::<ObjectRuntime>()?   │   │
//! │   │   │   runtime.add_child_object(parent, id, value, type)  │   │
//! │   │   │   └─► Wraps value in GlobalValue and stores it       │   │
//! │   │   └──────────────────────────────────────────────────────┘   │
//! │   │                                                              │
//! │   │   ┌──────────────────────────────────────────────────────┐   │
//! │   │   │ Native: borrow_child_object                          │   │
//! │   │   │   ctx.extensions().get::<ObjectRuntime>()?           │   │
//! │   │   │   runtime.borrow_child_object(parent, id, type)      │   │
//! │   │   │   └─► Returns GlobalValue.borrow_global() as ref     │   │
//! │   │   └──────────────────────────────────────────────────────┘   │
//! │   │                                                              │
//! │   └─► session.finish() - ObjectRuntime is dropped                │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Dependencies
//!
//! - `better_any = "0.1"` - Required for VM extension mechanism
//!   IMPORTANT: Version must match move-vm-runtime's dependency

use better_any::{Tid, TidAble};
use fastcrypto::hash::{Blake2b256, HashFunction};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use move_vm_runtime::native_extensions::NativeExtensionMarker;
use move_vm_types::values::{GlobalValue, Value};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::trace;

/// Callback type for on-demand child object fetching by computed ID.
/// Takes (parent_id, child_id) and returns Option<(type_tag, bcs_bytes)>.
/// This is called when a child object is requested but not found in the preloaded set.
pub type ChildFetcherFn =
    Box<dyn Fn(AccountAddress, AccountAddress) -> Option<(TypeTag, Vec<u8>)> + Send + Sync>;

/// Callback type for on-demand child object fetching with version info.
/// Takes (parent_id, child_id) and returns Option<(type_tag, bcs_bytes, version)>.
/// Use this for transaction replay to ensure correct version information.
pub type VersionedChildFetcherFn =
    Box<dyn Fn(AccountAddress, AccountAddress) -> Option<(TypeTag, Vec<u8>, u64)> + Send + Sync>;

/// Callback type for key-based child object fetching.
/// Takes (parent_id, child_id, key_type_tag, key_bcs_bytes) and returns Option<(type_tag, bcs_bytes)>.
/// This is called when ID-based lookup fails, allowing lookup by dynamic field key content.
/// This handles cases where package upgrades cause computed child IDs to differ from stored IDs.
///
/// For dynamic object fields (where key_type is `Wrapper<K>`), the returned type should be
/// `Field<Wrapper<K>, ID>` and the BCS should encode the Field wrapper containing an ID
/// reference to the actual object.
pub type KeyBasedChildFetcherFn = Box<
    dyn Fn(AccountAddress, AccountAddress, &TypeTag, &[u8]) -> Option<(TypeTag, Vec<u8>)>
        + Send
        + Sync,
>;

/// Error codes matching Sui's dynamic_field module
pub const E_FIELD_ALREADY_EXISTS: u64 = 0;
pub const E_FIELD_DOES_NOT_EXIST: u64 = 1;
pub const E_FIELD_TYPE_MISMATCH: u64 = 2;

/// Error codes for object operations
pub const E_OBJECT_NOT_FOUND: u64 = 100;
pub const E_OBJECT_ALREADY_EXISTS: u64 = 101;
pub const E_NOT_OWNER: u64 = 102;
pub const E_OBJECT_DELETED: u64 = 103;
pub const E_RECEIVE_NOT_FOUND: u64 = 104;

/// Unique identifier for objects.
pub type ObjectID = AccountAddress;

/// Ownership status of an object.
///
/// This mirrors Sui's `Owner` enum for compatibility with digest computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Owner {
    /// Owned by a specific address
    Address(AccountAddress),
    /// Shared object (can be accessed by anyone)
    Shared,
    /// Immutable (frozen, cannot be modified)
    Immutable,
    /// Object owned by another object (wrapped)
    Object(ObjectID),
}

impl Default for Owner {
    fn default() -> Self {
        Owner::Address(AccountAddress::ZERO)
    }
}

/// A 32-byte object digest (Blake2b256 hash).
/// Matches Sui's ObjectDigest type.
pub type ObjectDigest = [u8; 32];

/// A 32-byte transaction digest.
/// Matches Sui's TransactionDigest type.
pub type TransactionDigest = [u8; 32];

/// Special marker digest values matching Sui's constants.
pub mod digest_markers {
    use super::ObjectDigest;

    /// Marker for deleted objects (all bytes = 99)
    pub const OBJECT_DIGEST_DELETED: ObjectDigest = [99u8; 32];

    /// Marker for wrapped objects (all bytes = 88)
    pub const OBJECT_DIGEST_WRAPPED: ObjectDigest = [88u8; 32];

    /// Marker for cancelled objects (all bytes = 77)
    pub const OBJECT_DIGEST_CANCELLED: ObjectDigest = [77u8; 32];

    /// Zero digest (for genesis/placeholder)
    pub const OBJECT_DIGEST_ZERO: ObjectDigest = [0u8; 32];
}

/// A stored object in the object store.
///
/// This structure closely mirrors Sui's `ObjectInner` to enable accurate
/// digest computation and mainnet-compatible state representation.
///
/// ## Digest Computation
///
/// The object digest is computed as:
/// ```text
/// Blake2b256("ObjectInner::" || BCS(ObjectInnerForDigest { data, owner, previous_transaction, storage_rebate }))
/// ```
///
/// This matches Sui's `default_hash` function which uses the `BcsSignable` trait.
#[derive(Debug, Clone)]
pub struct StoredObject {
    /// BCS-serialized bytes of the Move object contents
    pub bytes: Vec<u8>,
    /// Type tag of the stored object (e.g., `0x2::coin::Coin<0x2::sui::SUI>`)
    pub type_tag: TypeTag,
    /// Owner of the object
    pub owner: Owner,
    /// Version number (lamport timestamp, incremented on mutation)
    pub version: u64,
    /// Whether the object has been deleted
    pub deleted: bool,
    // ========== NEW FIELDS FOR MAINNET FIDELITY ==========
    /// Object digest - Blake2b256 hash of the serialized object.
    /// Computed lazily and cached. Use `compute_digest()` to get.
    digest: Option<ObjectDigest>,
    /// The digest of the transaction that created or last mutated this object.
    /// This is `None` for objects created in the sandbox (not from mainnet).
    pub previous_transaction: Option<TransactionDigest>,
    /// The version at which this object was first shared (if it's a shared object).
    /// This is immutable once set and is used for shared object consensus checks.
    pub initial_shared_version: Option<u64>,
    /// Storage rebate in MIST - the amount refunded if this object is deleted.
    /// Set to 0 for sandbox-created objects (actual rebate requires gas price oracle).
    pub storage_rebate: u64,
    /// Whether this object has `public_transfer` ability (can be transferred freely).
    /// Determined by the type's abilities.
    pub has_public_transfer: bool,
}

impl StoredObject {
    /// Create a new stored object with default values for new fields.
    ///
    /// For shared objects, `initial_shared_version` is set to the object's version (1),
    /// matching Sui's behavior where shared objects track the version at which they
    /// became shared.
    pub fn new(bytes: Vec<u8>, type_tag: TypeTag, owner: Owner) -> Self {
        Self::new_at_version(bytes, type_tag, owner, 1)
    }

    /// Create a new stored object at a specific version.
    ///
    /// For shared objects, `initial_shared_version` is set to the provided version,
    /// which should be the version at which the object was shared. This is critical
    /// for shared object consensus validation.
    pub fn new_at_version(bytes: Vec<u8>, type_tag: TypeTag, owner: Owner, version: u64) -> Self {
        Self::new_with_storage_rebate(bytes, type_tag, owner, version, 0)
    }

    /// Create a new stored object with a specific storage rebate.
    ///
    /// Storage rebate is the amount of MIST refunded when this object is deleted.
    /// In Sui, this is calculated as: `object_size_bytes * storage_price_per_byte * 0.99`
    /// (99% is refundable, 1% is permanently burned).
    ///
    /// For convenience, use `calculate_storage_rebate()` to compute the rebate from
    /// object size and storage price.
    pub fn new_with_storage_rebate(
        bytes: Vec<u8>,
        type_tag: TypeTag,
        owner: Owner,
        version: u64,
        storage_rebate: u64,
    ) -> Self {
        let initial_shared_version = if matches!(owner, Owner::Shared) {
            Some(version) // Version at which object became shared
        } else {
            None
        };

        Self {
            bytes,
            type_tag,
            owner,
            version,
            deleted: false,
            digest: None, // Computed lazily
            previous_transaction: None,
            initial_shared_version,
            storage_rebate,
            has_public_transfer: true, // Assume transferable by default
        }
    }

    /// Calculate the storage rebate for an object of the given size.
    ///
    /// In Sui, storage costs are:
    /// - Storage units = object_size_bytes * 100 (each byte = 100 storage units)
    /// - Storage fee = storage_units * storage_price_per_unit
    /// - Storage rebate = storage_fee * 0.99 (99% refundable)
    ///
    /// The default storage price is 76 MIST per storage unit (as of mainnet epoch ~500+).
    /// This means: rebate ≈ object_size * 100 * 76 * 0.99 = object_size * 7524
    pub fn calculate_storage_rebate(object_size_bytes: usize, storage_price_per_unit: u64) -> u64 {
        let storage_units = (object_size_bytes as u64) * 100;
        let storage_fee = storage_units * storage_price_per_unit;
        // 99% is refundable
        storage_fee * 99 / 100
    }

    /// Calculate and set the storage rebate based on object size.
    ///
    /// Uses the default storage price of 76 MIST per storage unit.
    pub fn with_calculated_rebate(mut self, storage_price_per_unit: u64) -> Self {
        self.storage_rebate = Self::calculate_storage_rebate(self.bytes.len(), storage_price_per_unit);
        self
    }

    /// Create a stored object with full mainnet-compatible metadata.
    ///
    /// Use this when replaying mainnet transactions where you have complete
    /// object metadata from the chain.
    pub fn new_with_metadata(
        bytes: Vec<u8>,
        type_tag: TypeTag,
        owner: Owner,
        version: u64,
        previous_transaction: Option<TransactionDigest>,
        initial_shared_version: Option<u64>,
        storage_rebate: u64,
        has_public_transfer: bool,
    ) -> Self {
        Self {
            bytes,
            type_tag,
            owner,
            version,
            deleted: false,
            digest: None,
            previous_transaction,
            initial_shared_version,
            storage_rebate,
            has_public_transfer,
        }
    }

    /// Create a stored object from fetched mainnet data.
    ///
    /// This is a convenience constructor for use with `FetchedObjectData`.
    pub fn from_fetched(
        bytes: Vec<u8>,
        type_tag: TypeTag,
        owner: Owner,
        version: u64,
        digest: Option<ObjectDigest>,
        previous_transaction: Option<TransactionDigest>,
    ) -> Self {
        let initial_shared_version = if matches!(owner, Owner::Shared) {
            Some(version) // Assume current version is initial for fetched shared objects
        } else {
            None
        };

        Self {
            bytes,
            type_tag,
            owner,
            version,
            deleted: false,
            digest,
            previous_transaction,
            initial_shared_version,
            storage_rebate: 0,
            has_public_transfer: true,
        }
    }

    /// Mark this object as deleted.
    pub fn mark_deleted(&mut self) {
        self.deleted = true;
        // Clear computed digest - deleted objects have special marker digest
        self.digest = Some(digest_markers::OBJECT_DIGEST_DELETED);
    }

    /// Increment the version (called on mutation).
    pub fn increment_version(&mut self) {
        self.version += 1;
        // Invalidate cached digest since version changed
        self.digest = None;
    }

    /// Set the previous transaction digest (call after mutation).
    pub fn set_previous_transaction(&mut self, tx_digest: TransactionDigest) {
        self.previous_transaction = Some(tx_digest);
        // Invalidate cached digest since previous_transaction is part of digest computation
        self.digest = None;
    }

    /// Get the object digest, computing it if necessary.
    ///
    /// The digest is computed as:
    /// ```text
    /// Blake2b256("ObjectInner::" || BCS(ObjectInnerForDigest))
    /// ```
    ///
    /// This matches Sui's `default_hash` function for `ObjectInner`.
    pub fn digest(&mut self) -> ObjectDigest {
        if let Some(digest) = self.digest {
            return digest;
        }

        // Deleted objects have a special marker digest
        if self.deleted {
            self.digest = Some(digest_markers::OBJECT_DIGEST_DELETED);
            return digest_markers::OBJECT_DIGEST_DELETED;
        }

        let computed = self.compute_digest();
        self.digest = Some(computed);
        computed
    }

    /// Get the cached digest without computing (returns None if not yet computed).
    pub fn cached_digest(&self) -> Option<ObjectDigest> {
        self.digest
    }

    /// Force recomputation of the digest (e.g., after manual bytes modification).
    pub fn invalidate_digest(&mut self) {
        self.digest = None;
    }

    /// Compute the object digest following Sui's algorithm.
    ///
    /// ## Algorithm
    ///
    /// Sui computes object digests using `default_hash` which:
    /// 1. Prepends the type name: "ObjectInner::"
    /// 2. Appends BCS-serialized ObjectInner struct
    /// 3. Hashes with Blake2b256
    ///
    /// The ObjectInner struct contains: { data, owner, previous_transaction, storage_rebate }
    fn compute_digest(&self) -> ObjectDigest {
        // Build the data to hash following Sui's format
        let digest_data = ObjectInnerForDigest {
            // MoveObject contains: type_, has_public_transfer, version, contents
            type_tag: self.type_tag.clone(),
            has_public_transfer: self.has_public_transfer,
            version: self.version,
            contents: self.bytes.clone(),
            owner: self.owner,
            previous_transaction: self.previous_transaction.unwrap_or([0u8; 32]),
            storage_rebate: self.storage_rebate,
        };

        // Hash following Sui's BcsSignable pattern: "TypeName::" || BCS(data)
        let mut hasher = Blake2b256::default();

        // Write the type name prefix (matches serde_name::trace_name behavior)
        hasher.update(b"ObjectInner::");

        // BCS serialize and append
        // Note: This is a simplified serialization. For full compatibility,
        // we'd need to match Sui's exact ObjectInner BCS layout.
        let bcs_bytes = match bcs::to_bytes(&digest_data) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "BCS serialization failed during digest computation, using empty bytes"
                );
                Vec::new()
            }
        };
        hasher.update(&bcs_bytes);

        let hash = hasher.finalize();
        hash.into()
    }

    /// Get the object reference tuple (id, version, digest).
    ///
    /// Note: This requires the object ID which is not stored in StoredObject.
    /// The caller must provide the ID.
    pub fn object_ref(&mut self, id: ObjectID) -> (ObjectID, u64, ObjectDigest) {
        (id, self.version, self.digest())
    }

    /// Check if this object is alive (not deleted or wrapped).
    pub fn is_alive(&self) -> bool {
        !self.deleted
    }

    /// Get the initial shared version (if this is/was a shared object).
    pub fn initial_shared_version(&self) -> Option<u64> {
        self.initial_shared_version
    }

    /// Mark this object as shared at the current version.
    ///
    /// If the object is already shared, this does nothing.
    /// The initial_shared_version is immutable once set.
    pub fn mark_shared(&mut self) {
        if self.initial_shared_version.is_none() {
            self.initial_shared_version = Some(self.version);
        }
        self.owner = Owner::Shared;
        self.digest = None; // Invalidate digest since owner changed
    }
}

/// Internal struct for digest computation.
/// This mirrors Sui's ObjectInner structure for BCS serialization.
#[derive(Serialize, Deserialize)]
struct ObjectInnerForDigest {
    // MoveObject fields
    type_tag: TypeTag,
    has_public_transfer: bool,
    version: u64,
    contents: Vec<u8>,
    // ObjectInner fields
    owner: Owner,
    previous_transaction: [u8; 32],
    storage_rebate: u64,
}

/// Object store for tracking all objects created during execution.
///
/// This provides a general-purpose object store separate from the dynamic
/// field child objects. It tracks:
/// - All objects by their ObjectID
/// - Ownership information
/// - Version numbers
/// - Deleted status
/// - Pending receives (for object-to-object transfers)
#[derive(Debug, Default)]
pub struct ObjectStore {
    /// All stored objects by ID
    objects: HashMap<ObjectID, StoredObject>,
    /// Set of shared object IDs (for quick lookup)
    shared: HashSet<ObjectID>,
    /// Pending receives: (recipient_object_id, sender_object_id) -> object bytes
    /// Used for transfer::receive pattern
    pending_receives: HashMap<(ObjectID, ObjectID), Vec<u8>>,
}

impl ObjectStore {
    /// Create a new empty object store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a newly created object.
    pub fn record_created(
        &mut self,
        id: ObjectID,
        bytes: Vec<u8>,
        type_tag: TypeTag,
        owner: Owner,
    ) -> Result<(), u64> {
        if self.objects.contains_key(&id) {
            return Err(E_OBJECT_ALREADY_EXISTS);
        }

        let obj = StoredObject::new(bytes, type_tag, owner);

        // Track if shared
        if matches!(owner, Owner::Shared) {
            self.shared.insert(id);
        }

        self.objects.insert(id, obj);
        Ok(())
    }

    /// Get an object by ID.
    pub fn get(&self, id: &ObjectID) -> Option<&StoredObject> {
        self.objects.get(id).filter(|obj| !obj.deleted)
    }

    /// Get a mutable reference to an object by ID.
    pub fn get_mut(&mut self, id: &ObjectID) -> Option<&mut StoredObject> {
        self.objects.get_mut(id).filter(|obj| !obj.deleted)
    }

    /// Check if an object exists (and is not deleted).
    pub fn exists(&self, id: &ObjectID) -> bool {
        self.objects
            .get(id)
            .map(|obj| !obj.deleted)
            .unwrap_or(false)
    }

    /// Check if an object is shared.
    pub fn is_shared(&self, id: &ObjectID) -> bool {
        self.shared.contains(id)
    }

    /// Mark an object as shared.
    ///
    /// This sets the `initial_shared_version` if not already set, which is
    /// immutable once assigned (matching Sui's shared object semantics).
    pub fn mark_shared(&mut self, id: ObjectID) -> Result<(), u64> {
        let obj = self.objects.get_mut(&id).ok_or(E_OBJECT_NOT_FOUND)?;
        if obj.deleted {
            return Err(E_OBJECT_DELETED);
        }
        obj.mark_shared(); // Uses the new method that handles initial_shared_version
        self.shared.insert(id);
        Ok(())
    }

    /// Mark an object as immutable (frozen).
    pub fn mark_immutable(&mut self, id: ObjectID) -> Result<(), u64> {
        let obj = self.objects.get_mut(&id).ok_or(E_OBJECT_NOT_FOUND)?;
        if obj.deleted {
            return Err(E_OBJECT_DELETED);
        }
        obj.owner = Owner::Immutable;
        obj.invalidate_digest(); // Owner change invalidates digest
        Ok(())
    }

    /// Delete an object.
    pub fn delete(&mut self, id: &ObjectID) -> Result<StoredObject, u64> {
        let obj = self.objects.get_mut(id).ok_or(E_OBJECT_NOT_FOUND)?;
        if obj.deleted {
            return Err(E_OBJECT_DELETED);
        }
        obj.mark_deleted();
        self.shared.remove(id);

        // Return a clone of the deleted object
        Ok(obj.clone())
    }

    /// Transfer ownership of an object to a new owner.
    pub fn transfer(&mut self, id: &ObjectID, new_owner: Owner) -> Result<(), u64> {
        let obj = self.objects.get_mut(id).ok_or(E_OBJECT_NOT_FOUND)?;
        if obj.deleted {
            return Err(E_OBJECT_DELETED);
        }

        // Update shared set if ownership type changes
        let was_shared = matches!(obj.owner, Owner::Shared);
        let is_shared = matches!(new_owner, Owner::Shared);

        if was_shared && !is_shared {
            self.shared.remove(id);
        } else if !was_shared && is_shared {
            self.shared.insert(*id);
            // Set initial_shared_version when becoming shared
            if obj.initial_shared_version.is_none() {
                obj.initial_shared_version = Some(obj.version);
            }
        }

        obj.owner = new_owner;
        obj.increment_version(); // This also invalidates the digest
        Ok(())
    }

    /// Update object bytes (mutation).
    pub fn update_bytes(&mut self, id: &ObjectID, new_bytes: Vec<u8>) -> Result<(), u64> {
        let obj = self.objects.get_mut(id).ok_or(E_OBJECT_NOT_FOUND)?;
        if obj.deleted {
            return Err(E_OBJECT_DELETED);
        }
        obj.bytes = new_bytes;
        obj.invalidate_digest(); // Bytes change invalidates digest
        obj.increment_version(); // This also invalidates the digest
        Ok(())
    }

    /// Record a newly created object with full metadata.
    ///
    /// Use this when replaying mainnet transactions where you have complete
    /// object metadata including digest and previous transaction.
    pub fn record_created_with_metadata(
        &mut self,
        id: ObjectID,
        bytes: Vec<u8>,
        type_tag: TypeTag,
        owner: Owner,
        version: u64,
        digest: Option<ObjectDigest>,
        previous_transaction: Option<TransactionDigest>,
    ) -> Result<(), u64> {
        if self.objects.contains_key(&id) {
            return Err(E_OBJECT_ALREADY_EXISTS);
        }

        let obj = StoredObject::from_fetched(
            bytes,
            type_tag,
            owner,
            version,
            digest,
            previous_transaction,
        );

        // Track if shared
        if matches!(owner, Owner::Shared) {
            self.shared.insert(id);
        }

        self.objects.insert(id, obj);
        Ok(())
    }

    /// Get an object's digest, computing it if necessary.
    pub fn get_digest(&mut self, id: &ObjectID) -> Option<ObjectDigest> {
        self.objects.get_mut(id).map(|obj| obj.digest())
    }

    /// Get an object reference (id, version, digest).
    pub fn get_object_ref(&mut self, id: &ObjectID) -> Option<(ObjectID, u64, ObjectDigest)> {
        self.objects
            .get_mut(id)
            .filter(|obj| !obj.deleted)
            .map(|obj| (*id, obj.version, obj.digest()))
    }

    // ========== Receiving Objects ==========

    /// Send an object to another object (stage for receiving).
    ///
    /// This is used for the `transfer::receive` pattern where an object
    /// is sent to another object and can later be received.
    pub fn send_to_object(
        &mut self,
        recipient_id: ObjectID,
        object_id: ObjectID,
    ) -> Result<(), u64> {
        let obj = self.objects.get_mut(&object_id).ok_or(E_OBJECT_NOT_FOUND)?;
        if obj.deleted {
            return Err(E_OBJECT_DELETED);
        }

        // Store the object bytes in pending receives
        let bytes = obj.bytes.clone();
        self.pending_receives
            .insert((recipient_id, object_id), bytes);

        // Update ownership to indicate it's owned by the recipient object
        obj.owner = Owner::Object(recipient_id);
        obj.increment_version();

        Ok(())
    }

    /// Receive an object that was sent to this object.
    ///
    /// Returns the object bytes if found.
    pub fn receive_object(
        &mut self,
        recipient_id: ObjectID,
        object_id: ObjectID,
    ) -> Result<Vec<u8>, u64> {
        let key = (recipient_id, object_id);
        let bytes = self
            .pending_receives
            .remove(&key)
            .ok_or(E_RECEIVE_NOT_FOUND)?;

        // The object is no longer pending, update its ownership back to address
        // (the caller will handle the actual ownership based on what they do with it)
        if let Some(obj) = self.objects.get_mut(&object_id) {
            // Reset ownership - the receiving function determines final owner
            obj.owner = Owner::Address(AccountAddress::ZERO);
            obj.increment_version();
        }

        Ok(bytes)
    }

    /// Check if an object is pending receive at a recipient.
    pub fn has_pending_receive(&self, recipient_id: ObjectID, object_id: ObjectID) -> bool {
        self.pending_receives
            .contains_key(&(recipient_id, object_id))
    }

    // ========== Statistics ==========

    /// Get the number of objects (including deleted).
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Get the number of active (non-deleted) objects.
    pub fn active_count(&self) -> usize {
        self.objects.values().filter(|obj| !obj.deleted).count()
    }

    /// Get the number of shared objects.
    pub fn shared_count(&self) -> usize {
        self.shared.len()
    }

    /// Get all object IDs.
    pub fn object_ids(&self) -> impl Iterator<Item = &ObjectID> {
        self.objects.keys()
    }

    /// Get all active object IDs.
    pub fn active_object_ids(&self) -> impl Iterator<Item = &ObjectID> {
        self.objects
            .iter()
            .filter(|(_, obj)| !obj.deleted)
            .map(|(id, _)| id)
    }

    /// Clear all objects (for test isolation).
    pub fn clear(&mut self) {
        self.objects.clear();
        self.shared.clear();
        self.pending_receives.clear();
    }
}

/// A child object stored in the runtime (for dynamic fields).
pub struct ChildObject {
    /// The GlobalValue wrapping the Move value (enables reference semantics)
    pub value: GlobalValue,
    /// Type tag for type checking
    pub type_tag: TypeTag,
}

/// In-memory object runtime for dynamic field simulation and object storage.
///
/// This is registered as a VM extension and provides:
/// - Storage for GlobalValues that can be borrowed as references by native functions
/// - A general object store for tracking all objects created during execution
/// - Support for object receiving (send-to-object pattern)
#[derive(Tid, Default)]
pub struct ObjectRuntime {
    /// Map from (parent_id, child_id) -> child object (for dynamic fields)
    /// Using the same addressing scheme as Sui's dynamic fields
    children: HashMap<(AccountAddress, AccountAddress), ChildObject>,

    /// General object store for tracking all objects
    object_store: ObjectStore,
}

// Mark as a native extension so it can be registered with the VM
impl NativeExtensionMarker<'_> for ObjectRuntime {}

impl ObjectRuntime {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
            object_store: ObjectStore::new(),
        }
    }

    /// Get a reference to the object store.
    pub fn object_store(&self) -> &ObjectStore {
        &self.object_store
    }

    /// Get a mutable reference to the object store.
    pub fn object_store_mut(&mut self) -> &mut ObjectStore {
        &mut self.object_store
    }

    /// Add a child object under a parent.
    ///
    /// Returns error if object already exists at this location.
    pub fn add_child_object(
        &mut self,
        parent: AccountAddress,
        child_id: AccountAddress,
        value: Value,
        type_tag: TypeTag,
    ) -> Result<(), u64> {
        let key = (parent, child_id);

        if self.children.contains_key(&key) {
            return Err(E_FIELD_ALREADY_EXISTS);
        }

        // Use GlobalValue::none() + move_to() instead of GlobalValue::cached()
        // This creates a "Fresh" GlobalValue which returns ContainerRef::Local on borrow,
        // avoiding a field indexing bug in ContainerRef::Global's borrow_elem.
        let mut global_value = GlobalValue::none();
        global_value
            .move_to(value)
            .map_err(|_| E_FIELD_TYPE_MISMATCH)?;

        self.children.insert(
            key,
            ChildObject {
                value: global_value,
                type_tag,
            },
        );

        Ok(())
    }

    /// Check if a child object exists.
    pub fn child_object_exists(&self, parent: AccountAddress, child_id: AccountAddress) -> bool {
        self.children.contains_key(&(parent, child_id))
    }

    /// Check if a child object exists with a specific type.
    pub fn child_object_exists_with_type(
        &self,
        parent: AccountAddress,
        child_id: AccountAddress,
        expected_type: &TypeTag,
    ) -> bool {
        match self.children.get(&(parent, child_id)) {
            Some(child) => &child.type_tag == expected_type,
            None => false,
        }
    }

    /// Borrow a child object (immutable reference).
    ///
    /// Returns the Value wrapped as a reference that can be returned from a native.
    pub fn borrow_child_object(
        &self,
        parent: AccountAddress,
        child_id: AccountAddress,
        expected_type: &TypeTag,
    ) -> Result<Value, u64> {
        let child = self
            .children
            .get(&(parent, child_id))
            .ok_or(E_FIELD_DOES_NOT_EXIST)?;

        // Type check
        if &child.type_tag != expected_type {
            return Err(E_FIELD_TYPE_MISMATCH);
        }

        // borrow_global returns a Value that is internally a reference
        child
            .value
            .borrow_global()
            .map_err(|_| E_FIELD_DOES_NOT_EXIST)
    }

    /// Borrow a child object mutably.
    ///
    /// Note: In our simulation, this is the same as immutable borrow since
    /// we don't persist mutations anyway. But it satisfies the type system.
    pub fn borrow_child_object_mut(
        &mut self,
        parent: AccountAddress,
        child_id: AccountAddress,
        expected_type: &TypeTag,
    ) -> Result<Value, u64> {
        // For mut borrow, we still use borrow_global which returns a mutable-compatible ref
        // The GlobalValue tracks mutation status internally
        let child = self
            .children
            .get(&(parent, child_id))
            .ok_or(E_FIELD_DOES_NOT_EXIST)?;

        if &child.type_tag != expected_type {
            return Err(E_FIELD_TYPE_MISMATCH);
        }

        child
            .value
            .borrow_global()
            .map_err(|_| E_FIELD_DOES_NOT_EXIST)
    }

    /// Remove a child object and return the owned value.
    pub fn remove_child_object(
        &mut self,
        parent: AccountAddress,
        child_id: AccountAddress,
        expected_type: &TypeTag,
    ) -> Result<Value, u64> {
        let key = (parent, child_id);

        // Check type before removing
        {
            let child = self.children.get(&key).ok_or(E_FIELD_DOES_NOT_EXIST)?;
            if &child.type_tag != expected_type {
                return Err(E_FIELD_TYPE_MISMATCH);
            }
        }

        // Remove and extract value
        let child = self.children.remove(&key).ok_or(E_FIELD_DOES_NOT_EXIST)?;

        // move_from extracts the owned value from the GlobalValue
        child.value.into_value().ok_or(E_FIELD_DOES_NOT_EXIST)
    }

    /// Clear all stored objects (for test isolation).
    pub fn clear(&mut self) {
        self.children.clear();
        self.object_store.clear();
    }

    /// Get the number of stored child objects (dynamic fields).
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Get the number of dynamic field children.
    pub fn children_len(&self) -> usize {
        self.children.len()
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Get an iterator over all child object keys and their types.
    /// Used to extract dynamic field state for TransactionEffects.
    pub fn iter_children(
        &self,
    ) -> impl Iterator<Item = (&(AccountAddress, AccountAddress), &TypeTag)> {
        self.children.iter().map(|(k, v)| (k, &v.type_tag))
    }

    /// Get all child keys (parent_id, child_id pairs).
    pub fn child_keys(&self) -> Vec<(AccountAddress, AccountAddress)> {
        self.children.keys().cloned().collect()
    }

    /// Count the number of children for a specific parent (dynamic fields count).
    pub fn count_children_for_parent(&self, parent: AccountAddress) -> u64 {
        self.children.keys().filter(|(p, _)| *p == parent).count() as u64
    }

    /// List all child IDs for a specific parent.
    pub fn list_child_ids(&self, parent: AccountAddress) -> Vec<AccountAddress> {
        self.children
            .keys()
            .filter_map(|(p, c)| if *p == parent { Some(*c) } else { None })
            .collect()
    }
}

/// Inner state for SharedObjectRuntime that can be shared via Arc<Mutex>.
/// This holds the actual dynamic field data that persists across VM sessions.
#[derive(Default)]
pub struct ObjectRuntimeState {
    /// Map from (parent_id, child_id) -> (type_tag, serialized_bytes)
    /// We store serialized bytes instead of GlobalValue because GlobalValue
    /// cannot be safely shared across threads.
    pub children: HashMap<(AccountAddress, AccountAddress), (TypeTag, Vec<u8>)>,
    /// Set of children that existed before this PTB started (loaded from env)
    pub preloaded_children: HashSet<(AccountAddress, AccountAddress)>,
    /// Pending receives: (recipient_object_id, sent_object_id) -> (type_tag, bytes)
    /// Used for transfer::receive pattern where an object was sent to another object.
    pub pending_receives: HashMap<(AccountAddress, AccountAddress), (TypeTag, Vec<u8>)>,
    /// Set of children that have been removed during this PTB execution.
    /// This prevents on-demand fetching from re-creating them.
    pub removed_children: HashSet<(AccountAddress, AccountAddress)>,
}

impl ObjectRuntimeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a child object to the shared state.
    pub fn add_child(
        &mut self,
        parent: AccountAddress,
        child_id: AccountAddress,
        type_tag: TypeTag,
        bytes: Vec<u8>,
    ) {
        self.children.insert((parent, child_id), (type_tag, bytes));
    }

    /// Check if a child exists.
    pub fn has_child(&self, parent: AccountAddress, child_id: AccountAddress) -> bool {
        self.children.contains_key(&(parent, child_id))
    }

    /// Get a child's bytes.
    pub fn get_child(
        &self,
        parent: AccountAddress,
        child_id: AccountAddress,
    ) -> Option<&(TypeTag, Vec<u8>)> {
        self.children.get(&(parent, child_id))
    }

    /// Remove a child and return its data.
    /// Also tracks the removal to prevent on-demand re-fetching.
    pub fn remove_child(
        &mut self,
        parent: AccountAddress,
        child_id: AccountAddress,
    ) -> Option<(TypeTag, Vec<u8>)> {
        let result = self.children.remove(&(parent, child_id));
        if result.is_some() {
            // Track this child as removed to prevent re-fetching
            self.removed_children.insert((parent, child_id));
        }
        result
    }

    /// Check if a child has been removed during this PTB execution.
    pub fn is_child_removed(&self, parent: AccountAddress, child_id: AccountAddress) -> bool {
        self.removed_children.contains(&(parent, child_id))
    }

    /// Get all newly created children (not in preloaded set).
    pub fn new_children(&self) -> Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)> {
        self.children
            .iter()
            .filter(|(k, _)| !self.preloaded_children.contains(k))
            .map(|(k, (t, b))| (*k, t.clone(), b.clone()))
            .collect()
    }

    /// Get all children (both preloaded and new).
    pub fn all_children(&self) -> Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)> {
        self.children
            .iter()
            .map(|(k, (t, b))| (*k, t.clone(), b.clone()))
            .collect()
    }

    /// Clear all state.
    pub fn clear(&mut self) {
        self.children.clear();
        self.preloaded_children.clear();
        self.pending_receives.clear();
        self.removed_children.clear();
    }

    // ========== Pending Receives ==========

    /// Add a pending receive for an object sent to another object.
    /// This is used for transfer::receive pattern.
    pub fn add_pending_receive(
        &mut self,
        recipient_id: AccountAddress,
        sent_id: AccountAddress,
        type_tag: TypeTag,
        bytes: Vec<u8>,
    ) {
        self.pending_receives
            .insert((recipient_id, sent_id), (type_tag, bytes));
    }

    /// Try to receive an object that was sent to a recipient.
    /// Returns the object bytes and type if found, removes from pending.
    pub fn receive_pending(
        &mut self,
        recipient_id: AccountAddress,
        sent_id: AccountAddress,
    ) -> Option<(TypeTag, Vec<u8>)> {
        self.pending_receives.remove(&(recipient_id, sent_id))
    }

    /// Check if an object is pending receive at a recipient.
    pub fn has_pending_receive(
        &self,
        recipient_id: AccountAddress,
        sent_id: AccountAddress,
    ) -> bool {
        self.pending_receives.contains_key(&(recipient_id, sent_id))
    }

    /// Get all pending receives for a specific recipient.
    pub fn get_pending_receives_for(
        &self,
        recipient_id: AccountAddress,
    ) -> Vec<(AccountAddress, &TypeTag, &Vec<u8>)> {
        self.pending_receives
            .iter()
            .filter(|((r, _), _)| *r == recipient_id)
            .map(|((_, s), (t, b))| (*s, t, b))
            .collect()
    }

    /// Count the number of children for a specific parent.
    pub fn count_children_for_parent(&self, parent: AccountAddress) -> u64 {
        self.children.keys().filter(|(p, _)| *p == parent).count() as u64
    }

    /// List all child IDs for a specific parent.
    pub fn list_child_ids(&self, parent: AccountAddress) -> Vec<AccountAddress> {
        self.children
            .keys()
            .filter_map(|(p, c)| if *p == parent { Some(*c) } else { None })
            .collect()
    }
}

/// Information about how a child_id was computed from parent + key.
/// Used for key-based fallback lookup when hash doesn't match stored data.
#[derive(Debug, Clone)]
pub struct ComputedChildInfo {
    /// Parent object ID
    pub parent_id: AccountAddress,
    /// Type tag of the key (rewritten to runtime addresses)
    pub key_type: TypeTag,
    /// BCS-serialized key bytes
    pub key_bytes: Vec<u8>,
}

/// A shareable ObjectRuntime that persists state across VM sessions.
///
/// This wraps ObjectRuntimeState in Arc<Mutex> and provides a VM extension
/// that synchronizes with the shared state before and after each call.
///
/// ## Usage
///
/// See `PTBSession` in `vm.rs` for complete usage patterns with shared object state.
#[derive(Tid)]
pub struct SharedObjectRuntime {
    /// The shared state - this is cloned Arc so multiple runtimes can share
    shared_state: Arc<Mutex<ObjectRuntimeState>>,
    /// Local ObjectRuntime for this session - initialized from shared state
    /// and synced back after execution
    local: ObjectRuntime,
    /// Optional callback for on-demand child fetching from network/archive.
    /// Called when a child object is requested but not found in shared state.
    child_fetcher: Option<Arc<ChildFetcherFn>>,
    /// Optional callback for key-based child fetching.
    /// Called when ID-based lookup fails, allowing lookup by dynamic field key.
    key_based_child_fetcher: Option<Arc<KeyBasedChildFetcherFn>>,
    /// Track all child object IDs that were accessed during execution (for tracing).
    /// This is used to discover which children need to be fetched for historical replay.
    accessed_children: Arc<Mutex<HashSet<AccountAddress>>>,
    /// Address aliases for package upgrades (storage_id -> original_id).
    /// Used for module resolution: when looking for modules at storage address, find at original.
    address_aliases: HashMap<AccountAddress, AccountAddress>,
    /// Reverse address aliases (original_id -> storage_id).
    /// Used for dynamic field hash computation: when bytecode uses original_id,
    /// but children were created with storage_id after an upgrade.
    reverse_address_aliases: HashMap<AccountAddress, AccountAddress>,
    /// Mapping from computed child_id -> (parent, key_type, key_bytes).
    /// Populated during hash_type_and_key calls, used for key-based fallback lookup.
    computed_child_keys: Mutex<HashMap<AccountAddress, ComputedChildInfo>>,
}

// Mark as a native extension
impl NativeExtensionMarker<'_> for SharedObjectRuntime {}

impl SharedObjectRuntime {
    /// Create a new SharedObjectRuntime with the given shared state.
    /// Loads any existing children from the shared state into the local runtime.
    pub fn new(shared_state: Arc<Mutex<ObjectRuntimeState>>) -> Self {
        let local = ObjectRuntime::new();
        // Note: We don't load children here because GlobalValue requires struct values
        // which we can't easily reconstruct from bytes. The native functions will
        // check the shared state if the local runtime doesn't have the child.
        Self {
            shared_state,
            local,
            child_fetcher: None,
            key_based_child_fetcher: None,
            accessed_children: Arc::new(Mutex::new(HashSet::new())),
            address_aliases: HashMap::new(),
            reverse_address_aliases: HashMap::new(),
            computed_child_keys: Mutex::new(HashMap::new()),
        }
    }

    /// Create a SharedObjectRuntime with an on-demand child fetcher.
    /// The fetcher is called when a child object is not found in the preloaded set.
    pub fn with_child_fetcher(
        shared_state: Arc<Mutex<ObjectRuntimeState>>,
        fetcher: ChildFetcherFn,
    ) -> Self {
        Self {
            shared_state,
            local: ObjectRuntime::new(),
            child_fetcher: Some(Arc::new(fetcher)),
            key_based_child_fetcher: None,
            accessed_children: Arc::new(Mutex::new(HashSet::new())),
            address_aliases: HashMap::new(),
            reverse_address_aliases: HashMap::new(),
            computed_child_keys: Mutex::new(HashMap::new()),
        }
    }

    /// Create a SharedObjectRuntime with shared access tracking.
    /// The accessed_children Arc will be updated whenever a child is accessed.
    pub fn with_access_tracking(
        shared_state: Arc<Mutex<ObjectRuntimeState>>,
        accessed_children: Arc<Mutex<HashSet<AccountAddress>>>,
    ) -> Self {
        Self {
            shared_state,
            local: ObjectRuntime::new(),
            child_fetcher: None,
            key_based_child_fetcher: None,
            accessed_children,
            address_aliases: HashMap::new(),
            reverse_address_aliases: HashMap::new(),
            computed_child_keys: Mutex::new(HashMap::new()),
        }
    }

    /// Set the child fetcher callback (ID-based lookup).
    pub fn set_child_fetcher(&mut self, fetcher: ChildFetcherFn) {
        self.child_fetcher = Some(Arc::new(fetcher));
    }

    /// Set the key-based child fetcher callback.
    /// This is called when ID-based lookup fails, allowing lookup by dynamic field key.
    pub fn set_key_based_child_fetcher(&mut self, fetcher: KeyBasedChildFetcherFn) {
        self.key_based_child_fetcher = Some(Arc::new(fetcher));
    }

    /// Set address aliases for package upgrades.
    /// The input is storage_id -> original_id (for module resolution).
    /// We also build the reverse mapping original_id -> storage_id (for dynamic field hashing).
    ///
    /// Note: When multiple storage_ids map to the same original_id (package upgrades),
    /// we keep only the lexicographically largest storage_id (which tends to be the latest).
    /// For better accuracy, use `set_address_aliases_with_versions` with version hints.
    pub fn set_address_aliases(&mut self, aliases: HashMap<AccountAddress, AccountAddress>) {
        // Call the version-aware method without version hints (will use address comparison)
        self.set_address_aliases_with_versions(aliases, HashMap::new());
    }

    /// Set address aliases with version hints for accurate reverse mapping.
    ///
    /// The `aliases` map is storage_id -> original_id (for module resolution).
    /// The `versions` map is storage_id (hex string) -> version number.
    ///
    /// When multiple storage addresses map to the same original, we use version hints
    /// to pick the highest-versioned storage address. This is essential for upgraded
    /// packages where dynamic field children were created with the latest storage address.
    pub fn set_address_aliases_with_versions(
        &mut self,
        aliases: HashMap<AccountAddress, AccountAddress>,
        versions: HashMap<String, u64>,
    ) {
        // Build reverse mapping: original_id -> (storage_id, version)
        // This is needed for dynamic field hash computation when children were created
        // after a package upgrade (using storage_id in their types).
        let mut reverse_with_version: HashMap<AccountAddress, (AccountAddress, u64)> = HashMap::new();

        for (&storage, &original) in &aliases {
            // Look up version for this storage address
            let storage_hex = storage.to_hex_literal();
            let storage_normalized = crate::utilities::normalize_address(&storage_hex);

            // Try both normalized and original forms
            let version = versions.get(&storage_normalized)
                .or_else(|| versions.get(&storage_hex))
                .or_else(|| versions.get(&format!("0x{}", storage_normalized)))
                .copied()
                .unwrap_or(0);

            reverse_with_version.entry(original)
                .and_modify(|(existing_storage, existing_version)| {
                    // Prefer higher version; fall back to address comparison if versions equal
                    if version > *existing_version
                        || (version == *existing_version && storage > *existing_storage)
                    {
                        *existing_storage = storage;
                        *existing_version = version;
                    }
                })
                .or_insert((storage, version));
        }

        // Convert to simple reverse map (dropping version info)
        let reverse: HashMap<AccountAddress, AccountAddress> = reverse_with_version
            .into_iter()
            .map(|(original, (storage, _))| (original, storage))
            .collect();

        // Debug output for reverse aliases
        if !reverse.is_empty() {
            trace!(
                count = reverse.len(),
                "set_address_aliases_with_versions: built reverse aliases (original -> storage)"
            );
            for (original, storage) in &reverse {
                trace!(
                    original = %original.to_hex_literal(),
                    storage = %storage.to_hex_literal(),
                    "reverse alias"
                );
            }
        }

        self.reverse_address_aliases = reverse;
        self.address_aliases = aliases;
    }

    /// Rewrite a TypeTag to use storage addresses for dynamic field hash computation.
    ///
    /// For upgraded packages, dynamic field children may have been created with either:
    /// 1. **Original address** (pre-upgrade): bytecode address matches, no rewrite needed
    /// 2. **Storage address** (post-upgrade): need to rewrite original -> storage
    ///
    /// We try the REVERSE lookup (original -> storage) because:
    /// - Bytecode uses original_id (0xefe8b36d...)
    /// - Post-upgrade children were created with storage_id (0xd384ded6...)
    /// - Hash must use storage_id to match stored children
    ///
    /// Note: This may cause lookup failures for pre-upgrade children. The fallback
    /// mechanism (key-based lookup) should handle mismatches by trying all variants.
    pub fn rewrite_type_tag(&self, tag: TypeTag) -> TypeTag {
        match tag {
            TypeTag::Struct(s) => {
                let mut s = *s;
                let original_addr = s.address;

                // Try REVERSE lookup: original -> storage
                // This handles post-upgrade dynamic fields where children were created
                // with the current storage address, not the original bytecode address.
                if let Some(&storage_addr) = self.reverse_address_aliases.get(&s.address) {
                    s.address = storage_addr;
                }

                // Log if we rewrote the address (for debugging)
                if s.address != original_addr {
                    trace!(
                        original = %original_addr.to_hex_literal(),
                        rewritten = %s.address.to_hex_literal(),
                        module = %s.module,
                        name = %s.name,
                        "rewrite_type_tag"
                    );
                }

                s.type_params = s
                    .type_params
                    .into_iter()
                    .map(|t| self.rewrite_type_tag(t))
                    .collect();
                TypeTag::Struct(Box::new(s))
            }
            TypeTag::Vector(inner) => TypeTag::Vector(Box::new(self.rewrite_type_tag(*inner))),
            other => other,
        }
    }

    /// Get the set of child object IDs that were accessed during execution.
    /// This is useful for tracing which children need to be fetched for replay.
    pub fn get_accessed_children(&self) -> Vec<AccountAddress> {
        self.accessed_children.lock().iter().cloned().collect()
    }

    /// Record that a child was accessed (for tracing).
    pub fn record_child_access(&self, child_id: AccountAddress) {
        self.accessed_children.lock().insert(child_id);
    }

    /// Get the Arc for accessed children (to share with child fetcher).
    pub fn accessed_children_arc(&self) -> Arc<Mutex<HashSet<AccountAddress>>> {
        self.accessed_children.clone()
    }

    /// Get the child fetcher if set.
    pub fn child_fetcher(&self) -> Option<&Arc<ChildFetcherFn>> {
        self.child_fetcher.as_ref()
    }

    /// Get the key-based child fetcher if set.
    pub fn key_based_child_fetcher(&self) -> Option<&Arc<KeyBasedChildFetcherFn>> {
        self.key_based_child_fetcher.as_ref()
    }

    /// Get access to the shared state for external queries.
    pub fn shared_state(&self) -> &Arc<Mutex<ObjectRuntimeState>> {
        &self.shared_state
    }

    /// Get mutable access to the local runtime (for native function implementations).
    pub fn local_mut(&mut self) -> &mut ObjectRuntime {
        &mut self.local
    }

    /// Get reference to the local runtime.
    pub fn local(&self) -> &ObjectRuntime {
        &self.local
    }

    /// Check if a child exists (in local or shared state).
    pub fn child_exists(&self, parent: AccountAddress, child_id: AccountAddress) -> bool {
        if self.local.child_object_exists(parent, child_id) {
            return true;
        }
        self.shared_state.lock().has_child(parent, child_id)
    }

    /// Record the computed child key info for later key-based fallback lookup.
    /// Called from hash_type_and_key native function.
    pub fn record_computed_child(
        &self,
        child_id: AccountAddress,
        parent_id: AccountAddress,
        key_type: TypeTag,
        key_bytes: Vec<u8>,
    ) {
        self.computed_child_keys.lock().insert(
            child_id,
            ComputedChildInfo {
                parent_id,
                key_type,
                key_bytes,
            },
        );
    }

    /// Get the computed child info for a child_id if available.
    pub fn get_computed_child_info(&self, child_id: &AccountAddress) -> Option<ComputedChildInfo> {
        self.computed_child_keys.lock().get(child_id).cloned()
    }

    /// Try to fetch a child object on-demand if a fetcher is configured.
    /// Returns Some((type_tag, bytes)) if successfully fetched, None otherwise.
    /// Also records the child access for tracing purposes.
    pub fn try_fetch_child(
        &self,
        parent_id: AccountAddress,
        child_id: AccountAddress,
    ) -> Option<(TypeTag, Vec<u8>)> {
        // Always record the access for tracing
        self.record_child_access(child_id);

        // First try ID-based fetcher
        if let Some(fetcher) = &self.child_fetcher {
            if let Some(result) = fetcher(parent_id, child_id) {
                return Some(result);
            }
        }

        // If ID-based lookup failed, try key-based lookup
        if let Some(key_fetcher) = &self.key_based_child_fetcher {
            if let Some(info) = self.get_computed_child_info(&child_id) {
                if let Some(result) =
                    key_fetcher(info.parent_id, child_id, &info.key_type, &info.key_bytes)
                {
                    return Some(result);
                }
            }
        }

        None
    }
}

impl Default for SharedObjectRuntime {
    fn default() -> Self {
        Self {
            shared_state: Arc::new(Mutex::new(ObjectRuntimeState::new())),
            local: ObjectRuntime::new(),
            child_fetcher: None,
            key_based_child_fetcher: None,
            accessed_children: Arc::new(Mutex::new(HashSet::new())),
            address_aliases: HashMap::new(),
            reverse_address_aliases: HashMap::new(),
            computed_child_keys: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use move_vm_types::values::Value;

    /// Create a struct Value for testing (GlobalValue::cached requires struct values)
    fn make_test_struct() -> Value {
        // Create a simple struct with one u64 field
        Value::struct_(move_vm_types::values::Struct::pack(vec![Value::u64(42)]))
    }

    fn make_test_type_tag() -> TypeTag {
        // Use a struct type tag that matches our test struct
        TypeTag::Struct(Box::new(move_core_types::language_storage::StructTag {
            address: AccountAddress::ONE,
            module: move_core_types::identifier::Identifier::new("test").unwrap(),
            name: move_core_types::identifier::Identifier::new("TestStruct").unwrap(),
            type_params: vec![],
        }))
    }

    #[test]
    fn test_add_and_exists() {
        let mut runtime = ObjectRuntime::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child_id = AccountAddress::from_hex_literal("0x2").unwrap();
        let value = make_test_struct();
        let type_tag = make_test_type_tag();

        assert!(!runtime.child_object_exists(parent, child_id));

        runtime
            .add_child_object(parent, child_id, value, type_tag.clone())
            .unwrap();

        assert!(runtime.child_object_exists(parent, child_id));
        assert!(runtime.child_object_exists_with_type(parent, child_id, &type_tag));
        assert!(!runtime.child_object_exists_with_type(parent, child_id, &TypeTag::Bool));
    }

    #[test]
    fn test_add_duplicate_fails() {
        let mut runtime = ObjectRuntime::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child_id = AccountAddress::from_hex_literal("0x2").unwrap();
        let type_tag = make_test_type_tag();

        runtime
            .add_child_object(parent, child_id, make_test_struct(), type_tag.clone())
            .unwrap();

        let result = runtime.add_child_object(parent, child_id, make_test_struct(), type_tag);
        assert!(matches!(result, Err(e) if e == E_FIELD_ALREADY_EXISTS));
    }

    #[test]
    fn test_borrow_nonexistent_fails() {
        let runtime = ObjectRuntime::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child_id = AccountAddress::from_hex_literal("0x2").unwrap();

        let result = runtime.borrow_child_object(parent, child_id, &make_test_type_tag());
        assert!(matches!(result, Err(e) if e == E_FIELD_DOES_NOT_EXIST));
    }

    #[test]
    fn test_remove() {
        let mut runtime = ObjectRuntime::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child_id = AccountAddress::from_hex_literal("0x2").unwrap();
        let type_tag = make_test_type_tag();

        runtime
            .add_child_object(parent, child_id, make_test_struct(), type_tag.clone())
            .unwrap();
        assert!(runtime.child_object_exists(parent, child_id));

        let _removed = runtime
            .remove_child_object(parent, child_id, &type_tag)
            .unwrap();
        assert!(!runtime.child_object_exists(parent, child_id));

        // Second remove should fail
        let result = runtime.remove_child_object(parent, child_id, &type_tag);
        assert!(matches!(result, Err(e) if e == E_FIELD_DOES_NOT_EXIST));
    }

    // ========== ObjectStore tests ==========

    #[test]
    fn test_object_store_create_and_get() {
        let mut store = ObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let bytes = vec![1, 2, 3, 4];
        let type_tag = make_test_type_tag();
        let owner = Owner::Address(AccountAddress::from_hex_literal("0x1").unwrap());

        // Create object
        store
            .record_created(id, bytes.clone(), type_tag.clone(), owner)
            .unwrap();

        // Verify it exists
        assert!(store.exists(&id));
        assert_eq!(store.active_count(), 1);

        // Get the object
        let obj = store.get(&id).unwrap();
        assert_eq!(obj.bytes, bytes);
        assert_eq!(obj.type_tag, type_tag);
        assert_eq!(obj.version, 1);
        assert!(!obj.deleted);
    }

    #[test]
    fn test_object_store_duplicate_fails() {
        let mut store = ObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let type_tag = make_test_type_tag();
        let owner = Owner::Address(AccountAddress::ZERO);

        store
            .record_created(id, vec![1], type_tag.clone(), owner)
            .unwrap();

        // Second create should fail
        let result = store.record_created(id, vec![2], type_tag, owner);
        assert!(matches!(result, Err(e) if e == E_OBJECT_ALREADY_EXISTS));
    }

    #[test]
    fn test_object_store_shared() {
        let mut store = ObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let type_tag = make_test_type_tag();

        // Create as owned
        store
            .record_created(id, vec![1], type_tag, Owner::Address(AccountAddress::ZERO))
            .unwrap();
        assert!(!store.is_shared(&id));

        // Mark as shared
        store.mark_shared(id).unwrap();
        assert!(store.is_shared(&id));
        assert_eq!(store.shared_count(), 1);

        // Verify owner changed
        let obj = store.get(&id).unwrap();
        assert!(matches!(obj.owner, Owner::Shared));
    }

    #[test]
    fn test_object_store_delete() {
        let mut store = ObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let type_tag = make_test_type_tag();

        store
            .record_created(
                id,
                vec![1, 2, 3],
                type_tag,
                Owner::Address(AccountAddress::ZERO),
            )
            .unwrap();
        assert!(store.exists(&id));
        assert_eq!(store.active_count(), 1);

        // Delete
        let deleted = store.delete(&id).unwrap();
        assert!(deleted.deleted);

        // No longer exists (logically)
        assert!(!store.exists(&id));
        assert_eq!(store.active_count(), 0);

        // Second delete should fail
        let result = store.delete(&id);
        assert!(matches!(result, Err(e) if e == E_OBJECT_DELETED));
    }

    #[test]
    fn test_object_store_transfer() {
        let mut store = ObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let alice = AccountAddress::from_hex_literal("0xA11CE").unwrap();
        let bob = AccountAddress::from_hex_literal("0xB0B").unwrap();
        let type_tag = make_test_type_tag();

        store
            .record_created(id, vec![1], type_tag, Owner::Address(alice))
            .unwrap();

        // Transfer to Bob
        store.transfer(&id, Owner::Address(bob)).unwrap();

        let obj = store.get(&id).unwrap();
        assert!(matches!(obj.owner, Owner::Address(addr) if addr == bob));
        assert_eq!(obj.version, 2); // Version incremented
    }

    #[test]
    fn test_object_store_send_and_receive() {
        let mut store = ObjectStore::new();
        let recipient_id = AccountAddress::from_hex_literal("0x100").unwrap();
        let object_id = AccountAddress::from_hex_literal("0x200").unwrap();
        let type_tag = make_test_type_tag();

        // Create the recipient object
        store
            .record_created(
                recipient_id,
                vec![1],
                type_tag.clone(),
                Owner::Address(AccountAddress::ZERO),
            )
            .unwrap();

        // Create the object to send
        store
            .record_created(
                object_id,
                vec![42, 43, 44],
                type_tag,
                Owner::Address(AccountAddress::ZERO),
            )
            .unwrap();

        // Send object to recipient
        store.send_to_object(recipient_id, object_id).unwrap();

        // Verify pending receive
        assert!(store.has_pending_receive(recipient_id, object_id));

        // Verify ownership changed
        let obj = store.get(&object_id).unwrap();
        assert!(matches!(obj.owner, Owner::Object(id) if id == recipient_id));

        // Receive the object
        let bytes = store.receive_object(recipient_id, object_id).unwrap();
        assert_eq!(bytes, vec![42, 43, 44]);

        // No longer pending
        assert!(!store.has_pending_receive(recipient_id, object_id));
    }

    #[test]
    fn test_object_store_receive_not_found() {
        let mut store = ObjectStore::new();
        let recipient_id = AccountAddress::from_hex_literal("0x100").unwrap();
        let object_id = AccountAddress::from_hex_literal("0x200").unwrap();

        let result = store.receive_object(recipient_id, object_id);
        assert!(matches!(result, Err(e) if e == E_RECEIVE_NOT_FOUND));
    }

    #[test]
    fn test_runtime_includes_object_store() {
        let mut runtime = ObjectRuntime::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let type_tag = make_test_type_tag();

        // Access object store through runtime
        runtime
            .object_store_mut()
            .record_created(
                id,
                vec![1, 2, 3],
                type_tag,
                Owner::Address(AccountAddress::ZERO),
            )
            .unwrap();

        assert!(runtime.object_store().exists(&id));
        assert_eq!(runtime.object_store().active_count(), 1);

        // Clear should clear both children and object store
        runtime.clear();
        assert!(!runtime.object_store().exists(&id));
        assert_eq!(runtime.object_store().active_count(), 0);
    }

    // ========== New State Fidelity Tests ==========

    #[test]
    fn test_object_digest_computation() {
        let type_tag = make_test_type_tag();
        let mut obj = StoredObject::new(
            vec![1, 2, 3, 4],
            type_tag,
            Owner::Address(AccountAddress::ZERO),
        );

        // Digest should be computed lazily
        assert!(obj.cached_digest().is_none());

        // Get digest (computes it)
        let digest1 = obj.digest();
        assert!(obj.cached_digest().is_some());
        assert_eq!(digest1.len(), 32);

        // Same object should produce same digest
        let digest2 = obj.digest();
        assert_eq!(digest1, digest2);
    }

    #[test]
    fn test_digest_invalidation_on_mutation() {
        let type_tag = make_test_type_tag();
        let mut obj = StoredObject::new(
            vec![1, 2, 3],
            type_tag,
            Owner::Address(AccountAddress::ZERO),
        );

        let digest1 = obj.digest();

        // Version increment should invalidate digest
        obj.increment_version();
        assert!(obj.cached_digest().is_none());

        let digest2 = obj.digest();
        assert_ne!(digest1, digest2); // Different version = different digest
    }

    #[test]
    fn test_digest_invalidation_on_bytes_change() {
        let type_tag = make_test_type_tag();
        let mut obj = StoredObject::new(
            vec![1, 2, 3],
            type_tag,
            Owner::Address(AccountAddress::ZERO),
        );

        let digest1 = obj.digest();

        // Change bytes
        obj.bytes = vec![4, 5, 6];
        obj.invalidate_digest();

        let digest2 = obj.digest();
        assert_ne!(digest1, digest2); // Different bytes = different digest
    }

    #[test]
    fn test_deleted_object_has_marker_digest() {
        let type_tag = make_test_type_tag();
        let mut obj = StoredObject::new(
            vec![1, 2, 3],
            type_tag,
            Owner::Address(AccountAddress::ZERO),
        );

        obj.mark_deleted();

        let digest = obj.digest();
        assert_eq!(digest, digest_markers::OBJECT_DIGEST_DELETED);
    }

    #[test]
    fn test_initial_shared_version_tracking() {
        let type_tag = make_test_type_tag();

        // Object created as shared should have initial_shared_version = 1
        let obj_shared = StoredObject::new(vec![1], type_tag.clone(), Owner::Shared);
        assert_eq!(obj_shared.initial_shared_version, Some(1));

        // Object created as owned should have no initial_shared_version
        let mut obj_owned =
            StoredObject::new(vec![1], type_tag, Owner::Address(AccountAddress::ZERO));
        assert_eq!(obj_owned.initial_shared_version, None);

        // When marked shared, should record current version as initial
        obj_owned.version = 5;
        obj_owned.mark_shared();
        assert_eq!(obj_owned.initial_shared_version, Some(5));
        assert!(matches!(obj_owned.owner, Owner::Shared));

        // Marking shared again should NOT change initial_shared_version
        obj_owned.version = 10;
        obj_owned.mark_shared();
        assert_eq!(obj_owned.initial_shared_version, Some(5)); // Still 5
    }

    #[test]
    fn test_previous_transaction_tracking() {
        let type_tag = make_test_type_tag();
        let mut obj = StoredObject::new(vec![1], type_tag, Owner::Address(AccountAddress::ZERO));

        // Initially None
        assert!(obj.previous_transaction.is_none());

        // Set previous transaction
        let tx_digest = [42u8; 32];
        obj.set_previous_transaction(tx_digest);
        assert_eq!(obj.previous_transaction, Some(tx_digest));

        // Digest should be invalidated
        assert!(obj.cached_digest().is_none());
    }

    #[test]
    fn test_object_store_initial_shared_version() {
        let mut store = ObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let type_tag = make_test_type_tag();

        // Create as owned
        store
            .record_created(id, vec![1], type_tag, Owner::Address(AccountAddress::ZERO))
            .unwrap();

        // Initial shared version should be None
        let obj = store.get(&id).unwrap();
        assert!(obj.initial_shared_version.is_none());

        // Mark as shared
        store.mark_shared(id).unwrap();

        // Now should have initial_shared_version = 1
        let obj = store.get(&id).unwrap();
        assert_eq!(obj.initial_shared_version, Some(1));
    }

    #[test]
    fn test_object_store_transfer_to_shared() {
        let mut store = ObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let type_tag = make_test_type_tag();

        store
            .record_created(id, vec![1], type_tag, Owner::Address(AccountAddress::ZERO))
            .unwrap();

        // Bump version a few times
        store.update_bytes(&id, vec![2]).unwrap(); // v2
        store.update_bytes(&id, vec![3]).unwrap(); // v3

        // Transfer to shared
        store.transfer(&id, Owner::Shared).unwrap(); // v4

        let obj = store.get(&id).unwrap();
        assert_eq!(obj.version, 4);
        assert_eq!(obj.initial_shared_version, Some(3)); // Set before increment
        assert!(matches!(obj.owner, Owner::Shared));
    }

    #[test]
    fn test_object_ref_computation() {
        let mut store = ObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x100").unwrap();
        let type_tag = make_test_type_tag();

        store
            .record_created(
                id,
                vec![1, 2, 3],
                type_tag,
                Owner::Address(AccountAddress::ZERO),
            )
            .unwrap();

        let obj_ref = store.get_object_ref(&id).unwrap();
        assert_eq!(obj_ref.0, id);
        assert_eq!(obj_ref.1, 1); // version
        assert_eq!(obj_ref.2.len(), 32); // digest is 32 bytes
    }

    #[test]
    fn test_new_with_metadata() {
        let type_tag = make_test_type_tag();
        let prev_tx = [1u8; 32];

        let obj = StoredObject::new_with_metadata(
            vec![1, 2, 3],
            type_tag,
            Owner::Shared,
            5,             // version
            Some(prev_tx), // previous_transaction
            Some(3),       // initial_shared_version
            1000,          // storage_rebate
            true,          // has_public_transfer
        );

        assert_eq!(obj.version, 5);
        assert_eq!(obj.previous_transaction, Some(prev_tx));
        assert_eq!(obj.initial_shared_version, Some(3));
        assert_eq!(obj.storage_rebate, 1000);
        assert!(obj.has_public_transfer);
    }

    #[test]
    fn test_from_fetched() {
        let type_tag = make_test_type_tag();
        let digest = [99u8; 32];
        let prev_tx = [1u8; 32];

        let obj = StoredObject::from_fetched(
            vec![1, 2, 3],
            type_tag,
            Owner::Shared,
            5,             // version
            Some(digest),  // digest
            Some(prev_tx), // previous_transaction
        );

        assert_eq!(obj.version, 5);
        assert_eq!(obj.cached_digest(), Some(digest)); // Pre-set digest
        assert_eq!(obj.previous_transaction, Some(prev_tx));
        assert_eq!(obj.initial_shared_version, Some(5)); // Assumes current version for shared
    }
}
