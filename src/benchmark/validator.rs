//! # Move Bytecode Validator
//!
//! This module provides validation utilities for Move bytecode, ensuring that
//! target functions exist, are callable, and have resolvable type layouts.
//!
//! ## Purpose
//!
//! The `Validator` struct acts as a bridge between the module resolver and the
//! type system, enabling:
//! - **Target validation**: Verify that a function exists and is public/entry
//! - **Type layout resolution**: Convert TypeTags to MoveTypeLayouts for BCS serialization
//! - **Struct field introspection**: Resolve struct definitions to their field layouts
//!
//! ## Architecture
//!
//! ```text
//! LocalModuleResolver ──► Validator ──► MoveTypeLayout
//!        │                    │
//!        │                    ├── validate_target()
//!        │                    ├── resolve_type_layout()
//!        │                    └── resolve_struct_layout()
//!        │
//!        └── CompiledModule bytecode
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! let resolver = LocalModuleResolver::new();
//! // ... load modules into resolver ...
//! let validator = Validator::new(&resolver);
//!
//! // Validate a target function exists and is callable
//! let module = validator.validate_target(addr, "my_module", "my_function")?;
//!
//! // Resolve type layouts for BCS serialization
//! let layout = validator.resolve_type_layout(&type_tag)?;
//! ```

use anyhow::{anyhow, Context, Result};
use move_binary_format::file_format::{
    CompiledModule, DatatypeHandleIndex, SignatureToken, StructFieldInformation, Visibility,
};
use move_core_types::account_address::AccountAddress;
use move_core_types::annotated_value::{
    MoveFieldLayout, MoveStructLayout, MoveTypeLayout, MoveValue,
};
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};

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
            .ok_or_else(|| {
                anyhow!(
                    "function not found: {} in module {}::{}",
                    function_name,
                    package_addr,
                    module_name
                )
            })?;

        // Visibility check: we generally target public or entry functions for PTBs
        let is_public = matches!(func_def.visibility, Visibility::Public);
        let is_entry = func_def.is_entry;

        if !is_public && !is_entry {
            return Err(anyhow!(
                "function is not public or entry: {} in module {}::{}",
                function_name,
                package_addr,
                module_name
            ));
        }

        Ok(module)
    }

    pub fn resolve_type_layout(&self, tag: &TypeTag) -> Result<MoveTypeLayout> {
        match tag {
            TypeTag::Bool => Ok(MoveTypeLayout::Bool),
            TypeTag::U8 => Ok(MoveTypeLayout::U8),
            TypeTag::U16 => Ok(MoveTypeLayout::U16),
            TypeTag::U32 => Ok(MoveTypeLayout::U32),
            TypeTag::U64 => Ok(MoveTypeLayout::U64),
            TypeTag::U128 => Ok(MoveTypeLayout::U128),
            TypeTag::U256 => Ok(MoveTypeLayout::U256),
            TypeTag::Address => Ok(MoveTypeLayout::Address),
            TypeTag::Signer => Ok(MoveTypeLayout::Signer),
            TypeTag::Vector(inner) => {
                let inner_layout = self.resolve_type_layout(inner)?;
                Ok(MoveTypeLayout::Vector(Box::new(inner_layout)))
            }
            TypeTag::Struct(struct_tag) => self.resolve_struct_layout(struct_tag),
        }
    }

    fn resolve_struct_layout(&self, struct_tag: &StructTag) -> Result<MoveTypeLayout> {
        let module = self
            .resolver
            .get_module_by_addr_name(&struct_tag.address, struct_tag.module.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "module not found for struct: {}::{}",
                    struct_tag.address,
                    struct_tag.module
                )
            })?;

        let struct_name_ident = Identifier::new(struct_tag.name.as_str())?;
        let struct_def = module
            .struct_defs()
            .iter()
            .find(|def| {
                let handle = module.datatype_handle_at(def.struct_handle);
                let name = module.identifier_at(handle.name);
                name == struct_name_ident.as_ident_str()
            })
            .ok_or_else(|| anyhow!("struct definition not found: {}", struct_tag.name))?;

        let field_defs = match &struct_def.field_information {
            StructFieldInformation::Native => {
                // For now, return error for natives to be safe, unless it's a known one.
                // We could match on standard natives here if needed.
                return Err(anyhow!(
                    "native struct layout resolution not supported: {}",
                    struct_tag
                ));
            }
            StructFieldInformation::Declared(fields) => fields,
        };

        let mut field_layouts = Vec::new();
        for field in field_defs {
            let field_name = module.identifier_at(field.name).to_owned();
            let layout =
                self.resolve_signature_token(&field.signature.0, &struct_tag.type_params, module)?;
            field_layouts.push(MoveFieldLayout::new(field_name, layout));
        }

        Ok(MoveTypeLayout::Struct(Box::new(MoveStructLayout::new(
            struct_tag.clone(),
            field_layouts,
        ))))
    }

    fn resolve_signature_token(
        &self,
        token: &SignatureToken,
        type_args: &[TypeTag],
        context_module: &CompiledModule,
    ) -> Result<MoveTypeLayout> {
        let tag = self.resolve_token_to_tag(token, type_args, context_module)?;
        self.resolve_type_layout(&tag)
    }

    pub fn resolve_token_to_tag(
        &self,
        token: &SignatureToken,
        type_args: &[TypeTag],
        context_module: &CompiledModule,
    ) -> Result<TypeTag> {
        match token {
            SignatureToken::Bool => Ok(TypeTag::Bool),
            SignatureToken::U8 => Ok(TypeTag::U8),
            SignatureToken::U16 => Ok(TypeTag::U16),
            SignatureToken::U32 => Ok(TypeTag::U32),
            SignatureToken::U64 => Ok(TypeTag::U64),
            SignatureToken::U128 => Ok(TypeTag::U128),
            SignatureToken::U256 => Ok(TypeTag::U256),
            SignatureToken::Address => Ok(TypeTag::Address),
            SignatureToken::Signer => Ok(TypeTag::Signer),
            SignatureToken::Vector(inner) => {
                let inner_tag = self.resolve_token_to_tag(inner, type_args, context_module)?;
                Ok(TypeTag::Vector(Box::new(inner_tag)))
            }
            SignatureToken::Datatype(idx) => {
                let tag = self.resolve_struct_handle_to_tag(*idx, context_module, &[])?;
                Ok(TypeTag::Struct(Box::new(tag)))
            }
            SignatureToken::DatatypeInstantiation(inst) => {
                let (idx, tokens) = &**inst;
                let resolved = tokens
                    .iter()
                    .map(|t| self.resolve_token_to_tag(t, type_args, context_module))
                    .collect::<Result<Vec<_>>>()?;
                let tag = self.resolve_struct_handle_to_tag(*idx, context_module, &resolved)?;
                Ok(TypeTag::Struct(Box::new(tag)))
            }
            SignatureToken::TypeParameter(idx) => {
                let tag = type_args
                    .get(*idx as usize)
                    .ok_or_else(|| anyhow!("type argument index out of bounds: {}", idx))?;
                Ok(tag.clone())
            }
            SignatureToken::Reference(_) | SignatureToken::MutableReference(_) => {
                Err(anyhow!("cannot convert reference token to type tag"))
            }
        }
    }

    fn resolve_struct_handle_to_tag(
        &self,
        idx: DatatypeHandleIndex,
        module: &CompiledModule,
        type_args: &[TypeTag],
    ) -> Result<StructTag> {
        let handle = module.datatype_handle_at(idx);
        let module_handle = module.module_handle_at(handle.module);
        let address = *module.address_identifier_at(module_handle.address);
        let module_name = module.identifier_at(module_handle.name);
        let struct_name = module.identifier_at(handle.name);

        Ok(StructTag {
            address,
            module: module_name.to_owned(),
            name: struct_name.to_owned(),
            type_params: type_args.to_vec(),
        })
    }

    pub fn validate_bcs_roundtrip(&self, layout: &MoveTypeLayout, bytes: &[u8]) -> Result<()> {
        let value = MoveValue::simple_deserialize(bytes, layout)
            .with_context(|| "BCS deserialize failed")?;

        let reserialized = value
            .simple_serialize()
            .ok_or_else(|| anyhow!("BCS serialize failed (value too deep?)"))?;

        if bytes != reserialized {
            return Err(anyhow!(
                "BCS roundtrip mismatch: input_len={} output_len={}",
                bytes.len(),
                reserialized.len()
            ));
        }
        Ok(())
    }
}
