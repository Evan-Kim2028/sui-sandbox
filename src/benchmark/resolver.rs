use anyhow::{anyhow, Context, Result};
use move_binary_format::file_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;
use move_core_types::resolver::ModuleResolver;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub struct LocalModuleResolver {
    modules: BTreeMap<ModuleId, CompiledModule>,
    modules_bytes: BTreeMap<ModuleId, Vec<u8>>,
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
}

impl ModuleResolver for LocalModuleResolver {
    type Error = anyhow::Error;

    fn get_module(&self, id: &ModuleId) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(self.modules_bytes.get(id).cloned())
    }
}

