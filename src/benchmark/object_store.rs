//! In-memory object store for dynamic field simulation.
//!
//! This provides a minimal object storage layer that enables testing Move code
//! that uses dynamic fields (Bag, Table, etc.) without a full Sui runtime.
//!
//! ## Design
//!
//! Dynamic fields in Sui work by:
//! 1. Hashing (parent_id, key_type, key_value) to get a child object ID
//! 2. Storing a `Field<K,V>` struct at that ID as a child of the parent
//! 3. Looking up by the hash to retrieve the value
//!
//! We simulate this with a simple HashMap:
//! - Key: (parent_address, child_address)
//! - Value: BCS-serialized object bytes + type tag
//!
//! ## Limitations
//!
//! - No garbage collection / ownership tracking
//! - No shared object support
//! - Objects are not persisted between benchmark runs
//! - Type checking is best-effort (we store TypeTag but can't fully verify)

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// A stored object in our in-memory store.
#[derive(Debug, Clone)]
pub struct StoredObject {
    /// BCS-serialized bytes of the object
    pub bytes: Vec<u8>,
    /// Type tag of the stored object (for type checking)
    pub type_tag: TypeTag,
}

/// Thread-safe in-memory object store.
/// 
/// Uses (parent_address, child_address) as the key to match Sui's
/// dynamic field storage model where children are addressed by a hash
/// of the parent ID and key.
#[derive(Debug, Default)]
pub struct ObjectStore {
    /// Map from (parent, child_id) -> stored object
    objects: RwLock<HashMap<(AccountAddress, AccountAddress), StoredObject>>,
}

impl ObjectStore {
    pub fn new() -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
        }
    }
    
    /// Create a new Arc-wrapped store for sharing across native calls.
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::new())
    }
    
    /// Add a child object under a parent.
    /// This is called by `dynamic_field::add_child_object`.
    pub fn add_child(
        &self,
        parent: AccountAddress,
        child_id: AccountAddress,
        bytes: Vec<u8>,
        type_tag: TypeTag,
    ) -> Result<(), ObjectStoreError> {
        let mut objects = self.objects.write().unwrap();
        let key = (parent, child_id);
        
        if objects.contains_key(&key) {
            return Err(ObjectStoreError::AlreadyExists);
        }
        
        objects.insert(key, StoredObject { bytes, type_tag });
        Ok(())
    }
    
    /// Check if a child object exists.
    /// This is called by `dynamic_field::has_child_object`.
    pub fn has_child(&self, parent: AccountAddress, child_id: AccountAddress) -> bool {
        let objects = self.objects.read().unwrap();
        objects.contains_key(&(parent, child_id))
    }
    
    /// Check if a child object exists with a specific type.
    /// This is called by `dynamic_field::has_child_object_with_ty`.
    pub fn has_child_with_type(
        &self,
        parent: AccountAddress,
        child_id: AccountAddress,
        expected_type: &TypeTag,
    ) -> bool {
        let objects = self.objects.read().unwrap();
        match objects.get(&(parent, child_id)) {
            Some(obj) => &obj.type_tag == expected_type,
            None => false,
        }
    }
    
    /// Borrow a child object's bytes.
    /// This is called by `dynamic_field::borrow_child_object`.
    pub fn borrow_child(
        &self,
        parent: AccountAddress,
        child_id: AccountAddress,
    ) -> Result<StoredObject, ObjectStoreError> {
        let objects = self.objects.read().unwrap();
        objects
            .get(&(parent, child_id))
            .cloned()
            .ok_or(ObjectStoreError::NotFound)
    }
    
    /// Remove a child object and return its bytes.
    /// This is called by `dynamic_field::remove_child_object`.
    pub fn remove_child(
        &self,
        parent: AccountAddress,
        child_id: AccountAddress,
    ) -> Result<StoredObject, ObjectStoreError> {
        let mut objects = self.objects.write().unwrap();
        objects
            .remove(&(parent, child_id))
            .ok_or(ObjectStoreError::NotFound)
    }
    
    /// Clear all stored objects (for test isolation).
    pub fn clear(&self) {
        let mut objects = self.objects.write().unwrap();
        objects.clear();
    }
    
    /// Get the number of stored objects (for debugging).
    pub fn len(&self) -> usize {
        let objects = self.objects.read().unwrap();
        objects.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Errors from object store operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectStoreError {
    /// Object already exists at this location (EFieldAlreadyExists)
    AlreadyExists,
    /// Object not found (EFieldDoesNotExist)
    NotFound,
    /// Type mismatch (EFieldTypeMismatch)
    TypeMismatch,
}

impl ObjectStoreError {
    /// Convert to Sui error code for abort.
    pub fn to_sui_error_code(&self) -> u64 {
        match self {
            ObjectStoreError::AlreadyExists => 0, // EFieldAlreadyExists
            ObjectStoreError::NotFound => 1,      // EFieldDoesNotExist
            ObjectStoreError::TypeMismatch => 2,  // EFieldTypeMismatch
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_add_and_borrow() {
        let store = ObjectStore::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child = AccountAddress::from_hex_literal("0x2").unwrap();
        let bytes = vec![1, 2, 3];
        let type_tag = TypeTag::U64;
        
        // Add should succeed
        store.add_child(parent, child, bytes.clone(), type_tag.clone()).unwrap();
        
        // Has should return true
        assert!(store.has_child(parent, child));
        
        // Borrow should return the bytes
        let obj = store.borrow_child(parent, child).unwrap();
        assert_eq!(obj.bytes, bytes);
        assert_eq!(obj.type_tag, type_tag);
    }
    
    #[test]
    fn test_add_duplicate_fails() {
        let store = ObjectStore::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child = AccountAddress::from_hex_literal("0x2").unwrap();
        
        store.add_child(parent, child, vec![1], TypeTag::U64).unwrap();
        
        // Second add should fail
        let result = store.add_child(parent, child, vec![2], TypeTag::U64);
        assert_eq!(result, Err(ObjectStoreError::AlreadyExists));
    }
    
    #[test]
    fn test_remove() {
        let store = ObjectStore::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child = AccountAddress::from_hex_literal("0x2").unwrap();
        
        store.add_child(parent, child, vec![1, 2, 3], TypeTag::U64).unwrap();
        
        // Remove should succeed and return the object
        let obj = store.remove_child(parent, child).unwrap();
        assert_eq!(obj.bytes, vec![1, 2, 3]);
        
        // Should no longer exist
        assert!(!store.has_child(parent, child));
        
        // Second remove should fail
        assert_eq!(store.remove_child(parent, child), Err(ObjectStoreError::NotFound));
    }
}
