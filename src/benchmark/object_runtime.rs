//! ObjectRuntime - VM extension for dynamic field simulation.
//!
//! This module provides an in-memory object runtime that integrates with the Move VM
//! via its extension mechanism. It enables full dynamic field support including
//! borrow and remove operations that require proper reference semantics.
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
//! ## Current Limitations
//!
//! - Objects don't persist between sessions (each function call is isolated)
//! - No shared object support
//! - No ownership tracking / transfer verification
//! - Objects are identified by (parent_id, child_id) pairs only
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
        let type_tag = make_test_type_tag();

        runtime.add_child_object(parent, child_id, make_test_struct(), type_tag.clone()).unwrap();

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

        runtime.add_child_object(parent, child_id, make_test_struct(), type_tag.clone()).unwrap();
        assert!(runtime.child_object_exists(parent, child_id));

        let _removed = runtime.remove_child_object(parent, child_id, &type_tag).unwrap();
        assert!(!runtime.child_object_exists(parent, child_id));

        // Second remove should fail
        let result = runtime.remove_child_object(parent, child_id, &type_tag);
        assert!(matches!(result, Err(e) if e == E_FIELD_DOES_NOT_EXIST));
    }
}
