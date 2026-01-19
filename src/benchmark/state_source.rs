//! # State Source Abstraction
//!
//! This module provides a trait abstraction for accessing blockchain state,
//! enabling the simulation environment to work with different state backends:
//!
//! - **In-memory**: Uses BTreeMap-based state (default)
//! - **RPC-based**: Fetches state from mainnet on demand
//! - **Custom**: Implement `StateSource` for your own backend
//!
//! ## Design
//!
//! The `StateSource` trait abstracts three main state access patterns:
//! 1. **Module resolution**: Loading Move bytecode for function calls
//! 2. **Object access**: Reading/writing object state
//! 3. **Dynamic fields**: Accessing Table/Bag entries
//!
//! ## Example Usage
//!
//! ```ignore
//! // Create with local state (default)
//! let env = SimulationEnvironment::new()?;
//!
//! // Create with custom state source
//! let state_source = Arc::new(MyCustomStateSource::new());
//! let env = SimulationEnvironment::with_state_source(state_source)?;
//! ```

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::{ModuleId, TypeTag};
use std::sync::Arc;

// =============================================================================
// Version Metadata (P1 Fix)
// =============================================================================

/// Source of object data for provenance tracking (P1 fix).
///
/// Tracks where object data came from for debugging version mismatches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ObjectSource {
    /// From gRPC mainnet fullnode
    GrpcMainnet,
    /// From gRPC testnet fullnode
    GrpcTestnet,
    /// From gRPC archive (historical data)
    GrpcArchive,
    /// From local cache
    Cache,
    /// Locally synthesized (for testing)
    #[default]
    Local,
    /// Unknown source
    Unknown,
}

impl std::fmt::Display for ObjectSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObjectSource::GrpcMainnet => write!(f, "grpc:mainnet"),
            ObjectSource::GrpcTestnet => write!(f, "grpc:testnet"),
            ObjectSource::GrpcArchive => write!(f, "grpc:archive"),
            ObjectSource::Cache => write!(f, "cache"),
            ObjectSource::Local => write!(f, "local"),
            ObjectSource::Unknown => write!(f, "unknown"),
        }
    }
}

/// Version metadata for tracking object provenance and validation (P1 fix).
///
/// This metadata helps debug version mismatch issues during transaction replay.
#[derive(Debug, Clone, Default)]
pub struct ObjectVersionMetadata {
    /// Expected version (from transaction effects)
    pub expected_version: Option<u64>,
    /// Whether the version matches expectations
    pub version_valid: bool,
    /// Source of this data
    pub source: ObjectSource,
    /// Fetch timestamp (when we retrieved this data)
    pub fetched_at_ms: Option<u64>,
    /// Digest for validation
    pub digest: Option<String>,
}

impl ObjectVersionMetadata {
    /// Create new metadata with version validation.
    pub fn new(expected_version: Option<u64>, actual_version: u64, source: ObjectSource) -> Self {
        let version_valid = expected_version
            .map(|ev| ev == actual_version)
            .unwrap_or(true);
        Self {
            expected_version,
            version_valid,
            source,
            fetched_at_ms: Some(current_timestamp_ms()),
            digest: None,
        }
    }

    /// Create metadata for locally created objects.
    pub fn local() -> Self {
        Self {
            expected_version: None,
            version_valid: true,
            source: ObjectSource::Local,
            fetched_at_ms: Some(current_timestamp_ms()),
            digest: None,
        }
    }

    /// Validate that version matches expected.
    pub fn validate_version(&self, actual_version: u64) -> Result<()> {
        if let Some(expected) = self.expected_version {
            if expected != actual_version {
                return Err(anyhow::anyhow!(
                    "Version mismatch: expected {}, got {} (source: {})",
                    expected,
                    actual_version,
                    self.source
                ));
            }
        }
        Ok(())
    }
}

/// Get current timestamp in milliseconds.
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// =============================================================================
// Core State Source Trait
// =============================================================================

/// Trait for accessing blockchain state.
///
/// This is the primary abstraction point for embedding the simulation environment
/// into different contexts (local, full-node, RPC).
///
/// Implementers should ensure thread-safety as the trait requires `Send + Sync`.
pub trait StateSource: Send + Sync {
    /// Get module bytecode by module ID.
    ///
    /// # Arguments
    /// * `id` - The module ID (address + module name)
    ///
    /// # Returns
    /// * `Ok(Some(bytes))` - Module bytecode if found
    /// * `Ok(None)` - Module not found
    /// * `Err(e)` - Error accessing state
    fn get_module(&self, id: &ModuleId) -> Result<Option<Vec<u8>>>;

    /// Check if a module exists.
    ///
    /// Default implementation calls `get_module` and checks for `Some`.
    fn has_module(&self, id: &ModuleId) -> bool {
        self.get_module(id).map(|m| m.is_some()).unwrap_or(false)
    }

    /// Get object data by object ID.
    ///
    /// # Arguments
    /// * `id` - The object's account address
    ///
    /// # Returns
    /// * `Ok(Some(ObjectData))` - Object data if found
    /// * `Ok(None)` - Object not found
    /// * `Err(e)` - Error accessing state
    fn get_object(&self, id: &AccountAddress) -> Result<Option<ObjectData>>;

    /// Check if an object exists.
    ///
    /// Default implementation calls `get_object` and checks for `Some`.
    fn has_object(&self, id: &AccountAddress) -> bool {
        self.get_object(id).map(|o| o.is_some()).unwrap_or(false)
    }

    /// Get a dynamic field value.
    ///
    /// # Arguments
    /// * `parent_id` - The parent object's ID
    /// * `field_key` - The field key (BCS-encoded)
    ///
    /// # Returns
    /// * `Ok(Some((type_tag, bytes)))` - Field value if found
    /// * `Ok(None)` - Field not found
    /// * `Err(e)` - Error accessing state
    fn get_dynamic_field(
        &self,
        parent_id: &AccountAddress,
        field_key: &[u8],
    ) -> Result<Option<(TypeTag, Vec<u8>)>>;

    /// List all packages (module addresses) available in this state source.
    ///
    /// This is optional and may return an empty vector for state sources
    /// that don't support enumeration (e.g., RPC-based).
    fn list_packages(&self) -> Vec<AccountAddress> {
        vec![]
    }

    /// Get the total number of modules loaded.
    ///
    /// Returns 0 if the state source doesn't track this information.
    fn module_count(&self) -> usize {
        0
    }
}

// =============================================================================
// Object Data Structure
// =============================================================================

/// Object data returned by StateSource.
///
/// This is a simplified view of object state suitable for simulation.
/// Full-node implementations may need to convert from their internal
/// representation.
#[derive(Debug, Clone)]
pub struct ObjectData {
    /// Object ID (same as the key used to fetch)
    pub id: AccountAddress,

    /// Object type tag
    pub type_tag: TypeTag,

    /// BCS-serialized object contents
    pub bcs_bytes: Vec<u8>,

    /// Whether this is a shared object
    pub is_shared: bool,

    /// Whether this object is immutable
    pub is_immutable: bool,

    /// Object version (lamport timestamp)
    pub version: u64,
}

impl ObjectData {
    /// Create a new ObjectData instance.
    pub fn new(
        id: AccountAddress,
        type_tag: TypeTag,
        bcs_bytes: Vec<u8>,
        is_shared: bool,
        is_immutable: bool,
        version: u64,
    ) -> Self {
        Self {
            id,
            type_tag,
            bcs_bytes,
            is_shared,
            is_immutable,
            version,
        }
    }

    /// Create an owned (non-shared, mutable) object.
    pub fn owned(id: AccountAddress, type_tag: TypeTag, bcs_bytes: Vec<u8>) -> Self {
        Self::new(id, type_tag, bcs_bytes, false, false, 1)
    }

    /// Create a shared object.
    pub fn shared(id: AccountAddress, type_tag: TypeTag, bcs_bytes: Vec<u8>) -> Self {
        Self::new(id, type_tag, bcs_bytes, true, false, 1)
    }

    /// Create an immutable object.
    pub fn immutable(id: AccountAddress, type_tag: TypeTag, bcs_bytes: Vec<u8>) -> Self {
        Self::new(id, type_tag, bcs_bytes, false, true, 1)
    }
}

// =============================================================================
// Local State Source Implementation
// =============================================================================

use crate::benchmark::resolver::{LocalModuleResolver, ModuleProvider};
use std::collections::BTreeMap;
use std::sync::RwLock;

/// Local in-memory state source.
///
/// This is the default implementation used by `SimulationEnvironment::new()`.
/// It stores all state in memory and is suitable for:
/// - Unit testing
/// - Single-user simulations
/// - Benchmark scenarios
///
/// For production use with persistent state, implement `StateSource` with
/// a database-backed store.
pub struct LocalStateSource {
    /// Module resolver for bytecode access
    resolver: RwLock<LocalModuleResolver>,

    /// Object store
    objects: RwLock<BTreeMap<AccountAddress, ObjectData>>,

    /// Dynamic field store: (parent_id, field_key_hash) -> (type_tag, bytes)
    dynamic_fields: RwLock<BTreeMap<(AccountAddress, Vec<u8>), (TypeTag, Vec<u8>)>>,
}

impl LocalStateSource {
    /// Create a new local state source with the Sui framework loaded.
    pub fn new() -> Result<Self> {
        let resolver = LocalModuleResolver::with_sui_framework()?;
        Ok(Self {
            resolver: RwLock::new(resolver),
            objects: RwLock::new(BTreeMap::new()),
            dynamic_fields: RwLock::new(BTreeMap::new()),
        })
    }

    /// Create a local state source with a pre-configured resolver.
    pub fn with_resolver(resolver: LocalModuleResolver) -> Self {
        Self {
            resolver: RwLock::new(resolver),
            objects: RwLock::new(BTreeMap::new()),
            dynamic_fields: RwLock::new(BTreeMap::new()),
        }
    }

    /// Get mutable access to the resolver for loading modules.
    pub fn resolver_mut(&self) -> std::sync::RwLockWriteGuard<'_, LocalModuleResolver> {
        self.resolver.write().unwrap()
    }

    /// Get read access to the resolver.
    pub fn resolver(&self) -> std::sync::RwLockReadGuard<'_, LocalModuleResolver> {
        self.resolver.read().unwrap()
    }

    /// Insert an object into the store.
    pub fn insert_object(&self, object: ObjectData) {
        self.objects.write().unwrap().insert(object.id, object);
    }

    /// Remove an object from the store.
    pub fn remove_object(&self, id: &AccountAddress) -> Option<ObjectData> {
        self.objects.write().unwrap().remove(id)
    }

    /// Update object bytes.
    pub fn update_object_bytes(&self, id: &AccountAddress, bytes: Vec<u8>) -> Result<()> {
        let mut objects = self.objects.write().unwrap();
        if let Some(obj) = objects.get_mut(id) {
            obj.bcs_bytes = bytes;
            obj.version += 1;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Object {} not found", id.to_hex_literal()))
        }
    }

    /// Set a dynamic field value.
    pub fn set_dynamic_field(
        &self,
        parent_id: AccountAddress,
        field_key: Vec<u8>,
        type_tag: TypeTag,
        value: Vec<u8>,
    ) {
        self.dynamic_fields
            .write()
            .unwrap()
            .insert((parent_id, field_key), (type_tag, value));
    }

    /// Remove a dynamic field.
    pub fn remove_dynamic_field(
        &self,
        parent_id: &AccountAddress,
        field_key: &[u8],
    ) -> Option<(TypeTag, Vec<u8>)> {
        self.dynamic_fields
            .write()
            .unwrap()
            .remove(&(*parent_id, field_key.to_vec()))
    }

    /// List all objects in the store.
    pub fn list_objects(&self) -> Vec<ObjectData> {
        self.objects.read().unwrap().values().cloned().collect()
    }
}

impl StateSource for LocalStateSource {
    fn get_module(&self, id: &ModuleId) -> Result<Option<Vec<u8>>> {
        let resolver = self.resolver.read().unwrap();
        Ok(resolver.get_module_bytes(id).map(|b| b.to_vec()))
    }

    fn has_module(&self, id: &ModuleId) -> bool {
        let resolver = self.resolver.read().unwrap();
        resolver.has_module(id)
    }

    fn get_object(&self, id: &AccountAddress) -> Result<Option<ObjectData>> {
        Ok(self.objects.read().unwrap().get(id).cloned())
    }

    fn get_dynamic_field(
        &self,
        parent_id: &AccountAddress,
        field_key: &[u8],
    ) -> Result<Option<(TypeTag, Vec<u8>)>> {
        Ok(self
            .dynamic_fields
            .read()
            .unwrap()
            .get(&(*parent_id, field_key.to_vec()))
            .cloned())
    }

    fn list_packages(&self) -> Vec<AccountAddress> {
        self.resolver.read().unwrap().list_packages()
    }

    fn module_count(&self) -> usize {
        self.resolver.read().unwrap().module_count()
    }
}

// =============================================================================
// Arc<dyn StateSource> Convenience
// =============================================================================

/// Type alias for a shared state source.
pub type SharedStateSource = Arc<dyn StateSource>;

/// Create a new local state source wrapped in Arc.
pub fn new_local_state_source() -> Result<SharedStateSource> {
    Ok(Arc::new(LocalStateSource::new()?))
}

/// Create a local state source with a pre-configured resolver, wrapped in Arc.
pub fn local_state_source_with_resolver(resolver: LocalModuleResolver) -> SharedStateSource {
    Arc::new(LocalStateSource::with_resolver(resolver))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use move_core_types::identifier::Identifier;

    #[test]
    fn test_local_state_source_creation() {
        let source = LocalStateSource::new().unwrap();
        // Should have framework modules loaded
        assert!(source.module_count() > 0);
    }

    #[test]
    fn test_local_state_source_module_access() {
        let source = LocalStateSource::new().unwrap();

        // Check for a known framework module
        let sui_module_id = ModuleId::new(
            AccountAddress::from_hex_literal("0x2").unwrap(),
            Identifier::new("sui").unwrap(),
        );

        assert!(source.has_module(&sui_module_id));
        let bytes = source.get_module(&sui_module_id).unwrap();
        assert!(bytes.is_some());
    }

    #[test]
    fn test_local_state_source_object_operations() {
        let source = LocalStateSource::new().unwrap();

        let id = AccountAddress::from_hex_literal("0x123").unwrap();
        let obj = ObjectData::owned(id, TypeTag::U64, vec![1, 2, 3, 4]);

        // Insert
        source.insert_object(obj.clone());
        assert!(source.has_object(&id));

        // Get
        let retrieved = source.get_object(&id).unwrap().unwrap();
        assert_eq!(retrieved.bcs_bytes, vec![1, 2, 3, 4]);

        // Update
        source.update_object_bytes(&id, vec![5, 6, 7, 8]).unwrap();
        let updated = source.get_object(&id).unwrap().unwrap();
        assert_eq!(updated.bcs_bytes, vec![5, 6, 7, 8]);
        assert_eq!(updated.version, 2);

        // Remove
        let removed = source.remove_object(&id);
        assert!(removed.is_some());
        assert!(!source.has_object(&id));
    }

    #[test]
    fn test_local_state_source_dynamic_fields() {
        let source = LocalStateSource::new().unwrap();

        let parent_id = AccountAddress::from_hex_literal("0x456").unwrap();
        let field_key = vec![1, 2, 3];
        let type_tag = TypeTag::Bool;
        let value = vec![1]; // true

        // Set
        source.set_dynamic_field(
            parent_id,
            field_key.clone(),
            type_tag.clone(),
            value.clone(),
        );

        // Get
        let retrieved = source
            .get_dynamic_field(&parent_id, &field_key)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.0, type_tag);
        assert_eq!(retrieved.1, value);

        // Remove
        let removed = source.remove_dynamic_field(&parent_id, &field_key);
        assert!(removed.is_some());
        assert!(source
            .get_dynamic_field(&parent_id, &field_key)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_shared_state_source() {
        let source: SharedStateSource = new_local_state_source().unwrap();

        // Should be usable as a trait object
        assert!(source.module_count() > 0);
    }
}
