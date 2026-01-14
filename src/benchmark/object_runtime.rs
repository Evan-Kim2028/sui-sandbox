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
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use move_vm_runtime::native_extensions::NativeExtensionMarker;
use move_vm_types::values::{GlobalValue, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A stored object in the object store.
#[derive(Debug)]
pub struct StoredObject {
    /// BCS-serialized bytes of the object
    pub bytes: Vec<u8>,
    /// Type tag of the stored object
    pub type_tag: TypeTag,
    /// Owner of the object
    pub owner: Owner,
    /// Version number (incremented on mutation)
    pub version: u64,
    /// Whether the object has been deleted
    pub deleted: bool,
}

impl StoredObject {
    /// Create a new stored object.
    pub fn new(bytes: Vec<u8>, type_tag: TypeTag, owner: Owner) -> Self {
        Self {
            bytes,
            type_tag,
            owner,
            version: 1,
            deleted: false,
        }
    }

    /// Mark this object as deleted.
    pub fn mark_deleted(&mut self) {
        self.deleted = true;
    }

    /// Increment the version (called on mutation).
    pub fn increment_version(&mut self) {
        self.version += 1;
    }
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
    pub fn mark_shared(&mut self, id: ObjectID) -> Result<(), u64> {
        let obj = self.objects.get_mut(&id).ok_or(E_OBJECT_NOT_FOUND)?;
        if obj.deleted {
            return Err(E_OBJECT_DELETED);
        }
        obj.owner = Owner::Shared;
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
        Ok(StoredObject {
            bytes: obj.bytes.clone(),
            type_tag: obj.type_tag.clone(),
            owner: obj.owner,
            version: obj.version,
            deleted: true,
        })
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
        }

        obj.owner = new_owner;
        obj.increment_version();
        Ok(())
    }

    /// Update object bytes (mutation).
    pub fn update_bytes(&mut self, id: &ObjectID, new_bytes: Vec<u8>) -> Result<(), u64> {
        let obj = self.objects.get_mut(id).ok_or(E_OBJECT_NOT_FOUND)?;
        if obj.deleted {
            return Err(E_OBJECT_DELETED);
        }
        obj.bytes = new_bytes;
        obj.increment_version();
        Ok(())
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

        // Wrap in GlobalValue for reference semantics
        let global_value = GlobalValue::cached(value).map_err(|_| E_FIELD_TYPE_MISMATCH)?;

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
    pub fn iter_children(&self) -> impl Iterator<Item = (&(AccountAddress, AccountAddress), &TypeTag)> {
        self.children.iter().map(|(k, v)| (k, &v.type_tag))
    }

    /// Get all child keys (parent_id, child_id pairs).
    pub fn child_keys(&self) -> Vec<(AccountAddress, AccountAddress)> {
        self.children.keys().cloned().collect()
    }

    /// Count the number of children for a specific parent (dynamic fields count).
    pub fn count_children_for_parent(&self, parent: AccountAddress) -> u64 {
        self.children.keys()
            .filter(|(p, _)| *p == parent)
            .count() as u64
    }

    /// List all child IDs for a specific parent.
    pub fn list_child_ids(&self, parent: AccountAddress) -> Vec<AccountAddress> {
        self.children.keys()
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
}

impl ObjectRuntimeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a child object to the shared state.
    pub fn add_child(&mut self, parent: AccountAddress, child_id: AccountAddress, type_tag: TypeTag, bytes: Vec<u8>) {
        self.children.insert((parent, child_id), (type_tag, bytes));
    }

    /// Check if a child exists.
    pub fn has_child(&self, parent: AccountAddress, child_id: AccountAddress) -> bool {
        self.children.contains_key(&(parent, child_id))
    }

    /// Get a child's bytes.
    pub fn get_child(&self, parent: AccountAddress, child_id: AccountAddress) -> Option<&(TypeTag, Vec<u8>)> {
        self.children.get(&(parent, child_id))
    }

    /// Remove a child and return its data.
    pub fn remove_child(&mut self, parent: AccountAddress, child_id: AccountAddress) -> Option<(TypeTag, Vec<u8>)> {
        self.children.remove(&(parent, child_id))
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
    }

    /// Count the number of children for a specific parent.
    pub fn count_children_for_parent(&self, parent: AccountAddress) -> u64 {
        self.children.keys()
            .filter(|(p, _)| *p == parent)
            .count() as u64
    }

    /// List all child IDs for a specific parent.
    pub fn list_child_ids(&self, parent: AccountAddress) -> Vec<AccountAddress> {
        self.children.keys()
            .filter_map(|(p, c)| if *p == parent { Some(*c) } else { None })
            .collect()
    }
}

/// A shareable ObjectRuntime that persists state across VM sessions.
///
/// This wraps ObjectRuntimeState in Arc<Mutex> and provides a VM extension
/// that synchronizes with the shared state before and after each call.
///
/// ## Usage
///
/// ```ignore
/// // Create shared state
/// let shared_state = Arc::new(Mutex::new(ObjectRuntimeState::new()));
///
/// // For each VM call, create a SharedObjectRuntime extension
/// let runtime = SharedObjectRuntime::new(shared_state.clone());
/// extensions.add(runtime);
///
/// // After session.finish(), the shared_state will have been updated
/// // with any new child objects created during execution.
/// ```
#[derive(Tid)]
pub struct SharedObjectRuntime {
    /// The shared state - this is cloned Arc so multiple runtimes can share
    shared_state: Arc<Mutex<ObjectRuntimeState>>,
    /// Local ObjectRuntime for this session - initialized from shared state
    /// and synced back after execution
    local: ObjectRuntime,
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
        }
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
        if let Ok(state) = self.shared_state.lock() {
            return state.has_child(parent, child_id);
        }
        false
    }
}

impl Default for SharedObjectRuntime {
    fn default() -> Self {
        Self::new(Arc::new(Mutex::new(ObjectRuntimeState::new())))
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
            .record_created(
                id,
                vec![1],
                type_tag,
                Owner::Address(AccountAddress::ZERO),
            )
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
            .record_created(id, vec![1, 2, 3], type_tag, Owner::Address(AccountAddress::ZERO))
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
            .record_created(id, vec![1, 2, 3], type_tag, Owner::Address(AccountAddress::ZERO))
            .unwrap();

        assert!(runtime.object_store().exists(&id));
        assert_eq!(runtime.object_store().active_count(), 1);

        // Clear should clear both children and object store
        runtime.clear();
        assert!(!runtime.object_store().exists(&id));
        assert_eq!(runtime.object_store().active_count(), 0);
    }
}
