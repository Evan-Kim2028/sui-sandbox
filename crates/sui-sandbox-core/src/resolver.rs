//! # Module Resolution
//!
//! This module provides abstractions for loading and resolving Move bytecode modules
//! from various sources (local files, RPC, cache).
//!
//! ## Purpose
//!
//! The module resolution system enables:
//! - **Unified loading**: Load modules from files, memory, or remote sources
//! - **Address resolution**: Map module names to their deployed addresses
//! - **Dependency tracking**: Resolve module dependencies for VM execution
//!
//! ## Key Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`ModuleProvider`] | Trait for unified module loading across sources |
//! | [`LocalModuleResolver`] | Implementation that loads from local bytecode files |
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────┐
//! │   ModuleProvider    │  ◄── Trait interface
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌─────────────────────┐
//! │ LocalModuleResolver │  ◄── File-based implementation
//! │  - modules: BTreeMap│
//! │  - bytecode cache   │
//! └─────────────────────┘
//!            │
//!            ▼
//!     CompiledModule
//! ```
//!
//! ## Usage
//!
//! ```no_run
//! use sui_sandbox_core::resolver::{ModuleProvider, LocalModuleResolver};
//! use move_core_types::language_storage::ModuleId;
//! use move_core_types::account_address::AccountAddress;
//! use move_core_types::identifier::Identifier;
//!
//! let mut resolver = LocalModuleResolver::new();
//!
//! // Load modules from bytecode (bytecode_bytes would be actual Move bytecode)
//! let bytecode_bytes: Vec<u8> = vec![]; // Placeholder
//! let modules = vec![("my_module".to_string(), bytecode_bytes)];
//! // let package_addr = resolver.load_package(modules)?;
//!
//! // Query modules
//! let module_id = ModuleId::new(AccountAddress::ONE, Identifier::new("test").unwrap());
//! if resolver.has_module(&module_id) {
//!     let bytes = resolver.get_module_bytes(&module_id);
//! }
//! ```

use anyhow::{anyhow, Context, Result};
use move_binary_format::file_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;
use move_core_types::resolver::ModuleResolver;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use sui_sandbox_types::encoding::try_base64_decode;
use tracing::{debug, info, warn};

// =============================================================================
// ModuleProvider Trait
// =============================================================================

/// Trait for unified module loading across different sources.
///
/// This trait abstracts over module loading, allowing different implementations:
/// - `LocalModuleResolver`: Loads from local bytecode files
/// - Future: RPC-backed providers, cached providers, etc.
///
/// # Example
/// ```no_run
/// use sui_sandbox_core::resolver::ModuleProvider;
/// use anyhow::Result;
///
/// fn load_modules(provider: &mut impl ModuleProvider, bytecode_bytes: Vec<u8>) -> Result<()> {
///     let modules = vec![("my_module".to_string(), bytecode_bytes)];
///     provider.load_package(modules)?;
///     Ok(())
/// }
/// ```
pub trait ModuleProvider {
    /// Load a package (collection of modules) into the provider.
    ///
    /// Returns the package address if successfully loaded.
    fn load_package(&mut self, modules: Vec<(String, Vec<u8>)>) -> Result<AccountAddress>;

    /// Load a package at a specific address.
    ///
    /// This is useful for loading packages with a known address (e.g., from mainnet).
    fn load_package_at(
        &mut self,
        modules: Vec<(String, Vec<u8>)>,
        address: AccountAddress,
    ) -> Result<AccountAddress>;

    /// Check if a module exists in this provider.
    fn has_module(&self, module_id: &ModuleId) -> bool;

    /// Get the bytecode for a module.
    fn get_module_bytes(&self, module_id: &ModuleId) -> Option<&[u8]>;

    /// List all loaded packages.
    fn list_packages(&self) -> Vec<AccountAddress>;

    /// Get the module count.
    fn module_count(&self) -> usize;
}

// =============================================================================
// LocalModuleResolver
// =============================================================================

/// Cache key for function lookups.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FunctionKey {
    package: AccountAddress,
    module: String,
    function: String,
}

/// Cached function information.
#[derive(Debug, Clone)]
struct CachedFunctionInfo {
    signature: FunctionSignature,
    is_callable: bool,
    is_entry: bool,
}

#[derive(Clone)]
pub struct LocalModuleResolver {
    modules: BTreeMap<ModuleId, CompiledModule>,
    modules_bytes: BTreeMap<ModuleId, Vec<u8>>,
    /// Address aliases: maps target address -> source address
    /// When looking up a module at target address, also try source address
    address_aliases: BTreeMap<AccountAddress, AccountAddress>,
    /// Linkage upgrades: maps original (runtime) address -> upgraded (storage) address
    linkage_upgrades: BTreeMap<AccountAddress, AccountAddress>,
    /// Cache for function signatures and visibility (thread-safe).
    /// Key: (package_addr, module_name, function_name)
    function_cache: std::sync::Arc<
        parking_lot::RwLock<std::collections::HashMap<FunctionKey, CachedFunctionInfo>>,
    >,
    /// Per-package linkage tables: storage_addr → (dep_runtime_id → dep_storage_id).
    /// Each package carries its own view of which storage addresses its dependencies
    /// should resolve to, enabling correct multi-version dependency resolution.
    per_package_linkage: std::collections::HashMap<
        AccountAddress,
        std::collections::HashMap<AccountAddress, AccountAddress>,
    >,
    /// Maps storage_addr → runtime_id for upgraded packages.
    /// Used by relocate() to detect when a module belongs to the link context package.
    package_runtime_ids: std::collections::HashMap<AccountAddress, AccountAddress>,
}

impl Default for LocalModuleResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalModuleResolver {
    pub fn new() -> Self {
        Self {
            modules: BTreeMap::new(),
            modules_bytes: BTreeMap::new(),
            address_aliases: BTreeMap::new(),
            linkage_upgrades: BTreeMap::new(),
            function_cache: std::sync::Arc::new(parking_lot::RwLock::new(
                std::collections::HashMap::new(),
            )),
            per_package_linkage: std::collections::HashMap::new(),
            package_runtime_ids: std::collections::HashMap::new(),
        }
    }

    /// Create a new resolver pre-populated with Sui framework modules (0x1, 0x2).
    /// This enables execution of code that depends on standard library and Sui framework.
    pub fn with_sui_framework() -> Result<Self> {
        let mut resolver = Self::new();
        resolver.load_sui_framework()?;
        Ok(resolver)
    }

    /// Load Sui framework modules from bundled bytecode files.
    ///
    /// The framework bytecode is bundled at compile time from `framework_bytecode/` directory.
    /// These files are BCS-serialized Vec<Vec<u8>> containing individual module bytecode.
    ///
    /// Packages loaded:
    /// - move-stdlib (0x1): Standard library (vector, option, string, etc.)
    /// - sui-framework (0x2): Sui framework (object, transfer, coin, tx_context, etc.)
    /// - sui-system (0x3): System package (validator, staking, etc.)
    ///
    /// Version: mainnet-v1.62.1 (must match Dockerfile's SUI_VERSION)
    pub fn load_sui_framework(&mut self) -> Result<usize> {
        // Bundled framework bytecode - BCS-serialized Vec<Vec<u8>>
        // Path from crates/sui-sandbox-core/src/ to project root's framework_bytecode/
        static MOVE_STDLIB: &[u8] = include_bytes!("../../../framework_bytecode/move-stdlib");
        static SUI_FRAMEWORK: &[u8] = include_bytes!("../../../framework_bytecode/sui-framework");
        static SUI_SYSTEM: &[u8] = include_bytes!("../../../framework_bytecode/sui-system");

        let mut count = 0;

        // Load each package's modules
        for (pkg_addr, package_bytes) in [
            ("0x1 (Move stdlib)", MOVE_STDLIB),
            ("0x2 (Sui framework)", SUI_FRAMEWORK),
            ("0x3 (Sui system)", SUI_SYSTEM),
        ] {
            let module_bytes_list: Vec<Vec<u8>> = bcs::from_bytes(package_bytes).map_err(|e| {
                anyhow!("Failed to deserialize embedded package {}: {}", pkg_addr, e)
            })?;

            for (idx, bytes) in module_bytes_list.into_iter().enumerate() {
                let module = CompiledModule::deserialize_with_defaults(&bytes).map_err(|e| {
                    anyhow!(
                        "Failed to deserialize module {} in package {}: {:?}",
                        idx,
                        pkg_addr,
                        e
                    )
                })?;
                let id = module.self_id();
                self.modules.insert(id.clone(), module);
                self.modules_bytes.insert(id, bytes);
                count += 1;
            }
        }

        Ok(count)
    }

    /// Load framework modules from GraphQL (fetches latest mainnet version).
    ///
    /// This fetches the framework packages (0x1, 0x2, 0x3) directly from mainnet,
    /// ensuring we have the exact modules that on-chain code expects.
    ///
    /// This is preferred over the bundled framework for historical transaction replay,
    /// as the on-chain framework may have modules not present in the bundled version.
    pub fn load_sui_framework_from_graphql(
        &mut self,
        graphql: &sui_transport::GraphQLClient,
    ) -> Result<usize> {
        let framework_packages = [
            "0x0000000000000000000000000000000000000000000000000000000000000001", // move-stdlib
            "0x0000000000000000000000000000000000000000000000000000000000000002", // sui-framework
            "0x0000000000000000000000000000000000000000000000000000000000000003", // sui-system
        ];

        let mut count = 0;

        for pkg_addr in framework_packages {
            match graphql.fetch_package(pkg_addr) {
                Ok(pkg) => {
                    for module in &pkg.modules {
                        if let Some(ref bytecode_b64) = module.bytecode_base64 {
                            if let Some(bytes) = try_base64_decode(bytecode_b64) {
                                match CompiledModule::deserialize_with_defaults(&bytes) {
                                    Ok(compiled) => {
                                        let id = compiled.self_id();
                                        self.modules.insert(id.clone(), compiled);
                                        self.modules_bytes.insert(id, bytes);
                                        count += 1;
                                    }
                                    Err(e) => {
                                        warn!(
                                            module = %module.name,
                                            error = ?e,
                                            "failed to deserialize framework module"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        package = %pkg_addr,
                        error = %e,
                        "failed to fetch framework package"
                    );
                }
            }
        }

        Ok(count)
    }

    /// Load framework from GraphQL, falling back to bundled if fetch fails.
    pub fn load_sui_framework_auto(&mut self) -> Result<usize> {
        // Try to fetch from GraphQL first for latest version
        let client = sui_transport::GraphQLClient::mainnet();
        match self.load_sui_framework_from_graphql(&client) {
            Ok(count) if count > 0 => {
                info!(
                    count = count,
                    "loaded framework modules from mainnet GraphQL"
                );
                return Ok(count);
            }
            Ok(0) => {
                debug!("GraphQL returned no modules, falling back to bundled framework");
            }
            Err(e) => {
                debug!(
                    error = %e,
                    "GraphQL fetch failed, falling back to bundled framework"
                );
            }
            _ => {}
        }

        // Fall back to bundled framework
        self.load_sui_framework()
    }

    /// Create a new resolver with framework loaded from GraphQL (or bundled fallback).
    pub fn with_sui_framework_auto() -> Result<Self> {
        let mut resolver = Self::new();
        resolver.load_sui_framework_auto()?;
        Ok(resolver)
    }

    pub fn load_from_dir(&mut self, package_dir: &Path) -> Result<usize> {
        let bytecode_dir = package_dir.join("bytecode_modules");
        if !bytecode_dir.exists() {
            // If the package_dir itself contains .mv files (e.g. helper packages), try scanning it directly
            return self.scan_dir(package_dir);
        }
        self.scan_dir(&bytecode_dir)
    }

    fn scan_dir(&mut self, dir: &Path) -> Result<usize> {
        let mut count = 0;
        let entries = fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                count += self.scan_dir(&path)?;
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("mv") {
                continue;
            }

            let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            let module = CompiledModule::deserialize_with_defaults(&bytes)
                .map_err(|e| anyhow!("deserialize {}: {}", path.display(), e))?;

            let id = module.self_id();
            // Check for duplicate modules - warn but don't fail (later module wins)
            if self.modules.contains_key(&id) {
                warn!(
                    module = %id,
                    path = %path.display(),
                    "duplicate module found, overwriting previous"
                );
            }
            self.modules.insert(id.clone(), module);
            self.modules_bytes.insert(id, bytes);
            count += 1;
        }
        Ok(count)
    }

    pub fn get_module_struct(&self, id: &ModuleId) -> Option<&CompiledModule> {
        self.get_module_with_alias(id)
    }

    pub fn get_module_by_addr_name(
        &self,
        addr: &AccountAddress,
        name: &str,
    ) -> Option<&CompiledModule> {
        let id = ModuleId::new(*addr, Identifier::new(name).ok()?);
        self.get_module_with_alias(&id)
    }

    /// Get a compiled module, checking address aliases if direct lookup fails.
    /// This is the preferred way to look up modules when the address might be
    /// a deployment address that differs from the bytecode address.
    fn get_module_with_alias(&self, id: &ModuleId) -> Option<&CompiledModule> {
        // Try direct lookup first
        if let Some(module) = self.modules.get(id) {
            return Some(module);
        }
        // Check for alias (deployment address -> bytecode address)
        if let Some(aliased_addr) = self.address_aliases.get(id.address()) {
            let aliased_id = ModuleId::new(*aliased_addr, id.name().to_owned());
            return self.modules.get(&aliased_id);
        }
        None
    }

    pub fn iter_modules(&self) -> impl Iterator<Item = &CompiledModule> {
        self.modules.values()
    }

    /// Dynamically add a module from raw bytecode.
    /// This enables loading packages fetched from the RPC at runtime.
    /// Note: Adding modules invalidates the function cache for consistency.
    pub fn add_module_bytes(&mut self, bytes: Vec<u8>) -> Result<ModuleId> {
        let module = CompiledModule::deserialize_with_defaults(&bytes)
            .map_err(|e| anyhow!("failed to deserialize module: {:?}", e))?;
        let id = module.self_id();
        self.modules.insert(id.clone(), module);
        self.modules_bytes.insert(id.clone(), bytes);
        // Clear function cache for this package to ensure consistency
        self.invalidate_package_cache(id.address());
        Ok(id)
    }

    /// Invalidate cached function info for a specific package.
    fn invalidate_package_cache(&self, package_addr: &AccountAddress) {
        self.function_cache
            .write()
            .retain(|k, _| k.package != *package_addr);
    }

    /// Clear the entire function cache.
    pub fn clear_function_cache(&self) {
        self.function_cache.write().clear();
    }

    /// Get statistics about the function cache.
    pub fn function_cache_stats(&self) -> (usize, usize) {
        let cache = self.function_cache.read();
        let total = cache.len();
        let callable = cache.values().filter(|v| v.is_callable).count();
        (total, callable)
    }

    /// Dynamically add multiple modules (e.g., from a package).
    /// Returns the number of modules successfully loaded.
    /// Add package modules and return (module_count, package_address).
    /// The package_address is extracted from the first module's bytecode.
    pub fn add_package_modules(
        &mut self,
        modules: Vec<(String, Vec<u8>)>,
    ) -> Result<(usize, Option<AccountAddress>)> {
        self.add_package_modules_at(modules, None)
    }

    /// Dynamically add multiple modules with an optional target address.
    /// If target_addr is Some, modules will be aliased from their bytecode address
    /// to the target address, enabling package upgrade support.
    /// Returns (module_count, source_address_from_bytecode).
    pub fn add_package_modules_at(
        &mut self,
        modules: Vec<(String, Vec<u8>)>,
        target_addr: Option<AccountAddress>,
    ) -> Result<(usize, Option<AccountAddress>)> {
        let mut count = 0;
        let mut source_addr: Option<AccountAddress> = None;

        for (name, bytes) in modules {
            if bytes.is_empty() {
                // Skip modules with no bytecode (informational only)
                continue;
            }
            match self.add_module_bytes(bytes) {
                Ok(id) => {
                    count += 1;
                    // Track the source address from bytecode
                    if source_addr.is_none() {
                        source_addr = Some(*id.address());
                    }
                }
                Err(e) => {
                    warn!(module = %name, error = %e, "failed to load module");
                }
            }
        }

        // Set up address alias if target differs from source
        if let (Some(target), Some(source)) = (target_addr, source_addr) {
            if target != source {
                self.address_aliases.insert(target, source);
                // Track original -> upgraded mapping for LinkageResolver
                self.linkage_upgrades.insert(source, target);
            }
        }

        Ok((count, source_addr))
    }

    /// Check if a module is loaded.
    /// Also checks storage→runtime mapping for upgraded packages.
    pub fn has_module(&self, id: &ModuleId) -> bool {
        if self.modules.contains_key(id) {
            return true;
        }
        // For upgraded packages, the requested address may be a storage address
        // but modules are stored under the runtime_id (bytecode self-address).
        if let Some(runtime_id) = self.package_runtime_ids.get(id.address()) {
            if runtime_id != id.address() {
                let runtime_mod = ModuleId::new(*runtime_id, id.name().to_owned());
                return self.modules.contains_key(&runtime_mod);
            }
        }
        false
    }

    /// Check if a package (any module at this address) is loaded.
    pub fn has_package(&self, addr: &AccountAddress) -> bool {
        self.modules.keys().any(|id| id.address() == addr)
    }

    /// List all unique package addresses that have been loaded.
    pub fn list_packages(&self) -> Vec<AccountAddress> {
        use std::collections::BTreeSet;
        let addrs: BTreeSet<AccountAddress> = self.modules.keys().map(|id| *id.address()).collect();
        addrs.into_iter().collect()
    }

    /// Get module names for a specific package.
    pub fn get_package_modules(&self, package_addr: &AccountAddress) -> Vec<String> {
        self.modules
            .keys()
            .filter(|id| id.address() == package_addr)
            .map(|id| id.name().to_string())
            .collect()
    }

    /// Get the number of loaded modules.
    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// Get an iterator over all compiled modules.
    /// Useful for scanning modules for constants (e.g., version numbers).
    pub fn compiled_modules(&self) -> impl Iterator<Item = &CompiledModule> {
        self.modules.values()
    }

    /// Get all unique package addresses that are referenced by loaded modules
    /// but not yet loaded themselves. This is useful for fetching transitive dependencies.
    ///
    /// Returns a set of package addresses (not module IDs) that need to be fetched.
    pub fn get_missing_dependencies(&self) -> std::collections::BTreeSet<AccountAddress> {
        use std::collections::BTreeSet;

        // Framework addresses that we always have bundled
        let framework_addrs: BTreeSet<AccountAddress> =
            sui_sandbox_types::framework::FRAMEWORK_ADDRESSES
                .into_iter()
                .collect();

        let mut missing = BTreeSet::new();

        // Check each loaded module for its dependencies
        for module in self.modules.values() {
            // Get all module handles - these are references to other modules
            for handle in &module.module_handles {
                let addr = *module.address_identifier_at(handle.address);
                let name = module.identifier_at(handle.name);
                let dep_id = ModuleId::new(addr, name.to_owned());

                // Skip if it's a framework module or already loaded
                if framework_addrs.contains(&addr) {
                    continue;
                }
                if self.modules.contains_key(&dep_id) {
                    continue;
                }

                // This module is missing - add its package address
                missing.insert(addr);
            }
        }

        missing
    }

    /// Get all loaded package addresses (unique addresses of loaded modules).
    pub fn loaded_packages(&self) -> std::collections::BTreeSet<AccountAddress> {
        self.modules.keys().map(|id| *id.address()).collect()
    }

    /// Get the source address for an aliased address, if any.
    /// This is used for address relocation during module loading.
    pub fn get_alias(&self, target: &AccountAddress) -> Option<AccountAddress> {
        self.address_aliases.get(target).copied()
    }

    /// Add an address alias: when looking up modules at `target`, also try `source`.
    /// This is used for package upgrade linkage where the original package ID
    /// should resolve to modules in the upgraded package.
    pub fn add_address_alias(&mut self, target: AccountAddress, source: AccountAddress) {
        if target != source {
            self.address_aliases.insert(target, source);
        }
    }

    /// Register a linkage upgrade mapping (original -> upgraded).
    pub fn add_linkage_upgrade(&mut self, original: AccountAddress, upgraded: AccountAddress) {
        if original != upgraded {
            self.linkage_upgrades.insert(original, upgraded);
        }
    }

    /// Register multiple linkage upgrade mappings (original -> upgraded).
    pub fn add_linkage_upgrades<I>(&mut self, upgrades: I)
    where
        I: IntoIterator<Item = (AccountAddress, AccountAddress)>,
    {
        for (original, upgraded) in upgrades {
            self.add_linkage_upgrade(original, upgraded);
        }
    }

    /// Get the upgraded storage address for an original (runtime) address, if any.
    pub fn get_linkage_upgrade(&self, original: &AccountAddress) -> Option<AccountAddress> {
        self.linkage_upgrades.get(original).copied()
    }

    /// Register a package's linkage table for per-package dependency resolution.
    /// `storage_addr` is the on-chain address where the package bytes live.
    /// `runtime_id` is the package's original/bytecode address.
    /// `linkage` maps dep_runtime_id → dep_storage_id for this package's dependencies.
    pub fn add_package_linkage(
        &mut self,
        storage_addr: AccountAddress,
        runtime_id: AccountAddress,
        linkage: &std::collections::HashMap<AccountAddress, AccountAddress>,
    ) {
        if std::env::var("SANDBOX_TRACE_RELOCATE").is_ok() {
            eprintln!(
                "[add_pkg_linkage] storage={:#x} runtime={:#x} entries={}",
                storage_addr,
                runtime_id,
                linkage.len()
            );
            for (dep_runtime, dep_storage) in linkage {
                eprintln!("  linkage: {:#x} -> {:#x}", dep_runtime, dep_storage);
            }
        }
        self.package_runtime_ids.insert(storage_addr, runtime_id);
        if !linkage.is_empty() {
            self.per_package_linkage
                .insert(storage_addr, linkage.clone());
        }
    }

    /// Look up a dependency's storage address for a specific calling package.
    pub fn get_per_package_linkage(
        &self,
        link_context: &AccountAddress,
        dep_runtime_id: &AccountAddress,
    ) -> Option<AccountAddress> {
        let result = self
            .per_package_linkage
            .get(link_context)
            .and_then(|table| table.get(dep_runtime_id).copied());
        if std::env::var("SANDBOX_TRACE_RELOCATE").is_ok()
            && result.is_none()
            && link_context != &AccountAddress::ZERO
        {
            let has_table = self.per_package_linkage.contains_key(link_context);
            eprintln!(
                "[per_pkg_linkage] ctx={:#x} dep={:#x} -> None (has_table={})",
                link_context, dep_runtime_id, has_table
            );
        }
        result
    }

    /// Get the runtime ID for a package at the given storage address.
    pub fn get_link_context_runtime_id(
        &self,
        storage_addr: &AccountAddress,
    ) -> Option<AccountAddress> {
        self.package_runtime_ids.get(storage_addr).copied()
    }

    /// Search ALL registered packages' linkage tables for a dependency mapping.
    ///
    /// This handles transitive dependency resolution: when the link context package
    /// (A) loads a dependency (B), and B's bytecode references B's own dependencies
    /// using runtime addresses, A's linkage table may not contain those mappings.
    /// In that case, we search B's linkage table (and all other packages') to find
    /// the correct storage address.
    ///
    /// Returns the first matching storage address found across any package's linkage.
    pub fn find_in_any_package_linkage(
        &self,
        dep_runtime_id: &AccountAddress,
    ) -> Option<AccountAddress> {
        for table in self.per_package_linkage.values() {
            if let Some(storage_addr) = table.get(dep_runtime_id) {
                return Some(*storage_addr);
            }
        }
        None
    }

    /// Import address aliases from a PackageUpgradeResolver.
    ///
    /// This synchronizes the resolver's internal alias map with the comprehensive
    /// bidirectional mappings from PackageUpgradeResolver. The direction is:
    /// storage_id -> original_id (bytecode address).
    ///
    /// Note: This creates aliases in the OPPOSITE direction from `add_address_alias`
    /// because PackageUpgradeResolver maps storage->original while we need
    /// deployment_addr->bytecode_addr for module lookup.
    pub fn import_upgrade_aliases(
        &mut self,
        upgrade_resolver: &sui_resolver::package_upgrades::PackageUpgradeResolver,
    ) {
        // Track original -> storage upgrades for LinkageResolver
        for (original_id, storage_id) in upgrade_resolver.all_upgrades() {
            if let (Ok(original_addr), Ok(storage_addr)) = (
                AccountAddress::from_hex_literal(original_id),
                AccountAddress::from_hex_literal(storage_id),
            ) {
                if original_addr != storage_addr {
                    self.linkage_upgrades.insert(original_addr, storage_addr);
                }
            }
        }
        for (storage_id, original_id) in upgrade_resolver.all_storage_to_original() {
            if let (Ok(storage_addr), Ok(original_addr)) = (
                AccountAddress::from_hex_literal(storage_id),
                AccountAddress::from_hex_literal(original_id),
            ) {
                // storage_id is where bytecode is fetched from (on-chain)
                // original_id is what's in the bytecode self-address
                // For module lookup: we look up by original_id (from type tags/calls)
                // and need to find modules stored at original_id
                // Actually the mapping should be: if someone asks for storage_id,
                // redirect to original_id where modules are stored
                if storage_addr != original_addr {
                    self.address_aliases.insert(storage_addr, original_addr);
                }
            }
        }
    }

    /// Get all address aliases as a HashMap for use with VMHarness and ObjectRuntime.
    pub fn get_all_aliases(&self) -> std::collections::HashMap<AccountAddress, AccountAddress> {
        self.address_aliases.iter().map(|(k, v)| (*k, *v)).collect()
    }

    /// Validate that all aliased addresses have modules loaded.
    ///
    /// Returns a list of (target_addr, source_addr) pairs where the source
    /// address has no modules loaded. This helps catch configuration issues
    /// where aliases are set up but the actual modules weren't loaded.
    pub fn validate_aliases(&self) -> Vec<(AccountAddress, AccountAddress)> {
        let mut missing = Vec::new();

        for (target, source) in &self.address_aliases {
            // Check if any module exists at the source address
            let has_modules = self.modules.keys().any(|id| id.address() == source);

            if !has_modules {
                missing.push((*target, *source));
            }
        }

        missing
    }

    /// Validate aliases and log warnings for any that point to missing modules.
    pub fn validate_aliases_with_warnings(&self) {
        let missing = self.validate_aliases();
        for (target, source) in missing {
            warn!(
                target = %target.to_hex_literal(),
                source = %source.to_hex_literal(),
                "address alias points to missing modules"
            );
        }
    }

    /// Log unresolved function or datatype handles for loaded modules.
    ///
    /// This is useful when verifier errors report LOOKUP_FAILED for
    /// function/datatype handles, which often indicates a dependency
    /// package version mismatch.
    pub fn log_unresolved_member_handles(&self) {
        for module in self.modules.values() {
            let module_id = module.self_id();

            for (idx, handle) in module.function_handles.iter().enumerate() {
                let mod_handle = &module.module_handles[handle.module.0 as usize];
                let dep_addr = *module.address_identifier_at(mod_handle.address);
                let dep_name = module.identifier_at(mod_handle.name);
                let dep_id = ModuleId::new(dep_addr, dep_name.to_owned());
                let dep_module = self.get_module_with_alias(&dep_id);
                if let Some(dep_module) = dep_module {
                    let func_name = module.identifier_at(handle.name);
                    let mut found = false;
                    for def in dep_module.function_defs() {
                        let def_handle = dep_module.function_handle_at(def.function);
                        let def_name = dep_module.identifier_at(def_handle.name);
                        if def_name == func_name {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        eprintln!(
                            "[linkage] missing_function {}::{} referenced by {} (fh#{})",
                            dep_id.address().to_hex_literal(),
                            func_name,
                            module_id,
                            idx
                        );
                    }
                } else {
                    eprintln!(
                        "[linkage] missing_module {} referenced by {} (fh#{})",
                        dep_id, module_id, idx
                    );
                }
            }

            for (idx, handle) in module.datatype_handles.iter().enumerate() {
                let mod_handle = &module.module_handles[handle.module.0 as usize];
                let dep_addr = *module.address_identifier_at(mod_handle.address);
                let dep_name = module.identifier_at(mod_handle.name);
                let dep_id = ModuleId::new(dep_addr, dep_name.to_owned());
                let dep_module = self.get_module_with_alias(&dep_id);
                if let Some(dep_module) = dep_module {
                    let struct_name = module.identifier_at(handle.name);
                    let mut found = false;
                    for def in dep_module.struct_defs() {
                        let def_handle = &dep_module.datatype_handles[def.struct_handle.0 as usize];
                        let def_name = dep_module.identifier_at(def_handle.name);
                        if def_name == struct_name {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        eprintln!(
                            "[linkage] missing_struct {}::{} referenced by {} (dt#{})",
                            dep_id.address().to_hex_literal(),
                            struct_name,
                            module_id,
                            idx
                        );
                    }
                } else {
                    eprintln!(
                        "[linkage] missing_module {} referenced by {} (dt#{})",
                        dep_id, module_id, idx
                    );
                }
            }
        }
    }

    // ========================================================================
    // LLM Agent Tools - Introspection and search methods
    // ========================================================================

    /// List all loaded module paths (e.g., "0x2::coin").
    pub fn list_modules(&self) -> Vec<String> {
        self.modules
            .keys()
            .map(|id| format!("{}::{}", id.address().to_hex_literal(), id.name()))
            .collect()
    }

    /// List all functions in a module by path (e.g., "0x2::coin").
    pub fn list_functions(&self, module_path: &str) -> Option<Vec<String>> {
        let (addr, name) = Self::parse_module_path(module_path)?;
        let id = ModuleId::new(addr, Identifier::new(name).ok()?);
        let module = self.get_module_with_alias(&id)?;

        let functions: Vec<String> = module
            .function_defs
            .iter()
            .map(|def| {
                let handle = &module.function_handles[def.function.0 as usize];
                module.identifier_at(handle.name).to_string()
            })
            .collect();
        Some(functions)
    }

    /// List all structs in a module by path.
    pub fn list_structs(&self, module_path: &str) -> Option<Vec<String>> {
        let (addr, name) = Self::parse_module_path(module_path)?;
        let id = ModuleId::new(addr, Identifier::new(name).ok()?);
        let module = self.get_module_with_alias(&id)?;

        let structs: Vec<String> = module
            .struct_defs
            .iter()
            .map(|def| {
                let handle = &module.datatype_handles[def.struct_handle.0 as usize];
                module.identifier_at(handle.name).to_string()
            })
            .collect();
        Some(structs)
    }

    /// Get detailed function information.
    pub fn get_function_info(
        &self,
        module_path: &str,
        function_name: &str,
    ) -> Option<serde_json::Value> {
        let (addr, mod_name) = Self::parse_module_path(module_path)?;
        let id = ModuleId::new(addr, Identifier::new(mod_name).ok()?);
        let module = self.get_module_with_alias(&id)?;

        for def in &module.function_defs {
            let handle = &module.function_handles[def.function.0 as usize];
            let name = module.identifier_at(handle.name).to_string();
            if name == function_name {
                // Get visibility
                let visibility = match def.visibility {
                    move_binary_format::file_format::Visibility::Private => "private",
                    move_binary_format::file_format::Visibility::Public => "public",
                    move_binary_format::file_format::Visibility::Friend => "friend",
                };

                // Get is_entry
                let is_entry = def.is_entry;

                // Get parameters
                let params_sig = &module.signatures[handle.parameters.0 as usize];
                let params: Vec<String> = params_sig
                    .0
                    .iter()
                    .map(|t| format_signature_token(module, t))
                    .collect();

                // Get return types
                let return_sig = &module.signatures[handle.return_.0 as usize];
                let returns: Vec<String> = return_sig
                    .0
                    .iter()
                    .map(|t| format_signature_token(module, t))
                    .collect();

                // Get type parameters
                let type_params: Vec<serde_json::Value> = handle
                    .type_parameters
                    .iter()
                    .enumerate()
                    .map(|(i, param)| {
                        serde_json::json!({
                            "name": format!("T{}", i),
                            "constraints": get_abilities_list(*param),
                        })
                    })
                    .collect();

                return Some(serde_json::json!({
                    "path": format!("{}::{}", module_path, function_name),
                    "visibility": visibility,
                    "is_entry": is_entry,
                    "params": params,
                    "returns": returns,
                    "type_params": type_params,
                }));
            }
        }
        None
    }

    /// Find all functions that return a given type (constructors).
    pub fn find_constructors(&self, type_path: &str) -> Vec<serde_json::Value> {
        let mut results = Vec::new();
        let type_lower = type_path.to_lowercase();

        // Extract type name (last component after ::)
        let type_name = type_path.rsplit("::").next().unwrap_or(type_path);
        let type_name_lower = type_name.to_lowercase();

        for (id, module) in &self.modules {
            let mod_path = format!("{}::{}", id.address().to_hex_literal(), id.name());

            for def in &module.function_defs {
                let handle = &module.function_handles[def.function.0 as usize];
                let fn_name = module.identifier_at(handle.name).to_string();

                // Get return types
                let return_sig = &module.signatures[handle.return_.0 as usize];
                for ret in &return_sig.0 {
                    let ret_str = format_signature_token(module, ret);
                    let ret_lower = ret_str.to_lowercase();

                    // Check if return type matches (contains the type name)
                    if ret_lower.contains(&type_name_lower) || ret_lower.contains(&type_lower) {
                        // Get visibility
                        let visibility = match def.visibility {
                            move_binary_format::file_format::Visibility::Private => "private",
                            move_binary_format::file_format::Visibility::Public => "public",
                            move_binary_format::file_format::Visibility::Friend => "friend",
                        };

                        // Get parameters
                        let params_sig = &module.signatures[handle.parameters.0 as usize];
                        let params: Vec<String> = params_sig
                            .0
                            .iter()
                            .map(|t| format_signature_token(module, t))
                            .collect();

                        results.push(serde_json::json!({
                            "function": format!("{}::{}", mod_path, fn_name),
                            "visibility": visibility,
                            "is_entry": def.is_entry,
                            "params": params,
                            "returns": ret_str,
                        }));
                        break; // Only add once per function
                    }
                }
            }
        }
        results
    }

    /// Search for types matching a pattern.
    pub fn search_types(
        &self,
        pattern: &str,
        ability_filter: Option<&str>,
    ) -> Vec<serde_json::Value> {
        let mut results = Vec::new();
        let pattern_lower = pattern.to_lowercase();

        for (id, module) in &self.modules {
            let mod_path = format!("{}::{}", id.address().to_hex_literal(), id.name());

            for def in &module.struct_defs {
                let handle = &module.datatype_handles[def.struct_handle.0 as usize];
                let struct_name = module.identifier_at(handle.name).to_string();
                let full_path = format!("{}::{}", mod_path, struct_name);
                let full_lower = full_path.to_lowercase();

                // Check pattern match (simple wildcard support)
                let matches = if pattern_lower.contains('*') {
                    let parts: Vec<&str> = pattern_lower.split('*').collect();
                    let mut pos = 0;
                    let mut matched = true;
                    for part in parts {
                        if part.is_empty() {
                            continue;
                        }
                        if let Some(found) = full_lower[pos..].find(part) {
                            pos += found + part.len();
                        } else {
                            matched = false;
                            break;
                        }
                    }
                    matched
                } else {
                    full_lower.contains(&pattern_lower)
                };

                if matches {
                    let abilities = get_abilities_list(handle.abilities);

                    // Check ability filter
                    if let Some(filter) = ability_filter {
                        if !abilities
                            .iter()
                            .any(|a| a.to_lowercase() == filter.to_lowercase())
                        {
                            continue;
                        }
                    }

                    // Get field count
                    let field_count = match &def.field_information {
                        move_binary_format::file_format::StructFieldInformation::Declared(
                            fields,
                        ) => fields.len(),
                        _ => 0,
                    };

                    results.push(serde_json::json!({
                        "type_path": full_path,
                        "abilities": abilities,
                        "is_object": abilities.contains(&"key".to_string()),
                        "field_count": field_count,
                    }));
                }
            }
        }
        results
    }

    /// Search for functions matching a pattern.
    pub fn search_functions(&self, pattern: &str, entry_only: bool) -> Vec<serde_json::Value> {
        let mut results = Vec::new();
        let pattern_lower = pattern.to_lowercase();

        for (id, module) in &self.modules {
            let mod_path = format!("{}::{}", id.address().to_hex_literal(), id.name());

            for def in &module.function_defs {
                let handle = &module.function_handles[def.function.0 as usize];
                let fn_name = module.identifier_at(handle.name).to_string();
                let full_path = format!("{}::{}", mod_path, fn_name);
                let full_lower = full_path.to_lowercase();

                // Check pattern match
                let matches = if pattern_lower.contains('*') {
                    let parts: Vec<&str> = pattern_lower.split('*').collect();
                    let mut pos = 0;
                    let mut matched = true;
                    for part in parts {
                        if part.is_empty() {
                            continue;
                        }
                        if let Some(found) = full_lower[pos..].find(part) {
                            pos += found + part.len();
                        } else {
                            matched = false;
                            break;
                        }
                    }
                    matched
                } else {
                    full_lower.contains(&pattern_lower)
                };

                if matches {
                    // Check entry_only filter
                    if entry_only && !def.is_entry {
                        continue;
                    }

                    let visibility = match def.visibility {
                        move_binary_format::file_format::Visibility::Private => "private",
                        move_binary_format::file_format::Visibility::Public => "public",
                        move_binary_format::file_format::Visibility::Friend => "friend",
                    };

                    let params_sig = &module.signatures[handle.parameters.0 as usize];
                    let params: Vec<String> = params_sig
                        .0
                        .iter()
                        .map(|t| format_signature_token(module, t))
                        .collect();

                    let return_sig = &module.signatures[handle.return_.0 as usize];
                    let returns: Vec<String> = return_sig
                        .0
                        .iter()
                        .map(|t| format_signature_token(module, t))
                        .collect();

                    results.push(serde_json::json!({
                        "path": full_path,
                        "visibility": visibility,
                        "is_entry": def.is_entry,
                        "params": params,
                        "returns": returns,
                    }));
                }
            }
        }
        results
    }

    /// Disassemble a function to bytecode (simplified output).
    pub fn disassemble_function(&self, module_path: &str, function_name: &str) -> Option<String> {
        let (addr, mod_name) = Self::parse_module_path(module_path)?;
        let id = ModuleId::new(addr, Identifier::new(mod_name).ok()?);
        let module = self.get_module_with_alias(&id)?;

        for def in &module.function_defs {
            let handle = &module.function_handles[def.function.0 as usize];
            let name = module.identifier_at(handle.name).to_string();
            if name == function_name {
                // Get basic info
                let visibility = match def.visibility {
                    move_binary_format::file_format::Visibility::Private => "private",
                    move_binary_format::file_format::Visibility::Public => "public",
                    move_binary_format::file_format::Visibility::Friend => "friend",
                };

                let params_sig = &module.signatures[handle.parameters.0 as usize];
                let params: Vec<String> = params_sig
                    .0
                    .iter()
                    .map(|t| format_signature_token(module, t))
                    .collect();

                let return_sig = &module.signatures[handle.return_.0 as usize];
                let returns: Vec<String> = return_sig
                    .0
                    .iter()
                    .map(|t| format_signature_token(module, t))
                    .collect();

                let mut output = format!(
                    "{} fun {}({}) -> ({})",
                    visibility,
                    function_name,
                    params.join(", "),
                    returns.join(", ")
                );

                // Add bytecode info if available
                if let Some(code) = &def.code {
                    output.push_str(&format!("\n  locals: {}", code.locals.0));
                    output.push_str(&format!("\n  bytecode_len: {}", code.code.len()));
                }

                return Some(output);
            }
        }
        None
    }

    /// Parse module path like "0x2::coin" into (address, module_name).
    fn parse_module_path(path: &str) -> Option<(AccountAddress, &str)> {
        let parts: Vec<&str> = path.split("::").collect();
        if parts.len() != 2 {
            return None;
        }
        let addr = AccountAddress::from_hex_literal(parts[0]).ok()?;
        Some((addr, parts[1]))
    }

    /// Get struct definitions from a specific module.
    /// Returns a map of struct_name -> StructInfo.
    pub fn get_module_structs(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
    ) -> Option<Vec<(String, StructInfo)>> {
        let id = ModuleId::new(*package_addr, Identifier::new(module_name).ok()?);
        let module = self.get_module_with_alias(&id)?;

        let mut structs = Vec::new();

        for struct_def in &module.struct_defs {
            let datatype_handle = &module.datatype_handles[struct_def.struct_handle.0 as usize];
            let struct_name = module.identifier_at(datatype_handle.name).to_string();

            // Get abilities
            let abilities = get_abilities_list(datatype_handle.abilities);

            // Get type parameters
            let type_params: Vec<TypeParamInfo> = datatype_handle
                .type_parameters
                .iter()
                .enumerate()
                .map(|(i, param)| TypeParamInfo {
                    name: format!("T{}", i),
                    constraints: get_abilities_list(param.constraints),
                })
                .collect();

            // Get fields
            let fields = match &struct_def.field_information {
                move_binary_format::file_format::StructFieldInformation::Declared(field_defs) => {
                    field_defs
                        .iter()
                        .map(|field| {
                            let field_name = module.identifier_at(field.name).to_string();
                            let field_type = format_signature_token(module, &field.signature.0);
                            FieldInfo {
                                name: field_name,
                                field_type,
                            }
                        })
                        .collect()
                }
                move_binary_format::file_format::StructFieldInformation::Native => Vec::new(),
            };

            structs.push((
                struct_name,
                StructInfo {
                    abilities,
                    type_params,
                    fields,
                },
            ));
        }

        Some(structs)
    }

    /// Populate the function cache for a given function if not already cached.
    /// Returns true if the function was found and cached.
    fn ensure_function_cached(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> bool {
        let key = FunctionKey {
            package: *package_addr,
            module: module_name.to_string(),
            function: function_name.to_string(),
        };

        // Check if already cached
        if self.function_cache.read().contains_key(&key) {
            return true;
        }

        // Look up the function in bytecode
        let id = match Identifier::new(module_name) {
            Ok(ident) => ModuleId::new(*package_addr, ident),
            Err(_) => return false,
        };
        let module = match self.get_module_with_alias(&id) {
            Some(m) => m,
            None => return false,
        };

        for def in &module.function_defs {
            let handle = &module.function_handles[def.function.0 as usize];
            let fn_name = module.identifier_at(handle.name).to_string();
            if fn_name == function_name {
                let return_sig = &module.signatures[handle.return_.0 as usize];
                let is_public = matches!(
                    def.visibility,
                    move_binary_format::file_format::Visibility::Public
                );
                let is_entry = def.is_entry;

                let params_sig = &module.signatures[handle.parameters.0 as usize];
                let info = CachedFunctionInfo {
                    signature: FunctionSignature {
                        type_param_count: handle.type_parameters.len(),
                        parameter_types: params_sig.0.clone(),
                        return_types: return_sig.0.clone(),
                    },
                    is_callable: is_public || is_entry,
                    is_entry,
                };

                self.function_cache.write().insert(key, info);
                return true;
            }
        }
        false
    }

    /// Get a function's signature from the bytecode.
    ///
    /// This is used for type resolution - to know the return types of a function
    /// BEFORE calling it, enabling proper type tracking in PTB execution.
    ///
    /// Results are cached for performance on repeated lookups.
    pub fn get_function_signature(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> Option<FunctionSignature> {
        let key = FunctionKey {
            package: *package_addr,
            module: module_name.to_string(),
            function: function_name.to_string(),
        };

        // Try cache first
        if let Some(info) = self.function_cache.read().get(&key) {
            return Some(info.signature.clone());
        }

        // Populate cache and return
        if self.ensure_function_cached(package_addr, module_name, function_name) {
            self.function_cache
                .read()
                .get(&key)
                .map(|info| info.signature.clone())
        } else {
            None
        }
    }

    /// Check if a function is callable from a PTB (i.e., is public or entry).
    ///
    /// Returns `Ok(())` if the function is callable, or an error describing why not.
    /// This validation prevents calling private/friend functions that would fail
    /// on the real Sui network.
    ///
    /// Results are cached for performance on repeated lookups.
    ///
    /// # Visibility Rules
    /// - `public` functions: Always callable from PTBs
    /// - `entry` functions: Callable from PTBs (this is their purpose)
    /// - `friend` functions: NOT callable from PTBs (only from friend modules)
    /// - `private` functions: NOT callable from PTBs (only from same module)
    pub fn check_function_callable(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> Result<()> {
        let key = FunctionKey {
            package: *package_addr,
            module: module_name.to_string(),
            function: function_name.to_string(),
        };

        // Check cache first
        if let Some(info) = self.function_cache.read().get(&key) {
            if info.is_callable {
                return Ok(());
            } else {
                return Err(anyhow!(
                    "Function {}::{}::{} is not callable from a PTB. \
                     Only public or entry functions can be called directly.",
                    package_addr.to_hex_literal(),
                    module_name,
                    function_name
                ));
            }
        }

        // Populate cache
        if self.ensure_function_cached(package_addr, module_name, function_name) {
            // Now check the cached result
            let cache = self.function_cache.read();
            if let Some(info) = cache.get(&key) {
                if info.is_callable {
                    return Ok(());
                } else {
                    return Err(anyhow!(
                        "Function {}::{}::{} is not callable from a PTB. \
                         Only public or entry functions can be called directly.",
                        package_addr.to_hex_literal(),
                        module_name,
                        function_name
                    ));
                }
            }
        }

        // Function not found
        Err(anyhow!(
            "Function '{}' not found in module {}::{}",
            function_name,
            package_addr.to_hex_literal(),
            module_name
        ))
    }

    /// Check if a function is an entry function.
    ///
    /// Entry functions are the primary way to invoke Move code from transactions.
    /// They have special rules:
    /// - Cannot return values that need to be used (results are dropped)
    /// - Can accept special types like TxContext, Clock, etc.
    ///
    /// Results are cached for performance.
    pub fn is_entry_function(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> bool {
        let key = FunctionKey {
            package: *package_addr,
            module: module_name.to_string(),
            function: function_name.to_string(),
        };

        // Check cache first
        if let Some(info) = self.function_cache.read().get(&key) {
            return info.is_entry;
        }

        // Populate cache and check
        if self.ensure_function_cached(package_addr, module_name, function_name) {
            self.function_cache
                .read()
                .get(&key)
                .map(|info| info.is_entry)
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// Check that a public non-entry function's return types don't contain references.
    ///
    /// In Sui/Move, public non-entry functions cannot return references because
    /// references cannot escape the transaction boundary. Entry functions have
    /// special handling and are allowed to return references (they're dropped).
    ///
    /// This matches Sui client behavior at `execution.rs:check_non_entry_signature`.
    pub fn check_no_reference_returns(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> Result<()> {
        let id = ModuleId::new(
            *package_addr,
            Identifier::new(module_name)
                .map_err(|e| anyhow!("Invalid module name '{}': {}", module_name, e))?,
        );

        let module = self.get_module_with_alias(&id).ok_or_else(|| {
            anyhow!(
                "Module {}::{} not found",
                package_addr.to_hex_literal(),
                module_name
            )
        })?;

        // Find the function
        for def in &module.function_defs {
            let handle = &module.function_handles[def.function.0 as usize];
            let fn_name = module.identifier_at(handle.name).to_string();
            if fn_name == function_name {
                // Entry functions are exempt - they have special handling
                if def.is_entry {
                    return Ok(());
                }

                // For public non-entry functions, check return types for references
                let return_sig = &module.signatures[handle.return_.0 as usize];
                for (i, token) in return_sig.0.iter().enumerate() {
                    if Self::contains_reference(token) {
                        let type_str = format_signature_token(module, token);
                        return Err(anyhow!(
                            "Function {}::{}::{} has invalid return type at position {}: '{}'. \
                             Public non-entry functions cannot return references. \
                             References cannot escape the transaction boundary.",
                            package_addr.to_hex_literal(),
                            module_name,
                            function_name,
                            i,
                            type_str
                        ));
                    }
                }

                return Ok(());
            }
        }

        // Function not found - let other validation handle this
        Ok(())
    }

    /// Check if a signature token contains a reference (including nested).
    fn contains_reference(token: &move_binary_format::file_format::SignatureToken) -> bool {
        use move_binary_format::file_format::SignatureToken;

        match token {
            SignatureToken::Reference(_) | SignatureToken::MutableReference(_) => true,
            SignatureToken::Vector(inner) => Self::contains_reference(inner),
            SignatureToken::Datatype(_) => false,
            SignatureToken::DatatypeInstantiation(inst) => {
                let (_, type_args) = inst.as_ref();
                type_args.iter().any(Self::contains_reference)
            }
            _ => false,
        }
    }

    /// Validate type arguments for a function call.
    ///
    /// Checks:
    /// 1. Correct number of type arguments
    /// 2. Each type argument satisfies the ability constraints of its type parameter
    ///
    /// # Returns
    /// `Ok(())` if valid, or an error describing the constraint violation.
    pub fn validate_type_args(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
        type_args: &[TypeTag],
    ) -> Result<()> {
        let id = ModuleId::new(
            *package_addr,
            Identifier::new(module_name)
                .map_err(|e| anyhow!("Invalid module name '{}': {}", module_name, e))?,
        );

        // Try direct lookup first, then check for alias
        let module = self.get_module_with_alias(&id).ok_or_else(|| {
            anyhow!(
                "Module {}::{} not found",
                package_addr.to_hex_literal(),
                module_name
            )
        })?;

        // Find the function
        for def in &module.function_defs {
            let handle = &module.function_handles[def.function.0 as usize];
            let fn_name = module.identifier_at(handle.name).to_string();
            if fn_name == function_name {
                // Check type argument count
                let expected = handle.type_parameters.len();
                let provided = type_args.len();
                if expected != provided {
                    return Err(anyhow!(
                        "Function {}::{}::{} expects {} type argument(s), but {} provided",
                        package_addr.to_hex_literal(),
                        module_name,
                        function_name,
                        expected,
                        provided
                    ));
                }

                // Check ability constraints for each type argument
                for (i, (type_arg, constraint)) in type_args
                    .iter()
                    .zip(handle.type_parameters.iter())
                    .enumerate()
                {
                    if let Err(msg) = self.check_type_satisfies_constraints(type_arg, constraint) {
                        return Err(anyhow!(
                            "Type argument {} of {}::{}::{} violates constraints: {}",
                            i,
                            package_addr.to_hex_literal(),
                            module_name,
                            function_name,
                            msg
                        ));
                    }
                }

                return Ok(());
            }
        }

        Err(anyhow!(
            "Function '{}' not found in module {}::{}",
            function_name,
            package_addr.to_hex_literal(),
            module_name
        ))
    }

    /// Check if a type satisfies the given ability constraints.
    ///
    /// This is a conservative check - it returns Ok if the type appears to
    /// satisfy constraints based on known type information. For unknown types,
    /// it may conservatively allow them (since the VM will catch violations).
    fn check_type_satisfies_constraints(
        &self,
        type_tag: &TypeTag,
        constraints: &move_binary_format::file_format::AbilitySet,
    ) -> Result<(), String> {
        use move_binary_format::file_format::Ability;

        // Primitives have all abilities except key
        let primitive_abilities = |has_key_constraint: bool| {
            if has_key_constraint {
                Err("Primitive types do not have the 'key' ability".to_string())
            } else {
                Ok(())
            }
        };

        let has_key = constraints.has_ability(Ability::Key);
        let has_store = constraints.has_ability(Ability::Store);
        let has_copy = constraints.has_ability(Ability::Copy);
        let has_drop = constraints.has_ability(Ability::Drop);

        match type_tag {
            // Primitives have copy, drop, store but NOT key
            TypeTag::Bool
            | TypeTag::U8
            | TypeTag::U16
            | TypeTag::U32
            | TypeTag::U64
            | TypeTag::U128
            | TypeTag::U256
            | TypeTag::Address => primitive_abilities(has_key),

            // Signer has drop but NOT copy, store, or key
            TypeTag::Signer => {
                if has_copy {
                    return Err("'signer' does not have the 'copy' ability".to_string());
                }
                if has_store {
                    return Err("'signer' does not have the 'store' ability".to_string());
                }
                if has_key {
                    return Err("'signer' does not have the 'key' ability".to_string());
                }
                Ok(())
            }

            // Vector has the abilities of its element type (except key)
            TypeTag::Vector(inner) => {
                if has_key {
                    return Err("'vector' does not have the 'key' ability".to_string());
                }
                // Recursively check inner type constraints (minus key)
                // We reconstruct the constraint set without key using union of singletons
                let mut inner_constraints = move_binary_format::file_format::AbilitySet::EMPTY;
                if has_copy {
                    inner_constraints = inner_constraints.union(
                        move_binary_format::file_format::AbilitySet::singleton(Ability::Copy),
                    );
                }
                if has_drop {
                    inner_constraints = inner_constraints.union(
                        move_binary_format::file_format::AbilitySet::singleton(Ability::Drop),
                    );
                }
                if has_store {
                    inner_constraints = inner_constraints.union(
                        move_binary_format::file_format::AbilitySet::singleton(Ability::Store),
                    );
                }
                self.check_type_satisfies_constraints(inner, &inner_constraints)
            }

            // Structs - look up the actual abilities and validate type arguments
            TypeTag::Struct(struct_tag) => {
                let struct_id = ModuleId::new(
                    struct_tag.address,
                    Identifier::new(struct_tag.module.as_str())
                        .map_err(|_| "Invalid module name in type".to_string())?,
                );

                if let Some(module) = self.get_module_with_alias(&struct_id) {
                    // Find the struct definition
                    for struct_def in &module.struct_defs {
                        let handle = &module.datatype_handles[struct_def.struct_handle.0 as usize];
                        let name = module.identifier_at(handle.name).to_string();
                        if name == struct_tag.name.as_str() {
                            let abilities = handle.abilities;

                            // Check each required constraint
                            if has_key && !abilities.has_ability(Ability::Key) {
                                return Err(format!(
                                    "Type '{}::{}::{}' does not have the 'key' ability",
                                    struct_tag.address.to_hex_literal(),
                                    struct_tag.module,
                                    struct_tag.name
                                ));
                            }
                            if has_store && !abilities.has_ability(Ability::Store) {
                                return Err(format!(
                                    "Type '{}::{}::{}' does not have the 'store' ability",
                                    struct_tag.address.to_hex_literal(),
                                    struct_tag.module,
                                    struct_tag.name
                                ));
                            }
                            if has_copy && !abilities.has_ability(Ability::Copy) {
                                return Err(format!(
                                    "Type '{}::{}::{}' does not have the 'copy' ability",
                                    struct_tag.address.to_hex_literal(),
                                    struct_tag.module,
                                    struct_tag.name
                                ));
                            }
                            if has_drop && !abilities.has_ability(Ability::Drop) {
                                return Err(format!(
                                    "Type '{}::{}::{}' does not have the 'drop' ability",
                                    struct_tag.address.to_hex_literal(),
                                    struct_tag.module,
                                    struct_tag.name
                                ));
                            }

                            // ENHANCED: Validate type arguments against the struct's type parameter constraints
                            // This catches cases like passing a type without 'store' to Coin<T> where T: store
                            if !struct_tag.type_params.is_empty() {
                                self.validate_struct_type_params(
                                    struct_tag,
                                    &handle.type_parameters,
                                )?;
                            }

                            return Ok(());
                        }
                    }

                    // Struct not found in module - let VM handle it
                    Ok(())
                } else {
                    // Module not found - let VM handle it (might be runtime loaded)
                    Ok(())
                }
            }
        }
    }

    /// Validate that a struct's type arguments satisfy the struct's type parameter constraints.
    ///
    /// For example, for `Coin<SUI>` where Coin is defined as `struct Coin<phantom T>`,
    /// this validates that SUI satisfies any constraints on T.
    fn validate_struct_type_params(
        &self,
        struct_tag: &move_core_types::language_storage::StructTag,
        type_params: &[move_binary_format::file_format::DatatypeTyParameter],
    ) -> Result<(), String> {
        // Number of type args should match type params
        if struct_tag.type_params.len() != type_params.len() {
            return Err(format!(
                "Type argument count mismatch for {}::{}::{}: expected {}, got {}",
                struct_tag.address.to_hex_literal(),
                struct_tag.module,
                struct_tag.name,
                type_params.len(),
                struct_tag.type_params.len()
            ));
        }

        // Validate each type argument against its parameter's constraints
        for (i, (type_arg, param)) in struct_tag
            .type_params
            .iter()
            .zip(type_params.iter())
            .enumerate()
        {
            // Phantom type parameters don't affect runtime, so we skip validation
            // (they only need to satisfy constraints for compile-time reasons)
            if param.is_phantom {
                continue;
            }

            let param_constraints = param.constraints;
            if param_constraints != move_binary_format::file_format::AbilitySet::EMPTY {
                self.check_type_satisfies_constraints(type_arg, &param_constraints)
                    .map_err(|e| {
                        format!(
                            "Type argument {} for {}::{}::{} does not satisfy constraints: {}",
                            i,
                            struct_tag.address.to_hex_literal(),
                            struct_tag.module,
                            struct_tag.name,
                            e
                        )
                    })?;
            }
        }

        Ok(())
    }

    /// Resolve the return types of a function to TypeTags given concrete type arguments.
    ///
    /// This is the key method for full type tracking - it looks up the function signature
    /// in bytecode and instantiates the return types with the provided type arguments.
    pub fn resolve_function_return_types(
        &self,
        package_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
        type_args: &[TypeTag],
    ) -> Option<Vec<TypeTag>> {
        let id = ModuleId::new(*package_addr, Identifier::new(module_name).ok()?);

        // Try direct lookup first, then check for alias
        let module = self.modules.get(&id).or_else(|| {
            // Check if this address has an alias (for deployed packages)
            self.address_aliases
                .get(package_addr)
                .and_then(|aliased_addr| {
                    let aliased_id =
                        ModuleId::new(*aliased_addr, Identifier::new(module_name).ok()?);
                    self.modules.get(&aliased_id)
                })
        })?;

        // Find the function
        for def in &module.function_defs {
            let handle = &module.function_handles[def.function.0 as usize];
            let fn_name = module.identifier_at(handle.name).to_string();
            if fn_name == function_name {
                let return_sig = &module.signatures[handle.return_.0 as usize];

                // Convert each return type to TypeTag
                let return_types: Option<Vec<TypeTag>> = return_sig
                    .0
                    .iter()
                    .map(|token| signature_token_to_type_tag(module, token, type_args))
                    .collect();

                return return_types;
            }
        }
        None
    }

    /// Get struct information by type path.
    /// Type path can be "0x2::coin::Coin" or just "Coin" for search.
    pub fn get_struct_info(&self, type_path: &str) -> Option<serde_json::Value> {
        // Try to parse as full path first: 0x...::module::Type
        let parts: Vec<&str> = type_path.split("::").collect();

        if parts.len() >= 3 {
            // Full path: 0x...::module::Type
            let addr = AccountAddress::from_hex_literal(parts[0]).ok()?;
            let mod_name = parts[1];
            let type_name = parts[2];

            let id = ModuleId::new(addr, Identifier::new(mod_name).ok()?);
            let module = self.get_module_with_alias(&id)?;

            // Find the struct
            for struct_def in &module.struct_defs {
                let datatype_handle = &module.datatype_handles[struct_def.struct_handle.0 as usize];
                let name = module.identifier_at(datatype_handle.name).to_string();

                if name == type_name {
                    let abilities = get_abilities_list(datatype_handle.abilities);

                    let type_params: Vec<serde_json::Value> = datatype_handle
                        .type_parameters
                        .iter()
                        .enumerate()
                        .map(|(i, param)| {
                            serde_json::json!({
                                "name": format!("T{}", i),
                                "constraints": get_abilities_list(param.constraints),
                                "is_phantom": param.is_phantom,
                            })
                        })
                        .collect();

                    let fields: Vec<serde_json::Value> = match &struct_def.field_information {
                        move_binary_format::file_format::StructFieldInformation::Declared(
                            field_defs,
                        ) => field_defs
                            .iter()
                            .map(|field| {
                                let field_name = module.identifier_at(field.name).to_string();
                                let field_type = format_signature_token(module, &field.signature.0);
                                serde_json::json!({
                                    "name": field_name,
                                    "type": field_type,
                                })
                            })
                            .collect(),
                        move_binary_format::file_format::StructFieldInformation::Native => {
                            Vec::new()
                        }
                    };

                    return Some(serde_json::json!({
                        "path": format!("{}::{}::{}", addr.to_hex_literal(), mod_name, type_name),
                        "name": type_name,
                        "abilities": abilities,
                        "type_params": type_params,
                        "fields": fields,
                    }));
                }
            }
        } else {
            // Just a type name, search all modules
            for (id, module) in &self.modules {
                for struct_def in &module.struct_defs {
                    let datatype_handle =
                        &module.datatype_handles[struct_def.struct_handle.0 as usize];
                    let name = module.identifier_at(datatype_handle.name).to_string();

                    if name == type_path {
                        let abilities = get_abilities_list(datatype_handle.abilities);

                        let type_params: Vec<serde_json::Value> = datatype_handle
                            .type_parameters
                            .iter()
                            .enumerate()
                            .map(|(i, param)| {
                                serde_json::json!({
                                    "name": format!("T{}", i),
                                    "constraints": get_abilities_list(param.constraints),
                                    "is_phantom": param.is_phantom,
                                })
                            })
                            .collect();

                        let fields: Vec<serde_json::Value> = match &struct_def.field_information {
                            move_binary_format::file_format::StructFieldInformation::Declared(
                                field_defs,
                            ) => field_defs
                                .iter()
                                .map(|field| {
                                    let field_name = module.identifier_at(field.name).to_string();
                                    let field_type =
                                        format_signature_token(module, &field.signature.0);
                                    serde_json::json!({
                                        "name": field_name,
                                        "type": field_type,
                                    })
                                })
                                .collect(),
                            move_binary_format::file_format::StructFieldInformation::Native => {
                                Vec::new()
                            }
                        };

                        return Some(serde_json::json!({
                            "path": format!("{}::{}::{}", id.address().to_hex_literal(), id.name(), name),
                            "name": name,
                            "abilities": abilities,
                            "type_params": type_params,
                            "fields": fields,
                        }));
                    }
                }
            }
        }

        None
    }
}

/// Information about a struct.
#[derive(Debug, Clone)]
pub struct StructInfo {
    pub abilities: Vec<String>,
    pub type_params: Vec<TypeParamInfo>,
    pub fields: Vec<FieldInfo>,
}

#[derive(Debug, Clone)]
pub struct TypeParamInfo {
    pub name: String,
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub field_type: String,
}

/// Convert ability set to list of ability names.
fn get_abilities_list(abilities: move_binary_format::file_format::AbilitySet) -> Vec<String> {
    let mut result = Vec::new();
    if abilities.has_copy() {
        result.push("copy".to_string());
    }
    if abilities.has_drop() {
        result.push("drop".to_string());
    }
    if abilities.has_store() {
        result.push("store".to_string());
    }
    if abilities.has_key() {
        result.push("key".to_string());
    }
    result
}

/// Format a signature token as a type string.
fn format_signature_token(
    module: &CompiledModule,
    token: &move_binary_format::file_format::SignatureToken,
) -> String {
    use move_binary_format::file_format::SignatureToken;

    match token {
        SignatureToken::Bool => "bool".to_string(),
        SignatureToken::U8 => "u8".to_string(),
        SignatureToken::U16 => "u16".to_string(),
        SignatureToken::U32 => "u32".to_string(),
        SignatureToken::U64 => "u64".to_string(),
        SignatureToken::U128 => "u128".to_string(),
        SignatureToken::U256 => "u256".to_string(),
        SignatureToken::Address => "address".to_string(),
        SignatureToken::Signer => "signer".to_string(),
        SignatureToken::Vector(inner) => {
            format!("vector<{}>", format_signature_token(module, inner))
        }
        SignatureToken::Datatype(idx) => {
            let datatype_handle = &module.datatype_handles[idx.0 as usize];
            let module_handle = &module.module_handles[datatype_handle.module.0 as usize];
            let addr = module.address_identifier_at(module_handle.address);
            let mod_name = module.identifier_at(module_handle.name);
            let type_name = module.identifier_at(datatype_handle.name);
            format!("{}::{}::{}", addr.to_hex_literal(), mod_name, type_name)
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, type_args) = inst.as_ref();
            let datatype_handle = &module.datatype_handles[idx.0 as usize];
            let module_handle = &module.module_handles[datatype_handle.module.0 as usize];
            let addr = module.address_identifier_at(module_handle.address);
            let mod_name = module.identifier_at(module_handle.name);
            let type_name = module.identifier_at(datatype_handle.name);
            let args: Vec<String> = type_args
                .iter()
                .map(|t| format_signature_token(module, t))
                .collect();
            format!(
                "{}::{}::{}<{}>",
                addr.to_hex_literal(),
                mod_name,
                type_name,
                args.join(", ")
            )
        }
        SignatureToken::Reference(inner) => {
            format!("&{}", format_signature_token(module, inner))
        }
        SignatureToken::MutableReference(inner) => {
            format!("&mut {}", format_signature_token(module, inner))
        }
        SignatureToken::TypeParameter(idx) => {
            format!("T{}", idx)
        }
    }
}

// =============================================================================
// Function Signature Type Resolution
// =============================================================================

use move_core_types::language_storage::{StructTag, TypeTag};

/// Convert a SignatureToken to a TypeTag, substituting type parameters with provided type arguments.
///
/// This enables resolving function return types to TypeTags BEFORE execution,
/// which is critical for proper type tracking in PTB execution.
///
/// ## Address Handling
///
/// **Important**: The addresses in the returned TypeTags are the **bytecode addresses**
/// (from the module's self-address), NOT deployment addresses. This is by design:
///
/// - The Move VM uses bytecode addresses internally for type checking
/// - When a package is deployed at address X but compiled with self-address Y,
///   the types will use address Y (the bytecode address)
/// - This matches how the Sui client handles types
///
/// If you need to compare types from external sources (like GraphQL) that use
/// storage/deployment addresses, use `normalize_type_tag_with_aliases()` from
/// the `types` module to normalize them first.
pub fn signature_token_to_type_tag(
    module: &CompiledModule,
    token: &move_binary_format::file_format::SignatureToken,
    type_args: &[TypeTag],
) -> Option<TypeTag> {
    use move_binary_format::file_format::SignatureToken;

    match token {
        SignatureToken::Bool => Some(TypeTag::Bool),
        SignatureToken::U8 => Some(TypeTag::U8),
        SignatureToken::U16 => Some(TypeTag::U16),
        SignatureToken::U32 => Some(TypeTag::U32),
        SignatureToken::U64 => Some(TypeTag::U64),
        SignatureToken::U128 => Some(TypeTag::U128),
        SignatureToken::U256 => Some(TypeTag::U256),
        SignatureToken::Address => Some(TypeTag::Address),
        SignatureToken::Signer => Some(TypeTag::Signer),
        SignatureToken::Vector(inner) => {
            let inner_tag = signature_token_to_type_tag(module, inner, type_args)?;
            Some(TypeTag::Vector(Box::new(inner_tag)))
        }
        SignatureToken::Datatype(idx) => {
            let datatype_handle = &module.datatype_handles[idx.0 as usize];
            let module_handle = &module.module_handles[datatype_handle.module.0 as usize];
            let addr = *module.address_identifier_at(module_handle.address);
            let mod_name = module.identifier_at(module_handle.name).to_owned();
            let type_name = module.identifier_at(datatype_handle.name).to_owned();
            Some(TypeTag::Struct(Box::new(StructTag {
                address: addr,
                module: mod_name,
                name: type_name,
                type_params: vec![],
            })))
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, sig_type_args) = inst.as_ref();
            let datatype_handle = &module.datatype_handles[idx.0 as usize];
            let module_handle = &module.module_handles[datatype_handle.module.0 as usize];
            let addr = *module.address_identifier_at(module_handle.address);
            let mod_name = module.identifier_at(module_handle.name).to_owned();
            let type_name = module.identifier_at(datatype_handle.name).to_owned();

            // Recursively resolve type arguments
            let resolved_type_params: Option<Vec<TypeTag>> = sig_type_args
                .iter()
                .map(|t| signature_token_to_type_tag(module, t, type_args))
                .collect();

            Some(TypeTag::Struct(Box::new(StructTag {
                address: addr,
                module: mod_name,
                name: type_name,
                type_params: resolved_type_params?,
            })))
        }
        SignatureToken::TypeParameter(idx) => {
            // Substitute with concrete type argument
            type_args.get(*idx as usize).cloned()
        }
        // References cannot be return types in public functions, but handle gracefully
        SignatureToken::Reference(inner) | SignatureToken::MutableReference(inner) => {
            signature_token_to_type_tag(module, inner, type_args)
        }
    }
}

/// Information about a function's signature for type resolution.
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    /// Number of type parameters the function expects
    pub type_param_count: usize,
    /// Parameter types as SignatureTokens
    pub parameter_types: Vec<move_binary_format::file_format::SignatureToken>,
    /// Return types as SignatureTokens (need type_args to fully resolve)
    pub return_types: Vec<move_binary_format::file_format::SignatureToken>,
}

// =============================================================================
// ModuleProvider Implementation
// =============================================================================

impl ModuleProvider for LocalModuleResolver {
    fn load_package(&mut self, modules: Vec<(String, Vec<u8>)>) -> Result<AccountAddress> {
        let (_, addr) = self.add_package_modules(modules)?;
        addr.ok_or_else(|| anyhow!("No modules were loaded"))
    }

    fn load_package_at(
        &mut self,
        modules: Vec<(String, Vec<u8>)>,
        address: AccountAddress,
    ) -> Result<AccountAddress> {
        let (_, addr) = self.add_package_modules_at(modules, Some(address))?;
        addr.ok_or_else(|| anyhow!("No modules were loaded"))
    }

    fn has_module(&self, module_id: &ModuleId) -> bool {
        self.has_module(module_id)
    }

    fn get_module_bytes(&self, module_id: &ModuleId) -> Option<&[u8]> {
        self.modules_bytes.get(module_id).map(|v| v.as_slice())
    }

    fn list_packages(&self) -> Vec<AccountAddress> {
        self.list_packages()
    }

    fn module_count(&self) -> usize {
        self.module_count()
    }
}

impl ModuleResolver for LocalModuleResolver {
    type Error = anyhow::Error;

    fn get_module(&self, id: &ModuleId) -> Result<Option<Vec<u8>>, Self::Error> {
        // First, try direct lookup
        if let Some(bytes) = self.modules_bytes.get(id) {
            return Ok(Some(bytes.clone()));
        }

        // If not found, check if there's an alias for this address
        if let Some(aliased_addr) = self.address_aliases.get(id.address()) {
            let aliased_id = ModuleId::new(*aliased_addr, id.name().to_owned());
            if let Some(bytes) = self.modules_bytes.get(&aliased_id) {
                return Ok(Some(bytes.clone()));
            }
        }

        // For upgraded packages: storage_addr → runtime_id mapping.
        // When relocate() returns a storage address, the VM looks up the module
        // by storage address. Modules are stored under runtime_id, so we map here.
        if let Some(runtime_id) = self.package_runtime_ids.get(id.address()) {
            if runtime_id != id.address() {
                let runtime_mod = ModuleId::new(*runtime_id, id.name().to_owned());
                if let Some(bytes) = self.modules_bytes.get(&runtime_mod) {
                    return Ok(Some(bytes.clone()));
                }
            }
        }

        Ok(None)
    }
}
