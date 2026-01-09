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

impl LocalModuleResolver {
    pub fn new() -> Self {
        Self {
            modules: BTreeMap::new(),
            modules_bytes: BTreeMap::new(),
        }
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
        let entries = fs::read_dir(dir)
            .with_context(|| format!("read {}", dir.display()))?;
            
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
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
}

impl ModuleResolver for LocalModuleResolver {
    type Error = anyhow::Error;

    fn get_module(&self, id: &ModuleId) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(self.modules_bytes.get(id).cloned())
    }
}
