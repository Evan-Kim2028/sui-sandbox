//! Debug swap function parameters to understand the type error

use move_binary_format::file_format::CompiledModule;
use sui_move_interface_extractor::cache::CacheManager;

const CETUS_ROUTER: &str = "0x47a7b90756fba96fe649c2aaa10ec60dec6b8cb8545573d621310072721133aa";

#[test]
#[ignore] // Requires .tx-cache with Cetus package data
fn test_debug_swap_params() {
    let cache = CacheManager::new(".tx-cache").expect("No cache");

    let pkg = cache
        .get_package(CETUS_ROUTER)
        .expect("err")
        .expect("no pkg");

    let cetus_module = pkg
        .modules
        .iter()
        .find(|(name, _)| name == "cetus")
        .expect("no cetus module");
    let (_name, bytecode) = cetus_module;

    let module = CompiledModule::deserialize_with_defaults(bytecode).expect("deser");

    println!("Module address: {}", module.self_id());
    println!("\nLooking for swap_a2b...\n");

    for def in &module.function_defs {
        let handle = &module.function_handles[def.function.0 as usize];
        let func_name = module.identifier_at(handle.name).to_string();

        if func_name == "swap_a2b" {
            println!("=== swap_a2b ===");

            // Type parameters
            let ty_params = &handle.type_parameters;
            println!("Type parameters: {}", ty_params.len());
            for (i, tp) in ty_params.iter().enumerate() {
                println!("  T{}: {:?}", i, tp);
            }

            // Get signature
            let sig = &module.signatures[handle.parameters.0 as usize];
            println!("\nParameters ({}):", sig.0.len());
            for (i, tok) in sig.0.iter().enumerate() {
                println!("  param {}: {:?}", i, tok);
            }

            // Return signature
            let ret_sig = &module.signatures[handle.return_.0 as usize];
            println!("\nReturn ({}):", ret_sig.0.len());
            for (i, tok) in ret_sig.0.iter().enumerate() {
                println!("  return {}: {:?}", i, tok);
            }

            break;
        }
    }
}
