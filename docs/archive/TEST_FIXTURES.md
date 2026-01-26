# Test Fixture Structure

This document explains how test fixtures are structured and how to create new fixtures for the `sui-sandbox` benchmark suite.

## Directory Layout

```
tests/fixture/
├── build/
│   ├── fixture/                    # Main test corpus
│   │   ├── bytecode_modules/      # Compiled Move bytecode (.mv files)
│   │   ├── debug_info/            # Move debug info
│   │   ├── sources/              # Move source files
│   │   └── BuildInfo.yaml         # Build metadata
│   └── failure_cases/            # Failure stage test fixtures (planned)
│       └── bytecode_modules/      # Compiled failure test cases
├── Move.toml                     # Move package configuration
├── Move.lock                     # Move dependency lock file
└── sources/                      # Additional Move source files
    └── ...
```

## Fixture Requirements

### Bytecode Modules

1. **Location**: `tests/fixture/build/fixture/bytecode_modules/*.mv`
2. **Format**: Compiled Move bytecode files
3. **Naming**: `{module_address}_{module_name}.mv` (canonicalized)
4. **Compilation**: Must be compiled with `sui move build` or equivalent Move compiler

### Move Source Files

1. **Location**: `tests/fixture/build/fixture/sources/*.move`
2. **Naming**: `{package_name}_{module_name}.move` (e.g., `test_module.move`)
3. **Structure**:

   ```move
   module fixture::module_name;

   public fun simple_function(param: u64): u64 {
       param + 1
   }
   ```

### Module Naming

- **Address**: Defined in `Move.toml` (default: `0x1` for `fixture` package)
- **Module**: In source file header (`module fixture::module_name;`)
- **Canonicalized address**: For bytecode files, use full 64-character hex string

## Creating New Fixtures

### Step 1: Write Move Source

Create a new Move source file:

```bash
touch tests/fixture/build/fixture/sources/new_module.move
```

Example source:

```move
module fixture::new_module;

public struct Counter has drop {
    value: u64
}

public fun create_counter(initial: u64): Counter {
    Counter { value: initial }
}

public fun increment(counter: &mut Counter): u64 {
    counter.value = counter.value + 1;
    counter.value
}
```

### Step 2: Compile Bytecode

Compile the Move source to bytecode:

```bash
cd tests/fixture
sui move build
```

This will generate `.mv` files in `build/fixture/bytecode_modules/`.

### Step 3: Verify Fixture

Run tests to verify the fixture is accessible:

```bash
cargo test benchmark_local_can_load_fixture_modules
```

### Step 4: Add Tests

Add tests for the new fixture in `tests/benchmark_local_tests.rs`:

```rust
#[test]
fn benchmark_local_test_new_module() {
    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let module = resolver
        .iter_modules()
        .find(|m| {
            let name = sui-sandbox::bytecode::compiled_module_name(m);
            name == "new_module"
        })
        .expect("new_module should exist");

    // Add your test assertions here
}
```

## Test Fixture Categories

### 1. Success Cases (`build/fixture/`)

Modules that should successfully validate and execute:

- **simple_func**: Basic function with u64 parameter
- **various_types**: Functions with different Move primitive types
- **complex_layouts**: Functions with complex struct layouts (nested structs, generics, vectors)

### 2. Failure Stage Tests (`build/failure_cases/`)

Modules designed to trigger specific failure stages:

| Stage | Description | Trigger |
|--------|-------------|----------|
| A1 | Target not found | Missing module/function, non-public function |
| A2 | Unresolvable type | Type from non-existent module |
| A3 | BCS roundtrip fail | Malformed BCS bytes |
| A4 | Object params detected | Function with `&mut T` parameter |
| A5 | Generic function | Function with `<T>` type parameter |
| B1 | VM harness creation fail | Corrupt module bytes |
| B2 | Execution abort | Function with `abort!` |

## Move.toml Configuration

The fixture package uses the following `Move.toml`:

```toml
[package]
name = "fixture"
version = "0.0.1"

[dependencies]

[addresses]
fixture = "0x1"
```

### Address Configuration

- **`0x1`**: Default address for `fixture` package
- **Canonicalization**: Tests automatically convert to full 64-character hex string
- **Custom addresses**: Can add more entries in `[addresses]` section

## Common Patterns

### Entry Functions

```move
public entry fun entry_function(user: address, amount: u64) {
    // Entry function code
}
```

### Public Functions

```move
public fun public_function(x: u64): u64 {
    x * 2
}
```

### Struct Definitions

```move
public struct MyStruct has drop {
    field1: u64,
    field2: bool,
}

public fun create_struct(v: u64, b: bool): MyStruct {
    MyStruct { field1: v, field2: b }
}
```

### Generic Functions

```move
public struct Container<T> has drop {
    value: T
}

public fun create_container<T: drop>(value: T): Container<T> {
    Container { value }
}
```

## Troubleshooting

### Fixture Not Found

**Error**: `module not found: 0x...::module_name`

**Solution**:

1. Check `Move.toml` address configuration
2. Verify module name in source header matches test expectations
3. Ensure `.mv` file exists in `bytecode_modules/` directory

### Compilation Errors

**Error**: Move compiler fails

**Solution**:

1. Check Move syntax (semicolons, braces, etc.)
2. Verify imports and dependencies
3. Ensure struct/function visibility keywords are correct

### BCS Roundtrip Failures

**Error**: `BCS roundtrip mismatch`

**Solution**:

1. Verify Move struct fields match layout expectations
2. Check for missing or extra fields
3. Ensure field types match signature

## Performance Considerations

- **Fixture size**: Keep individual modules small (<10 KB)
- **Function count**: 5-10 functions per module is typical
- **Complexity**: Balance coverage with test execution time
- **Total corpus**: Main fixture should contain 10-50 modules for reasonable test duration

## Related Documentation

- [Getting Started](getting-started/QUICKSTART.md) - Quick start guide
- [NO_CHAIN_TYPE_INHABITATION_SPEC.md](NO_CHAIN_TYPE_INHABITATION_SPEC.md) - Benchmark spec
- [TROUBLESHOOTING.md](getting-started/TROUBLESHOOTING.md) - Common issues and fixes
