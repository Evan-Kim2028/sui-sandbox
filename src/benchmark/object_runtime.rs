//! ObjectRuntime - VM extension for dynamic field simulation.
//!
//! This module provides an in-memory object runtime that integrates with the Move VM
//! via its extension mechanism. It enables full dynamic field support including
//! borrow and remove operations that require proper reference semantics.
//!
//! ## Architecture
//!
//! The Move VM allows native functions to access external state via "extensions".
//! Extensions are registered when creating a session and accessed via:
//! ```ignore
//! let runtime: &mut ObjectRuntime = context.extensions_mut().get_mut()?;
//! ```
//!
//! This module provides:
//! - `ObjectRuntime`: The extension struct that stores GlobalValues
//! - Integration with `NativeContextExtensions` via `better_any` derive macros
//!
//! ## Why We Need This
//!
//! Dynamic field operations like `borrow_child_object` must return a **reference**
//! to a value that lives in the VM's memory management. The VM needs to:
//! 1. Track that reference for borrow checking
//! 2. Allow mutations to propagate (for borrow_mut)
//! 3. Handle the value's lifecycle
//!
//! `GlobalValue` from move-vm-types provides this, but the value must live
//! somewhere that outlives the native function call. The extension mechanism
//! provides exactly this - state that persists for the session duration.

use better_any::{Tid, TidAble};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use move_vm_runtime::native_extensions::NativeExtensionMarker;
use move_vm_types::values::{GlobalValue, Value};
use std::collections::HashMap;

/// Error codes matching Sui's dynamic_field module
pub const E_FIELD_ALREADY_EXISTS: u64 = 0;
pub const E_FIELD_DOES_NOT_EXIST: u64 = 1;
pub const E_FIELD_TYPE_MISMATCH: u64 = 2;

/// A child object stored in the runtime.
pub struct ChildObject {
    /// The GlobalValue wrapping the Move value (enables reference semantics)
    pub value: GlobalValue,
    /// Type tag for type checking
    pub type_tag: TypeTag,
}

/// In-memory object runtime for dynamic field simulation.
///
/// This is registered as a VM extension and provides storage for GlobalValues
/// that can be borrowed as references by native functions.
#[derive(Tid, Default)]
pub struct ObjectRuntime {
    /// Map from (parent_id, child_id) -> child object
    /// Using the same addressing scheme as Sui's dynamic fields
    children: HashMap<(AccountAddress, AccountAddress), ChildObject>,
}

// Mark as a native extension so it can be registered with the VM
impl NativeExtensionMarker<'_> for ObjectRuntime {}

impl ObjectRuntime {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
        }
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
        let global_value = GlobalValue::cached(value)
            .map_err(|_| E_FIELD_TYPE_MISMATCH)?;

        self.children.insert(key, ChildObject {
            value: global_value,
            type_tag,
        });

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
        let child = self.children
            .get(&(parent, child_id))
            .ok_or(E_FIELD_DOES_NOT_EXIST)?;

        // Type check
        if &child.type_tag != expected_type {
            return Err(E_FIELD_TYPE_MISMATCH);
        }

        // borrow_global returns a Value that is internally a reference
        child.value
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
        let child = self.children
            .get(&(parent, child_id))
            .ok_or(E_FIELD_DOES_NOT_EXIST)?;

        if &child.type_tag != expected_type {
            return Err(E_FIELD_TYPE_MISMATCH);
        }

        child.value
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
        child.value
            .into_value()
            .ok_or(E_FIELD_DOES_NOT_EXIST)
    }

    /// Clear all stored objects (for test isolation).
    pub fn clear(&mut self) {
        self.children.clear();
    }

    /// Get the number of stored objects.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use move_vm_types::values::Value;

    #[test]
    fn test_add_and_exists() {
        let mut runtime = ObjectRuntime::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child_id = AccountAddress::from_hex_literal("0x2").unwrap();
        let value = Value::u64(42);
        let type_tag = TypeTag::U64;

        assert!(!runtime.child_object_exists(parent, child_id));

        runtime.add_child_object(parent, child_id, value, type_tag.clone()).unwrap();

        assert!(runtime.child_object_exists(parent, child_id));
        assert!(runtime.child_object_exists_with_type(parent, child_id, &type_tag));
        assert!(!runtime.child_object_exists_with_type(parent, child_id, &TypeTag::Bool));
    }

    #[test]
    fn test_add_duplicate_fails() {
        let mut runtime = ObjectRuntime::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child_id = AccountAddress::from_hex_literal("0x2").unwrap();

        runtime.add_child_object(parent, child_id, Value::u64(1), TypeTag::U64).unwrap();

        let result = runtime.add_child_object(parent, child_id, Value::u64(2), TypeTag::U64);
        assert_eq!(result, Err(E_FIELD_ALREADY_EXISTS));
    }

    #[test]
    fn test_borrow_nonexistent_fails() {
        let runtime = ObjectRuntime::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child_id = AccountAddress::from_hex_literal("0x2").unwrap();

        let result = runtime.borrow_child_object(parent, child_id, &TypeTag::U64);
        assert_eq!(result, Err(E_FIELD_DOES_NOT_EXIST));
    }

    #[test]
    fn test_remove() {
        let mut runtime = ObjectRuntime::new();
        let parent = AccountAddress::from_hex_literal("0x1").unwrap();
        let child_id = AccountAddress::from_hex_literal("0x2").unwrap();
        let type_tag = TypeTag::U64;

        runtime.add_child_object(parent, child_id, Value::u64(42), type_tag.clone()).unwrap();
        assert!(runtime.child_object_exists(parent, child_id));

        let _removed = runtime.remove_child_object(parent, child_id, &type_tag).unwrap();
        assert!(!runtime.child_object_exists(parent, child_id));

        // Second remove should fail
        let result = runtime.remove_child_object(parent, child_id, &type_tag);
        assert_eq!(result, Err(E_FIELD_DOES_NOT_EXIST));
    }
}
