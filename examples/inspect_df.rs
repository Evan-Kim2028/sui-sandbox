//! Dynamic Field Module Inspector
//!
//! A diagnostic tool for inspecting the structure of Sui framework modules related
//! to dynamic fields, tables, and bags.
//!
//! Run with: cargo run --example inspect_df
//!
//! ## Overview
//!
//! This example inspects the bytecode structure of key Sui framework modules:
//! - `dynamic_field` - Core dynamic field operations
//! - `table` - Table collection type
//! - `bag` - Bag collection type (heterogeneous)
//! - `object` - UID and object operations
//!
//! ## Use Cases
//!
//! - Understanding how dynamic field hashing works
//! - Debugging field access issues in transaction replay
//! - Comparing bundled framework vs on-chain versions
//!
//! ## No Setup Required
//!
//! Uses bundled Sui framework - no API keys needed.

use move_binary_format::CompiledModule;
use move_core_types::resolver::ModuleResolver;
use sui_data_fetcher::graphql::GraphQLClient;
use sui_sandbox_core::resolver::LocalModuleResolver;

fn main() -> anyhow::Result<()> {
    // First, let's load from GraphQL to check the on-chain version
    println!("=== Loading from GraphQL ===");
    let graphql = GraphQLClient::mainnet();
    let mut resolver_graphql = LocalModuleResolver::new();
    match resolver_graphql.load_sui_framework_from_graphql(&graphql) {
        Ok(n) => println!("Loaded {} modules from GraphQL", n),
        Err(e) => println!("GraphQL load failed: {}", e),
    }

    let df_id_graphql = move_core_types::language_storage::ModuleId::new(
        move_core_types::account_address::AccountAddress::from_hex_literal("0x2")?,
        move_core_types::identifier::Identifier::new("dynamic_field")?,
    );

    if let Some(bytes) = resolver_graphql.get_module(&df_id_graphql)? {
        let module = CompiledModule::deserialize_with_defaults(&bytes)?;
        println!("\n=== GraphQL dynamic_field module ===");
        println!("  Field Instantiations:");
        for (i, inst) in module.field_instantiations().iter().enumerate() {
            let handle = &module.field_handles()[inst.handle.0 as usize];
            println!(
                "    {}: handle {} (owner_struct={}, field={})",
                i, inst.handle.0, handle.owner.0, handle.field
            );
        }
    }

    println!("\n========================================\n");
    // First let's inspect the PoolInner object bytes from the transaction
    // The PoolInner object ID is 0x5c44ceb4c4e8ebb76813c729f8681a449ed1831129ac6e1cf966c7fcefe7dddb
    // which has 2225 bytes according to the debug output

    // Let's just run the module inspection for now
    let mut resolver = LocalModuleResolver::new();
    resolver.load_sui_framework()?;

    // Get dynamic_field module
    let df_id = move_core_types::language_storage::ModuleId::new(
        move_core_types::account_address::AccountAddress::from_hex_literal("0x2")?,
        move_core_types::identifier::Identifier::new("dynamic_field")?,
    );

    if let Some(bytes) = resolver.get_module(&df_id)? {
        let module = CompiledModule::deserialize_with_defaults(&bytes)?;
        println!("=== dynamic_field module ===");

        // Print all function handles to understand Call(21)
        println!("Function Handles:");
        for (i, handle) in module.function_handles().iter().enumerate() {
            let mod_handle = &module.module_handles()[handle.module.0 as usize];
            let mod_name = module.identifier_at(mod_handle.name);
            let func_name = module.identifier_at(handle.name);
            println!("  {}: {}::{}", i, mod_name, func_name);
        }

        println!("\nFunctions:");
        for (i, func) in module.function_defs().iter().enumerate() {
            let handle = &module.function_handles()[func.function.0 as usize];
            let name = module.identifier_at(handle.name);
            let is_native = func.code.is_none();
            println!(
                "  {}: {} {}",
                i,
                name,
                if is_native { "(native)" } else { "" }
            );

            // Print bytecode for borrow and borrow_mut functions
            if let Some(code) = &func.code {
                if name.as_str() == "borrow" || name.as_str() == "borrow_mut" {
                    println!(
                        "    === {} BYTECODE ({} instructions) ===",
                        name.as_str().to_uppercase(),
                        code.code.len()
                    );
                    for (j, instr) in code.code.iter().enumerate() {
                        println!("      {}: {:?}", j, instr);
                    }
                }
            }
        }

        // Print function instantiations for CallGeneric resolution
        println!("\n  Function Instantiations:");
        for (i, inst) in module.function_instantiations().iter().enumerate() {
            let handle = &module.function_handles()[inst.handle.0 as usize];
            let name = module.identifier_at(handle.name);
            println!("    {}: {} -> handle {}", i, name, inst.handle.0);
        }

        // Print field instantiations for field access resolution
        println!("\n  Field Instantiations:");
        for (i, inst) in module.field_instantiations().iter().enumerate() {
            let handle = &module.field_handles()[inst.handle.0 as usize];
            println!(
                "    {}: handle {} (owner_struct={}, field={})",
                i, inst.handle.0, handle.owner.0, handle.field
            );
        }

        // Print field handles
        println!("\n  Field Handles:");
        for (i, handle) in module.field_handles().iter().enumerate() {
            println!(
                "    {}: owner_struct={}, field={}",
                i, handle.owner.0, handle.field
            );
        }

        // Print struct defs to understand field ordering
        println!("\n  Struct Definitions:");
        for (i, struct_def) in module.struct_defs().iter().enumerate() {
            if let move_binary_format::file_format::StructFieldInformation::Declared(fields) =
                &struct_def.field_information
            {
                println!("    struct {} has {} fields:", i, fields.len());
                for (fi, field) in fields.iter().enumerate() {
                    let field_name = module.identifier_at(field.name);
                    println!("      field {}: {}", fi, field_name);
                }
            }
        }
    }

    // Now inspect table module
    let table_id = move_core_types::language_storage::ModuleId::new(
        move_core_types::account_address::AccountAddress::from_hex_literal("0x2")?,
        move_core_types::identifier::Identifier::new("table")?,
    );

    if let Some(bytes) = resolver.get_module(&table_id)? {
        let module = CompiledModule::deserialize_with_defaults(&bytes)?;
        println!("\n=== table module ===");
        println!("Functions:");
        for (i, func) in module.function_defs().iter().enumerate() {
            let handle = &module.function_handles()[func.function.0 as usize];
            let name = module.identifier_at(handle.name);
            let is_native = func.code.is_none();
            println!(
                "  {}: {} {}",
                i,
                name,
                if is_native { "(native)" } else { "" }
            );

            // Print bytecode for borrow, borrow_mut, and contains functions
            if let Some(code) = &func.code {
                if name.as_str() == "borrow"
                    || name.as_str() == "borrow_mut"
                    || name.as_str() == "contains"
                {
                    println!(
                        "    === {} BYTECODE ({} instructions) ===",
                        name.as_str().to_uppercase(),
                        code.code.len()
                    );
                    for (j, instr) in code.code.iter().enumerate() {
                        println!("      {}: {:?}", j, instr);
                    }
                }
            }
        }

        // Print function instantiations
        println!("\n  Function Instantiations:");
        for (i, inst) in module.function_instantiations().iter().enumerate() {
            let handle = &module.function_handles()[inst.handle.0 as usize];
            let mod_handle = &module.module_handles()[handle.module.0 as usize];
            let mod_name = module.identifier_at(mod_handle.name);
            let func_name = module.identifier_at(handle.name);
            println!(
                "    {}: {}::{} -> handle {}",
                i, mod_name, func_name, inst.handle.0
            );
        }

        // Print field instantiations for table module
        println!("\n  Field Instantiations:");
        for (i, inst) in module.field_instantiations().iter().enumerate() {
            let handle = &module.field_handles()[inst.handle.0 as usize];
            println!(
                "    {}: handle {} (owner_struct={}, field={})",
                i, inst.handle.0, handle.owner.0, handle.field
            );
        }

        // Print field handles for table module
        println!("\n  Field Handles:");
        for (i, handle) in module.field_handles().iter().enumerate() {
            println!(
                "    {}: owner_struct={}, field={}",
                i, handle.owner.0, handle.field
            );
        }

        // Print struct definitions for table module
        println!("\n  Struct Definitions:");
        for (i, struct_def) in module.struct_defs().iter().enumerate() {
            if let move_binary_format::file_format::StructFieldInformation::Declared(fields) =
                &struct_def.field_information
            {
                println!("    struct {} has {} fields:", i, fields.len());
                for (fi, field) in fields.iter().enumerate() {
                    let field_name = module.identifier_at(field.name);
                    println!("      field {}: {}", fi, field_name);
                }
            }
        }
    }

    // Also inspect object module for uid_to_address
    let obj_id = move_core_types::language_storage::ModuleId::new(
        move_core_types::account_address::AccountAddress::from_hex_literal("0x2")?,
        move_core_types::identifier::Identifier::new("object")?,
    );

    if let Some(bytes) = resolver.get_module(&obj_id)? {
        let module = CompiledModule::deserialize_with_defaults(&bytes)?;
        println!("\n=== object module ===");

        // Print field handles to understand the structure
        println!("Field Handles:");
        for (i, handle) in module.field_handles().iter().enumerate() {
            println!(
                "  {}: owner_struct={}, field={}",
                i, handle.owner.0, handle.field
            );
        }

        // Print struct definitions
        println!("\nStruct Definitions:");
        for (i, struct_def) in module.struct_defs().iter().enumerate() {
            if let move_binary_format::file_format::StructFieldInformation::Declared(fields) =
                &struct_def.field_information
            {
                println!("  struct {} has {} fields:", i, fields.len());
                for (fi, field) in fields.iter().enumerate() {
                    let field_name = module.identifier_at(field.name);
                    println!("    field {}: {}", fi, field_name);
                }
            }
        }

        for (i, func) in module.function_defs().iter().enumerate() {
            let handle = &module.function_handles()[func.function.0 as usize];
            let name = module.identifier_at(handle.name);

            if name.as_str() == "uid_to_address" || name.as_str() == "uid_to_inner" {
                let is_native = func.code.is_none();
                println!(
                    "\nFunction {}: {} {}",
                    i,
                    name,
                    if is_native { "(native)" } else { "" }
                );
                if let Some(code) = &func.code {
                    println!("    Bytecode ({} instructions):", code.code.len());
                    for (j, instr) in code.code.iter().enumerate() {
                        println!("      {}: {:?}", j, instr);
                    }
                }
            }
        }
    }

    // Also inspect bag module
    let bag_id = move_core_types::language_storage::ModuleId::new(
        move_core_types::account_address::AccountAddress::from_hex_literal("0x2")?,
        move_core_types::identifier::Identifier::new("bag")?,
    );

    if let Some(bytes) = resolver.get_module(&bag_id)? {
        let module = CompiledModule::deserialize_with_defaults(&bytes)?;
        println!("\n=== bag module ===");
        println!("Functions:");
        for (i, func) in module.function_defs().iter().enumerate() {
            let handle = &module.function_handles()[func.function.0 as usize];
            let name = module.identifier_at(handle.name);
            let is_native = func.code.is_none();
            println!(
                "  {}: {} {}",
                i,
                name,
                if is_native { "(native)" } else { "" }
            );

            // Print bytecode for borrow and contains functions
            if let Some(code) = &func.code {
                if name.as_str() == "borrow" || name.as_str() == "contains_with_type" {
                    println!(
                        "    === {} BYTECODE ({} instructions) ===",
                        name.as_str().to_uppercase(),
                        code.code.len()
                    );
                    for (j, instr) in code.code.iter().enumerate() {
                        println!("      {}: {:?}", j, instr);
                    }
                }
            }
        }

        // Print struct definitions for Bag
        println!("\n  Struct Definitions:");
        for (i, struct_def) in module.struct_defs().iter().enumerate() {
            // Just print the struct name from field information
            println!("    struct {}", i);
            if let move_binary_format::file_format::StructFieldInformation::Declared(fields) =
                &struct_def.field_information
            {
                for (fi, field) in fields.iter().enumerate() {
                    let field_name = module.identifier_at(field.name);
                    println!("      field {}: {}", fi, field_name);
                }
            }
        }
    }

    Ok(())
}
