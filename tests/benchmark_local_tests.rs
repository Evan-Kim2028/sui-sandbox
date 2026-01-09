use std::path::Path;

use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::validator::Validator;

#[test]
fn benchmark_local_can_load_fixture_modules() {
    let fixture_dir = Path::new("tests/fixture/build/fixture");
    assert!(fixture_dir.exists(), "fixture dir missing: {fixture_dir:?}");

    let mut resolver = LocalModuleResolver::new();
    let loaded = resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");
    assert!(loaded > 0, "expected >0 .mv modules from fixture");
}

#[test]
fn benchmark_local_bcs_roundtrip_primitives() {
    use move_core_types::annotated_value::MoveTypeLayout;

    let resolver = LocalModuleResolver::new();
    let validator = Validator::new(&resolver);

    let cases: Vec<(MoveTypeLayout, Vec<u8>)> = vec![
        (MoveTypeLayout::Bool, vec![0u8]),
        (MoveTypeLayout::U8, vec![7u8]),
        (MoveTypeLayout::U64, 42u64.to_le_bytes().to_vec()),
        (MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U8)), vec![0u8]), // empty vec<u8>
    ];

    for (layout, bytes) in cases {
        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("bcs roundtrip should succeed");
    }
}

#[test]
fn benchmark_local_vm_can_execute_entry_zero_args_fixture() {
    use sui_move_interface_extractor::benchmark::vm::VMHarness;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let mut harness = VMHarness::new(&resolver).expect("vm harness should construct");

    // The fixture corpus contains a simple non-entry function:
    // `fixture::test_module::simple_func(u64): u64`
    // Tier B will evolve to support more realistic entry execution, but we start
    // by proving we can execute *some* function in the local VM.
    let module = resolver
        .iter_modules()
        .find(|m| {
            let name = sui_move_interface_extractor::bytecode::compiled_module_name(m);
            name == "test_module"
        })
        .expect("test_module module should exist in fixture corpus");

    let module_id = module.self_id();
    harness
        .execute_function(&module_id, "simple_func", vec![], vec![42u64.to_le_bytes().to_vec()])
        .expect("VM should execute simple_func");
}
