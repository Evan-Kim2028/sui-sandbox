//! Session state management for sui-sandbox
//!
//! Handles persistence of sandbox state across CLI invocations.

use anyhow::{Context, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::types::parse_type_tag;
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};

/// Serializable state that can be persisted to disk
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedState {
    /// Package address -> list of (module_name, bytecode_base64)
    pub packages: HashMap<String, Vec<(String, String)>>,
    /// Object ID -> (type_tag, bcs_bytes_base64)
    pub objects: HashMap<String, (String, String)>,
    /// List of published package addresses in order
    pub published_order: Vec<String>,
    /// Last used sender address
    pub last_sender: Option<String>,
    /// Session metadata
    pub metadata: SessionMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMetadata {
    pub created_at: Option<String>,
    pub last_modified: Option<String>,
    pub rpc_url: Option<String>,
}

/// Runtime sandbox state (in-memory)
pub struct SandboxState {
    /// Module resolver with loaded packages
    pub resolver: LocalModuleResolver,
    /// Persisted state for save/load
    pub persisted: PersistedState,
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
            persisted: PersistedState {
                metadata: SessionMetadata {
                    created_at: Some(chrono::Utc::now().to_rfc3339()),
                    last_modified: None,
                    rpc_url: Some(rpc_url.to_string()),
                },
                ..Default::default()
            },
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

    /// Load state from a file
    pub fn load(path: &Path, rpc_url: &str) -> Result<Self> {
        let data = std::fs::read(path).context("Failed to read state file")?;
        let persisted: PersistedState =
            bincode::deserialize(&data).context("Failed to deserialize state")?;

        let mut resolver = LocalModuleResolver::new();
        resolver.load_sui_framework_auto()?;

        // Restore packages to resolver
        for (addr_str, modules) in &persisted.packages {
            let decoded_modules: Vec<(String, Vec<u8>)> = modules
                .iter()
                .filter_map(|(name, b64)| {
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
                        .ok()
                        .map(|bytes| (name.clone(), bytes))
                })
                .collect();

            if !decoded_modules.is_empty() {
                // Try to parse the address to get the target
                if let Ok(addr) = AccountAddress::from_hex_literal(addr_str) {
                    let _ = resolver.add_package_modules_at(decoded_modules, Some(addr));
                } else {
                    let _ = resolver.add_package_modules(decoded_modules);
                }
            }
        }

        Ok(Self {
            resolver,
            persisted,
            rpc_url: rpc_url.to_string(),
            dirty: false,
        })
    }

    /// Save state to a file
    pub fn save(&self, path: &Path) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut persisted = self.persisted.clone();
        persisted.metadata.last_modified = Some(chrono::Utc::now().to_rfc3339());

        let data = bincode::serialize(&persisted).context("Failed to serialize state")?;
        std::fs::write(path, data).context("Failed to write state file")?;

        Ok(())
    }

    /// Add a package to the state
    pub fn add_package(&mut self, address: AccountAddress, modules: Vec<(String, Vec<u8>)>) {
        let addr_str = format!("0x{}", hex::encode(address));

        // Add to resolver
        let _ = self.resolver.add_package_modules_at(
            modules
                .iter()
                .map(|(n, b)| (n.clone(), b.clone()))
                .collect(),
            Some(address),
        );

        // Add to persisted state
        let encoded_modules: Vec<(String, String)> = modules
            .into_iter()
            .map(|(name, bytes)| {
                (
                    name,
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes),
                )
            })
            .collect();

        self.persisted
            .packages
            .insert(addr_str.clone(), encoded_modules);
        if !self.persisted.published_order.contains(&addr_str) {
            self.persisted.published_order.push(addr_str);
        }
        self.dirty = true;
    }

    /// Get the last published package address
    pub fn last_published(&self) -> Option<AccountAddress> {
        self.persisted
            .published_order
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
        self.persisted.published_order.clone()
    }

    /// Get package module names
    pub fn get_package_modules(&self, address: &str) -> Option<Vec<String>> {
        // First try from persisted state
        if let Some(modules) = self.persisted.packages.get(address) {
            return Some(modules.iter().map(|(name, _)| name.clone()).collect());
        }
        // Fall back to resolver
        if let Ok(addr) = AccountAddress::from_hex_literal(address) {
            let modules = self.resolver.get_package_modules(&addr);
            if !modules.is_empty() {
                return Some(modules);
            }
        }
        None
    }

    /// Add an object to the state.
    pub fn add_object(
        &mut self,
        object_id: &str,
        type_tag: Option<&str>,
        bcs_base64: &str,
    ) -> Result<()> {
        let _addr = AccountAddress::from_hex_literal(object_id)
            .map_err(|e| anyhow::anyhow!("Invalid object ID '{}': {}", object_id, e))?;
        let stored_type = type_tag.unwrap_or_default().to_string();
        self.persisted
            .objects
            .insert(object_id.to_string(), (stored_type, bcs_base64.to_string()));
        self.dirty = true;
        Ok(())
    }

    /// Get object bytes and parsed type tag for PTB inputs.
    pub fn get_object_input(&self, object_id: &str) -> Result<(Vec<u8>, Option<TypeTag>)> {
        let (type_str, bcs_base64) = self.persisted.objects.get(object_id).ok_or_else(|| {
            anyhow::anyhow!(
                "Object {} not found in session. Run `sui-sandbox fetch object {}` first.",
                object_id,
                object_id
            )
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bcs_base64)
            .map_err(|e| anyhow::anyhow!("Invalid cached BCS for {}: {}", object_id, e))?;
        let parsed = if type_str.is_empty() {
            None
        } else {
            parse_type_tag(type_str).ok()
        };
        Ok((bytes, parsed))
    }

    /// Set last sender
    pub fn set_last_sender(&mut self, sender: AccountAddress) {
        self.persisted.last_sender = Some(format!("0x{}", hex::encode(sender)));
        self.dirty = true;
    }

    /// Get last sender or default
    #[cfg(test)]
    pub fn last_sender(&self) -> AccountAddress {
        self.persisted
            .last_sender
            .as_ref()
            .and_then(|s| AccountAddress::from_hex_literal(s).ok())
            .unwrap_or(AccountAddress::ZERO)
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
        let path = dir.path().join("state.bin");

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
        assert!(loaded.loaded_packages()[0].contains("123"));
        assert_eq!(loaded.last_sender(), addr);
    }

    #[test]
    fn test_load_or_create_new() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.bin");

        let state = SandboxState::load_or_create(&path, "https://test.rpc").unwrap();
        assert!(state.loaded_packages().is_empty());
    }
}
