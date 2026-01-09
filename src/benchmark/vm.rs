use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::{ModuleId, StructTag, TypeTag};
use move_core_types::resolver::{LinkageResolver, ModuleResolver};
use move_vm_runtime::move_vm::MoveVM;
use move_vm_types::gas::UnmeteredGasMeter;
use std::collections::BTreeMap;

use crate::benchmark::resolver::LocalModuleResolver;

pub struct InMemoryStorage<'a> {
    module_resolver: &'a LocalModuleResolver,
    resources: BTreeMap<AccountAddress, BTreeMap<StructTag, Vec<u8>>>,
}

impl<'a> InMemoryStorage<'a> {
    pub fn new(module_resolver: &'a LocalModuleResolver) -> Self {
        Self {
            module_resolver,
            resources: BTreeMap::new(),
        }
    }

    pub fn set_resource(&mut self, addr: AccountAddress, tag: StructTag, data: Vec<u8>) {
        self.resources.entry(addr).or_default().insert(tag, data);
    }
}

impl<'a> LinkageResolver for InMemoryStorage<'a> {
    type Error = anyhow::Error;

    fn link_context(&self) -> AccountAddress {
        AccountAddress::ZERO
    }

    fn relocate(&self, module_id: &ModuleId) -> Result<ModuleId, Self::Error> {
        Ok(module_id.clone())
    }
}

impl<'a> ModuleResolver for InMemoryStorage<'a> {
    type Error = anyhow::Error;

    fn get_module(&self, id: &ModuleId) -> Result<Option<Vec<u8>>, Self::Error> {
        self.module_resolver.get_module(id)
    }
}

pub struct VMHarness<'a> {
    vm: MoveVM,
    storage: InMemoryStorage<'a>,
}

impl<'a> VMHarness<'a> {
    pub fn new(resolver: &'a LocalModuleResolver) -> Result<Self> {
        let vm = MoveVM::new(vec![]).map_err(|e| anyhow!("failed to create VM: {:?}", e))?;
        Ok(Self {
            vm,
            storage: InMemoryStorage::new(resolver),
        })
    }

    pub fn execute_entry_function(
        &mut self,
        module: &ModuleId,
        function_name: &move_core_types::identifier::IdentStr,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> Result<()> {
        let mut session = self.vm.new_session(&self.storage);
        
        let mut loaded_ty_args = Vec::new();
        for tag in ty_args {
            let ty = session.load_type(&tag).map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = UnmeteredGasMeter;

        session
            .execute_entry_function(
                module,
                function_name,
                loaded_ty_args,
                args,
                &mut gas_meter,
            )
            .map_err(|e| anyhow!("execution failed: {:?}", e))?;

        let (result, _store) = session.finish();
        let _changes = result.map_err(|e| anyhow!("session finish failed: {:?}", e))?;
        
        Ok(())
    }

    pub fn execute_function(
        &mut self,
        module: &ModuleId,
        function_name: &str,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> Result<()> {
        let function_name = move_core_types::identifier::Identifier::new(function_name)?;
        let mut session = self.vm.new_session(&self.storage);

        let mut loaded_ty_args = Vec::new();
        for tag in ty_args {
            let ty = session
                .load_type(&tag)
                .map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = UnmeteredGasMeter;
        session
            .execute_function_bypass_visibility(
                module,
                function_name.as_ident_str(),
                loaded_ty_args,
                args,
                &mut gas_meter,
                None,
            )
            .map_err(|e| anyhow!("execution failed: {:?}", e))?;

        let (result, _store) = session.finish();
        let _changes = result.map_err(|e| anyhow!("session finish failed: {:?}", e))?;
        Ok(())
    }
}
