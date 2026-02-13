//! Session state management for sui-sandbox
//!
//! Handles persistence of sandbox state across CLI invocations.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::simulation::state::{SerializedPackage, SerializedPackageModule};
use sui_sandbox_core::simulation::{
    CoinMetadata, PersistentState, SerializedModule, SerializedObject, StateMetadata,
    SUI_COIN_TYPE, SUI_DECIMALS, SUI_SYMBOL,
};
use sui_sandbox_core::types::parse_type_tag;
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};

/// Additional object metadata for persistence.
#[derive(Default)]
pub(crate) struct ObjectMetadata {
    pub version: u64,
    pub is_shared: bool,
    pub is_immutable: bool,
    pub owner: Option<String>,
}

/// Runtime sandbox state (in-memory)
pub struct SandboxState {
    /// Module resolver with loaded packages
    pub resolver: LocalModuleResolver,
    /// Persistent JSON state for CLI sessions
    pub persisted: PersistentState,
    /// RPC URL for fetching
    pub rpc_url: String,
    /// Whether state has been modified
    pub dirty: bool,
}

impl SandboxState {
    /// Create a new empty sandbox state
    pub fn new(rpc_url: &str) -> Result<Self> {
        let mut resolver = LocalModuleResolver::new();
        // Load Sui framework automatically
        resolver.load_sui_framework_auto()?;

        Ok(Self {
            resolver,
            persisted: default_persistent_state(),
            rpc_url: rpc_url.to_string(),
            dirty: false,
        })
    }

    /// Load state from file or create new if not exists
    pub fn load_or_create(path: &Path, rpc_url: &str) -> Result<Self> {
        if path.exists() {
            Self::load(path, rpc_url)
        } else {
            Self::new(rpc_url)
        }
    }

    /// Load state from a file (JSON only)
    pub fn load(path: &Path, rpc_url: &str) -> Result<Self> {
        let data = std::fs::read(path).context("Failed to read state file")?;
        match serde_json::from_slice::<PersistentState>(&data) {
            Ok(persisted) => Self::from_persistent_state(persisted, rpc_url),
            Err(json_err) => Err(anyhow!(
                "Failed to parse state file as JSON. Legacy bincode state files are no longer supported.\nJSON error: {}",
                json_err
            )),
        }
    }

    /// Save state to a file (JSON)
    pub fn save(&self, path: &Path) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut persisted = self.persisted.clone();
        persisted.version = PersistentState::CURRENT_VERSION;
        update_metadata(&mut persisted, true);

        let data = serde_json::to_string_pretty(&persisted)
            .map_err(|e| anyhow!("Failed to serialize state: {}", e))?;
        std::fs::write(path, data).context("Failed to write state file")?;

        Ok(())
    }

    /// Add a package to the state
    pub fn add_package(&mut self, address: AccountAddress, modules: Vec<(String, Vec<u8>)>) {
        let addr_str = address.to_hex_literal();

        // Add to resolver
        let _ = self.resolver.add_package_modules_at(
            modules
                .iter()
                .map(|(n, b)| (n.clone(), b.clone()))
                .collect(),
            Some(address),
        );

        let mut package_modules: Vec<SerializedPackageModule> = Vec::new();
        for (name, bytes) in modules {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            package_modules.push(SerializedPackageModule {
                name: name.clone(),
                bytecode_b64: b64.clone(),
            });
            upsert_module(&mut self.persisted, &addr_str, &name, b64);
        }

        upsert_package(&mut self.persisted, &addr_str, package_modules);
        self.dirty = true;
    }

    /// Get the last published package address
    pub fn last_published(&self) -> Option<AccountAddress> {
        self.loaded_packages()
            .last()
            .and_then(|s| AccountAddress::from_hex_literal(s).ok())
    }

    /// Create a VM harness with a specific sender and gas budget
    pub fn create_harness_with_sender(
        &self,
        sender: AccountAddress,
        gas_budget: Option<u64>,
    ) -> Result<VMHarness<'_>> {
        let config = SimulationConfig {
            sender_address: sender.into_bytes(),
            gas_budget,
            ..Default::default()
        };
        VMHarness::with_config(&self.resolver, false, config)
    }

    /// Get list of loaded packages
    pub fn loaded_packages(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut packages = Vec::new();

        for pkg in &self.persisted.packages {
            let addr = normalize_addr(&pkg.address).unwrap_or_else(|| pkg.address.clone());
            if seen.insert(addr.to_lowercase()) {
                packages.push(addr);
            }
        }

        for module in &self.persisted.modules {
            if let Some((addr, _module)) = split_module_id(&module.id) {
                let addr = normalize_addr(addr).unwrap_or_else(|| addr.to_string());
                if seen.insert(addr.to_lowercase()) {
                    packages.push(addr);
                }
            }
        }

        packages
    }

    /// Get package module names
    pub fn get_package_modules(&self, address: &str) -> Option<Vec<String>> {
        let addr = AccountAddress::from_hex_literal(address).ok()?;
        let mut modules: Vec<String> = Vec::new();

        for pkg in &self.persisted.packages {
            if addresses_equal(&pkg.address, &addr.to_hex_literal()) {
                modules.extend(pkg.modules.iter().map(|m| m.name.clone()));
            }
        }
        if !modules.is_empty() {
            return Some(modules);
        }

        for module in &self.persisted.modules {
            if let Some((addr_str, name)) = split_module_id(&module.id) {
                if addresses_equal(addr_str, &addr.to_hex_literal()) {
                    modules.push(name.to_string());
                }
            }
        }

        if modules.is_empty() {
            None
        } else {
            Some(modules)
        }
    }

    /// Add an object to the state (requires a valid type tag).
    #[allow(dead_code)]
    pub fn add_object(
        &mut self,
        object_id: &str,
        type_tag: Option<&str>,
        bcs_base64: &str,
    ) -> Result<()> {
        self.add_object_with_metadata(object_id, type_tag, bcs_base64, ObjectMetadata::default())
    }

    /// Add an object with metadata to the state (requires a valid type tag).
    pub fn add_object_with_metadata(
        &mut self,
        object_id: &str,
        type_tag: Option<&str>,
        bcs_base64: &str,
        metadata: ObjectMetadata,
    ) -> Result<()> {
        let addr = AccountAddress::from_hex_literal(object_id)
            .map_err(|e| anyhow!("Invalid object ID '{}': {}", object_id, e))?;
        let type_tag = type_tag
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("Object {} missing type tag; cannot persist", object_id))?;
        parse_type_tag(type_tag).map_err(|e| anyhow!("Invalid type tag {}: {}", type_tag, e))?;

        let obj = SerializedObject {
            id: addr.to_hex_literal(),
            type_tag: type_tag.to_string(),
            bcs_bytes_b64: bcs_base64.to_string(),
            is_shared: metadata.is_shared,
            is_immutable: metadata.is_immutable,
            version: metadata.version,
            owner: metadata.owner,
        };

        upsert_object(&mut self.persisted, obj);
        self.dirty = true;
        Ok(())
    }

    /// Get object bytes and parsed type tag for PTB inputs.
    pub fn get_object_input(&self, object_id: &str) -> Result<(Vec<u8>, Option<TypeTag>)> {
        let obj = self.find_object(object_id).ok_or_else(|| {
            anyhow::anyhow!(
                "Object {} not found in session. Run `sui-sandbox fetch object {}` first.",
                object_id,
                object_id
            )
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&obj.bcs_bytes_b64)
            .map_err(|e| anyhow!("Invalid cached BCS for {}: {}", object_id, e))?;
        let parsed = if obj.type_tag.trim().is_empty() {
            None
        } else {
            parse_type_tag(&obj.type_tag).ok()
        };
        Ok((bytes, parsed))
    }

    /// Get object type tag if present.
    pub fn object_type_tag(&self, object_id: &str) -> Option<String> {
        self.find_object(object_id).map(|obj| obj.type_tag.clone())
    }

    /// Set last sender
    pub fn set_last_sender(&mut self, sender: AccountAddress) {
        self.persisted.sender = sender.to_hex_literal();
        self.dirty = true;
    }

    /// Get last sender as hex string (if set).
    pub fn last_sender_hex(&self) -> Option<String> {
        let sender = self.persisted.sender.trim();
        if sender.is_empty() {
            return None;
        }
        match AccountAddress::from_hex_literal(sender) {
            Ok(addr) if addr == AccountAddress::ZERO => None,
            Ok(addr) => Some(addr.to_hex_literal()),
            Err(_) => None,
        }
    }

    /// Get metadata created timestamp.
    pub fn metadata_created_at(&self) -> Option<&str> {
        self.persisted
            .metadata
            .as_ref()
            .and_then(|m| m.created_at.as_deref())
    }

    /// Get metadata modified timestamp.
    pub fn metadata_modified_at(&self) -> Option<&str> {
        self.persisted
            .metadata
            .as_ref()
            .and_then(|m| m.modified_at.as_deref())
    }

    /// Count persisted objects in the session.
    pub fn objects_count(&self) -> usize {
        self.persisted.objects.len()
    }

    /// Count persisted modules (standalone + package modules).
    pub fn modules_count(&self) -> usize {
        let pkg_modules: usize = self
            .persisted
            .packages
            .iter()
            .map(|p| p.modules.len())
            .sum();
        self.persisted.modules.len() + pkg_modules
    }

    /// Count tracked dynamic field entries in persisted state.
    pub fn dynamic_fields_count(&self) -> usize {
        self.persisted.dynamic_fields.len()
    }

    /// Return a snapshot-safe clone of the persisted session state.
    pub fn snapshot_state(&self) -> PersistentState {
        let mut cloned = self.persisted.clone();
        cloned.version = PersistentState::CURRENT_VERSION;
        update_metadata(&mut cloned, false);
        cloned
    }

    /// Replace in-memory state from a persisted snapshot payload.
    pub fn replace_persistent_state(&mut self, persisted: PersistentState) -> Result<()> {
        let loaded = Self::from_persistent_state(persisted, &self.rpc_url)?;
        self.resolver = loaded.resolver;
        self.persisted = loaded.persisted;
        self.dirty = true;
        Ok(())
    }

    /// Mark the state as dirty so it will be persisted on command success.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Reset session state to a clean local baseline while keeping config defaults.
    pub fn reset_session(&mut self) -> Result<()> {
        let mut resolver = LocalModuleResolver::new();
        resolver.load_sui_framework_auto()?;
        self.resolver = resolver;
        self.persisted = default_persistent_state();
        self.dirty = true;
        Ok(())
    }

    /// Get last sender or default
    #[cfg(test)]
    pub fn last_sender(&self) -> AccountAddress {
        self.last_sender_hex()
            .and_then(|s| AccountAddress::from_hex_literal(&s).ok())
            .unwrap_or(AccountAddress::ZERO)
    }

    fn find_object(&self, object_id: &str) -> Option<&SerializedObject> {
        let addr = AccountAddress::from_hex_literal(object_id).ok()?;
        self.persisted.objects.iter().find(|obj| {
            AccountAddress::from_hex_literal(&obj.id)
                .map(|id| id == addr)
                .unwrap_or(false)
        })
    }

    fn from_persistent_state(mut persisted: PersistentState, rpc_url: &str) -> Result<Self> {
        if persisted.version > PersistentState::CURRENT_VERSION {
            return Err(anyhow!(
                "State file version {} is newer than supported version {}",
                persisted.version,
                PersistentState::CURRENT_VERSION
            ));
        }

        let resolver = build_resolver(&persisted)?;
        let mut dirty = false;

        if persisted.sender.trim().is_empty() {
            persisted.sender = "0x0".to_string();
            dirty = true;
        }

        if persisted.metadata.is_none() {
            let now = chrono::Utc::now().to_rfc3339();
            persisted.metadata = Some(StateMetadata {
                description: None,
                created_at: Some(now),
                modified_at: None,
                tags: Vec::new(),
            });
            dirty = true;
        }

        Ok(Self {
            resolver,
            persisted,
            rpc_url: rpc_url.to_string(),
            dirty,
        })
    }
}

fn default_persistent_state() -> PersistentState {
    let mut coin_registry = HashMap::new();
    coin_registry.insert(
        SUI_COIN_TYPE.to_string(),
        CoinMetadata {
            decimals: SUI_DECIMALS,
            symbol: SUI_SYMBOL.to_string(),
            name: "Sui".to_string(),
            type_tag: SUI_COIN_TYPE.to_string(),
        },
    );

    let now = chrono::Utc::now().to_rfc3339();

    PersistentState {
        version: PersistentState::CURRENT_VERSION,
        objects: Vec::new(),
        object_history: Vec::new(),
        modules: Vec::new(),
        packages: Vec::new(),
        coin_registry,
        sender: "0x0".to_string(),
        id_counter: 0,
        timestamp_ms: None,
        dynamic_fields: Vec::new(),
        pending_receives: Vec::new(),
        config: Some(SimulationConfig::default()),
        metadata: Some(StateMetadata {
            description: None,
            created_at: Some(now),
            modified_at: None,
            tags: Vec::new(),
        }),
        fetcher_config: None,
    }
}

fn build_resolver(persisted: &PersistentState) -> Result<LocalModuleResolver> {
    let mut resolver = LocalModuleResolver::new();
    resolver.load_sui_framework_auto()?;

    // Load standalone modules - skip invalid bytecode gracefully
    for module in &persisted.modules {
        let bytes = match base64::engine::general_purpose::STANDARD.decode(&module.bytecode_b64) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Warning: invalid base64 for module {}: {}", module.id, e);
                continue;
            }
        };
        if let Err(e) = resolver.add_module_bytes(bytes) {
            eprintln!("Warning: failed to load module {}: {}", module.id, e);
        }
    }

    // Load packages - skip invalid bytecode gracefully
    for pkg in &persisted.packages {
        let addr = AccountAddress::from_hex_literal(&pkg.address).ok();
        let mut modules = Vec::new();
        for module in &pkg.modules {
            let bytes = match base64::engine::general_purpose::STANDARD.decode(&module.bytecode_b64)
            {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "Warning: invalid base64 for module {}::{}: {}",
                        pkg.address, module.name, e
                    );
                    continue;
                }
            };
            modules.push((module.name.clone(), bytes));
        }
        if !modules.is_empty() {
            let _ = resolver.add_package_modules_at(modules, addr);
        }
    }

    Ok(resolver)
}

fn update_metadata(state: &mut PersistentState, mark_modified: bool) {
    let now = chrono::Utc::now().to_rfc3339();
    let metadata = state.metadata.get_or_insert_with(StateMetadata::default);
    if metadata.created_at.is_none() {
        metadata.created_at = Some(now.clone());
    }
    if mark_modified {
        metadata.modified_at = Some(now);
    }
}

fn upsert_package(
    state: &mut PersistentState,
    address: &str,
    modules: Vec<SerializedPackageModule>,
) {
    if let Some(existing) = state
        .packages
        .iter_mut()
        .find(|pkg| addresses_equal(&pkg.address, address))
    {
        existing.modules = modules;
        return;
    }

    state.packages.push(SerializedPackage {
        address: address.to_string(),
        version: 0,
        original_id: None,
        modules,
        linkage: Vec::new(),
    });
}

fn upsert_module(state: &mut PersistentState, address: &str, name: &str, bytecode_b64: String) {
    let id = format!("{}::{}", address, name);
    if let Some(existing) = state.modules.iter_mut().find(|m| m.id == id) {
        existing.bytecode_b64 = bytecode_b64;
        return;
    }
    state.modules.push(SerializedModule { id, bytecode_b64 });
}

fn upsert_object(state: &mut PersistentState, obj: SerializedObject) {
    if let Some(existing) = state
        .objects
        .iter_mut()
        .find(|o| addresses_equal(&o.id, &obj.id))
    {
        *existing = obj;
        return;
    }
    state.objects.push(obj);
}

fn split_module_id(id: &str) -> Option<(&str, &str)> {
    let (addr, name) = id.split_once("::")?;

    Some((addr, name))
}

fn normalize_addr(address: &str) -> Option<String> {
    sui_sandbox_types::normalize_address_checked(address)
}

fn addresses_equal(a: &str, b: &str) -> bool {
    match (
        AccountAddress::from_hex_literal(a),
        AccountAddress::from_hex_literal(b),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.eq_ignore_ascii_case(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_new_state() {
        let state = SandboxState::new("https://test.rpc").unwrap();
        assert!(state.loaded_packages().is_empty());
        assert_eq!(state.last_sender(), AccountAddress::ZERO);
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");

        // Create and save state
        let mut state = SandboxState::new("https://test.rpc").unwrap();

        // Add a mock package
        let addr = AccountAddress::from_hex_literal("0x123").unwrap();
        let modules = vec![("test_module".to_string(), vec![1, 2, 3, 4])];
        state.add_package(addr, modules);
        state.set_last_sender(addr);

        state.save(&path).unwrap();

        // Load and verify
        let loaded = SandboxState::load(&path, "https://test.rpc").unwrap();
        assert_eq!(loaded.loaded_packages().len(), 1);
        assert!(loaded.loaded_packages()[0].contains("0x"));
        assert_eq!(loaded.last_sender(), addr);
    }

    #[test]
    fn test_load_or_create_new() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");

        let state = SandboxState::load_or_create(&path, "https://test.rpc").unwrap();
        assert!(state.loaded_packages().is_empty());
    }
}
