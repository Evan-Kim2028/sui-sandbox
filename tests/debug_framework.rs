//! Debug framework module loading

use move_core_types::resolver::ModuleResolver;
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;

#[test]
fn test_framework_modules() {
    let resolver = LocalModuleResolver::with_sui_framework().expect("load framework");

    println!("Total modules loaded: {}", resolver.module_count());

    // Check for key modules
    let key_modules = [
        ("0x1", "vector"),
        ("0x1", "option"),
        ("0x2", "object"),
        ("0x2", "transfer"),
        ("0x2", "coin"),
        ("0x2", "sui"),
        ("0x2", "clock"),
        ("0x2", "tx_context"),
    ];

    println!("\nChecking key modules:");
    for (addr, name) in key_modules {
        let module_id = move_core_types::language_storage::ModuleId::new(
            move_core_types::account_address::AccountAddress::from_hex_literal(addr).unwrap(),
            move_core_types::identifier::Identifier::new(name).unwrap(),
        );
        let has = resolver.get_module(&module_id).is_ok();
        println!("  {}::{}: {}", addr, name, if has { "✓" } else { "✗" });
    }
}

#[test]
fn test_object_module_structs() {
    use move_binary_format::file_format::CompiledModule;

    let resolver = LocalModuleResolver::with_sui_framework().expect("load framework");

    let object_module_id = move_core_types::language_storage::ModuleId::new(
        move_core_types::account_address::AccountAddress::from_hex_literal("0x2").unwrap(),
        move_core_types::identifier::Identifier::new("object").unwrap(),
    );

    let module_bytes = resolver
        .get_module(&object_module_id)
        .expect("get object module")
        .expect("no module");
    let module = CompiledModule::deserialize_with_defaults(&module_bytes).expect("deser");

    println!("Structs in 0x2::object:");
    for def in &module.struct_defs {
        let handle = &module.datatype_handles[def.struct_handle.0 as usize];
        let name = module.identifier_at(handle.name);
        println!("  - {}", name);

        // Show fields if not native
        if let move_binary_format::file_format::StructFieldInformation::Declared(fields) =
            &def.field_information
        {
            for field in fields {
                let field_name = module.identifier_at(field.name);
                println!("      field: {}", field_name);
            }
        }
    }
}
