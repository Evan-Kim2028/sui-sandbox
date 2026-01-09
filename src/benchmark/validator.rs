use anyhow::{anyhow, Result};
use move_binary_format::file_format::{CompiledModule, Visibility};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;

use crate::benchmark::resolver::LocalModuleResolver;

pub struct Validator<'a> {
    resolver: &'a LocalModuleResolver,
}

impl<'a> Validator<'a> {
    pub fn new(resolver: &'a LocalModuleResolver) -> Self {
        Self { resolver }
    }

    pub fn validate_target(
        &self,
        package_addr: AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> Result<&CompiledModule> {
        let module = self
            .resolver
            .get_module_by_addr_name(&package_addr, module_name)
            .ok_or_else(|| anyhow!("module not found: {}::{}", package_addr, module_name))?;

        let func_name_ident = Identifier::new(function_name)?;
        let func_def = module
            .function_defs()
            .iter()
            .find(|def| {
                let handle = module.function_handle_at(def.function);
                let name = module.identifier_at(handle.name);
                name == func_name_ident.as_ident_str()
            })
            .ok_or_else(|| anyhow!("function not found: {}", function_name))?;

        // Visibility check: we generally target public or entry functions for PTBs
        let is_public = matches!(func_def.visibility, Visibility::Public);
        let is_entry = func_def.is_entry;

        if !is_public && !is_entry {
             // In some contexts (e.g. friend) checks are more complex, but for general PTBs:
             return Err(anyhow!("function is not public or entry"));
        }

        Ok(module)
    }
}
