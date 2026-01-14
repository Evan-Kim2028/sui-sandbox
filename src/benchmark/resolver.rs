use anyhow::{anyhow, Context, Result};
use move_binary_format::file_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;
use move_core_types::resolver::ModuleResolver;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone)]
pub struct LocalModuleResolver {
    modules: BTreeMap<ModuleId, CompiledModule>,
    modules_bytes: BTreeMap<ModuleId, Vec<u8>>,
    /// Address aliases: maps target address -> source address
    /// When looking up a module at target address, also try source address
    address_aliases: BTreeMap<AccountAddress, AccountAddress>,
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
        static MOVE_STDLIB: &[u8] = include_bytes!("../../framework_bytecode/move-stdlib");
        static SUI_FRAMEWORK: &[u8] = include_bytes!("../../framework_bytecode/sui-framework");
        static SUI_SYSTEM: &[u8] = include_bytes!("../../framework_bytecode/sui-system");

        let mut count = 0;

        // Load each package's modules
        for package_bytes in [MOVE_STDLIB, SUI_FRAMEWORK, SUI_SYSTEM] {
            let module_bytes_list: Vec<Vec<u8>> = bcs::from_bytes(package_bytes)
                .map_err(|e| anyhow!("failed to deserialize framework package: {}", e))?;

            for bytes in module_bytes_list {
                let module = CompiledModule::deserialize_with_defaults(&bytes)
                    .map_err(|e| anyhow!("failed to deserialize framework module: {:?}", e))?;
                let id = module.self_id();
                self.modules.insert(id.clone(), module);
                self.modules_bytes.insert(id, bytes);
                count += 1;
            }
        }

        Ok(count)
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
                eprintln!(
                    "Warning: duplicate module {} found at {}, overwriting previous",
                    id,
                    path.display()
                );
            }
            self.modules.insert(id.clone(), module);
            self.modules_bytes.insert(id, bytes);
            count += 1;
        }
        Ok(count)
    }

    pub fn get_module_struct(&self, id: &ModuleId) -> Option<&CompiledModule> {
        self.modules.get(id)
    }

    pub fn get_module_by_addr_name(
        &self,
        addr: &AccountAddress,
        name: &str,
    ) -> Option<&CompiledModule> {
        let id = ModuleId::new(*addr, Identifier::new(name).ok()?);
        self.modules.get(&id)
    }

    pub fn iter_modules(&self) -> impl Iterator<Item = &CompiledModule> {
        self.modules.values()
    }

    /// Dynamically add a module from raw bytecode.
    /// This enables loading packages fetched from the RPC at runtime.
    pub fn add_module_bytes(&mut self, bytes: Vec<u8>) -> Result<ModuleId> {
        let module = CompiledModule::deserialize_with_defaults(&bytes)
            .map_err(|e| anyhow!("failed to deserialize module: {:?}", e))?;
        let id = module.self_id();
        self.modules.insert(id.clone(), module);
        self.modules_bytes.insert(id.clone(), bytes);
        Ok(id)
    }

    /// Dynamically add multiple modules (e.g., from a package).
    /// Returns the number of modules successfully loaded.
    /// Add package modules and return (module_count, package_address).
    /// The package_address is extracted from the first module's bytecode.
    pub fn add_package_modules(&mut self, modules: Vec<(String, Vec<u8>)>) -> Result<(usize, Option<AccountAddress>)> {
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
                    eprintln!("  Loaded module: {} ({})", name, id);

                    // Track the source address from bytecode
                    if source_addr.is_none() {
                        source_addr = Some(*id.address());
                    }
                }
                Err(e) => {
                    eprintln!("  Warning: Failed to load module {}: {}", name, e);
                }
            }
        }

        // Set up address alias if target differs from source
        if let (Some(target), Some(source)) = (target_addr, source_addr) {
            if target != source {
                eprintln!("  Address alias: {} -> {}", target.to_hex_literal(), source.to_hex_literal());
                self.address_aliases.insert(target, source);
            }
        }

        Ok((count, source_addr))
    }

    /// Check if a module is loaded.
    pub fn has_module(&self, id: &ModuleId) -> bool {
        self.modules.contains_key(id)
    }

    /// Check if a package (any module at this address) is loaded.
    pub fn has_package(&self, addr: &AccountAddress) -> bool {
        self.modules.keys().any(|id| id.address() == addr)
    }

    /// List all unique package addresses that have been loaded.
    pub fn list_packages(&self) -> Vec<AccountAddress> {
        use std::collections::BTreeSet;
        let addrs: BTreeSet<AccountAddress> = self.modules.keys()
            .map(|id| *id.address())
            .collect();
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

    /// Get all unique package addresses that are referenced by loaded modules
    /// but not yet loaded themselves. This is useful for fetching transitive dependencies.
    ///
    /// Returns a set of package addresses (not module IDs) that need to be fetched.
    pub fn get_missing_dependencies(&self) -> std::collections::BTreeSet<AccountAddress> {
        use std::collections::BTreeSet;

        // Framework addresses that we always have bundled
        let framework_addrs: BTreeSet<AccountAddress> = [
            AccountAddress::from_hex_literal("0x1").unwrap(),
            AccountAddress::from_hex_literal("0x2").unwrap(),
            AccountAddress::from_hex_literal("0x3").unwrap(),
        ]
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
        self.modules
            .keys()
            .map(|id| *id.address())
            .collect()
    }

    /// Get the source address for an aliased address, if any.
    /// This is used for address relocation during module loading.
    pub fn get_alias(&self, target: &AccountAddress) -> Option<AccountAddress> {
        self.address_aliases.get(target).copied()
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
        let module = self.modules.get(&id)?;

        let functions: Vec<String> = module.function_defs.iter()
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
        let module = self.modules.get(&id)?;

        let structs: Vec<String> = module.struct_defs.iter()
            .map(|def| {
                let handle = &module.datatype_handles[def.struct_handle.0 as usize];
                module.identifier_at(handle.name).to_string()
            })
            .collect();
        Some(structs)
    }

    /// Get detailed function information.
    pub fn get_function_info(&self, module_path: &str, function_name: &str) -> Option<serde_json::Value> {
        let (addr, mod_name) = Self::parse_module_path(module_path)?;
        let id = ModuleId::new(addr, Identifier::new(mod_name).ok()?);
        let module = self.modules.get(&id)?;

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
                let params: Vec<String> = params_sig.0.iter()
                    .map(|t| format_signature_token(module, t))
                    .collect();

                // Get return types
                let return_sig = &module.signatures[handle.return_.0 as usize];
                let returns: Vec<String> = return_sig.0.iter()
                    .map(|t| format_signature_token(module, t))
                    .collect();

                // Get type parameters
                let type_params: Vec<serde_json::Value> = handle.type_parameters.iter()
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
                        let params: Vec<String> = params_sig.0.iter()
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
    pub fn search_types(&self, pattern: &str, ability_filter: Option<&str>) -> Vec<serde_json::Value> {
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
                        if part.is_empty() { continue; }
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
                        if !abilities.iter().any(|a| a.to_lowercase() == filter.to_lowercase()) {
                            continue;
                        }
                    }

                    // Get field count
                    let field_count = match &def.field_information {
                        move_binary_format::file_format::StructFieldInformation::Declared(fields) => fields.len(),
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
                        if part.is_empty() { continue; }
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
                    let params: Vec<String> = params_sig.0.iter()
                        .map(|t| format_signature_token(module, t))
                        .collect();

                    let return_sig = &module.signatures[handle.return_.0 as usize];
                    let returns: Vec<String> = return_sig.0.iter()
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
        let module = self.modules.get(&id)?;

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
                let params: Vec<String> = params_sig.0.iter()
                    .map(|t| format_signature_token(module, t))
                    .collect();

                let return_sig = &module.signatures[handle.return_.0 as usize];
                let returns: Vec<String> = return_sig.0.iter()
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
        let module = self.modules.get(&id)?;

        let mut structs = Vec::new();

        for struct_def in &module.struct_defs {
            let datatype_handle = &module.datatype_handles[struct_def.struct_handle.0 as usize];
            let struct_name = module.identifier_at(datatype_handle.name).to_string();

            // Get abilities
            let abilities = get_abilities_list(datatype_handle.abilities);

            // Get type parameters
            let type_params: Vec<TypeParamInfo> = datatype_handle.type_parameters
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
                    field_defs.iter().map(|field| {
                        let field_name = module.identifier_at(field.name).to_string();
                        let field_type = format_signature_token(module, &field.signature.0);
                        FieldInfo {
                            name: field_name,
                            field_type,
                        }
                    }).collect()
                }
                move_binary_format::file_format::StructFieldInformation::Native => Vec::new(),
            };

            structs.push((struct_name, StructInfo {
                abilities,
                type_params,
                fields,
            }));
        }

        Some(structs)
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
            let module = self.modules.get(&id)?;

            // Find the struct
            for struct_def in &module.struct_defs {
                let datatype_handle = &module.datatype_handles[struct_def.struct_handle.0 as usize];
                let name = module.identifier_at(datatype_handle.name).to_string();

                if name == type_name {
                    let abilities = get_abilities_list(datatype_handle.abilities);

                    let type_params: Vec<serde_json::Value> = datatype_handle.type_parameters
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
                        move_binary_format::file_format::StructFieldInformation::Declared(field_defs) => {
                            field_defs.iter().map(|field| {
                                let field_name = module.identifier_at(field.name).to_string();
                                let field_type = format_signature_token(module, &field.signature.0);
                                serde_json::json!({
                                    "name": field_name,
                                    "type": field_type,
                                })
                            }).collect()
                        }
                        move_binary_format::file_format::StructFieldInformation::Native => Vec::new(),
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
                    let datatype_handle = &module.datatype_handles[struct_def.struct_handle.0 as usize];
                    let name = module.identifier_at(datatype_handle.name).to_string();

                    if name == type_path {
                        let abilities = get_abilities_list(datatype_handle.abilities);

                        let type_params: Vec<serde_json::Value> = datatype_handle.type_parameters
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
                            move_binary_format::file_format::StructFieldInformation::Declared(field_defs) => {
                                field_defs.iter().map(|field| {
                                    let field_name = module.identifier_at(field.name).to_string();
                                    let field_type = format_signature_token(module, &field.signature.0);
                                    serde_json::json!({
                                        "name": field_name,
                                        "type": field_type,
                                    })
                                }).collect()
                            }
                            move_binary_format::file_format::StructFieldInformation::Native => Vec::new(),
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
            let args: Vec<String> = type_args.iter()
                .map(|t| format_signature_token(module, t))
                .collect();
            format!("{}::{}::{}<{}>", addr.to_hex_literal(), mod_name, type_name, args.join(", "))
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

        Ok(None)
    }
}
