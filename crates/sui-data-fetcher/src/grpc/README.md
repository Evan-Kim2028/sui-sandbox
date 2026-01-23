# gRPC to Move VM Transformation Validation

This document describes the validation infrastructure added to address P0 and P1 coupling issues between the gRPC layer and Move VM execution.

## Version Management

### Current Versions

```rust
use sui_move_interface_extractor::grpc::version::{PINNED_SUI_VERSION, PROTO_SCHEMA_VERSION};

println!("Sui version: {}", PINNED_SUI_VERSION);  // mainnet-v1.62.1
println!("Proto version: {}", PROTO_SCHEMA_VERSION);  // sui.rpc.v2
```

### Upgrading Sui/gRPC Versions

Use the upgrade script:

```bash
./scripts/update-sui-version.sh mainnet-v1.70.0
```

This automatically:

1. Updates all `tag = "mainnet-vX.XX.X"` entries in Cargo.toml
2. Updates `PINNED_SUI_VERSION` in `src/grpc/version.rs`
3. Fetches new proto definitions from Sui repository

Then manually:

1. Update `Dockerfile` SUI_VERSION ARG
2. Run `cargo build` to regenerate proto Rust code
3. Rebuild framework bytecode (see script output for commands)
4. Run `cargo test` to verify compatibility

### Runtime Compatibility Checking

```rust
use sui_move_interface_extractor::grpc::{
    check_protocol_compatibility, check_service_compatibility, FeatureFlags
};

// Check against network protocol version
let result = check_protocol_compatibility(protocol_version);
if !result.compatible {
    eprintln!("Incompatible: {:?}", result.errors);
}

// Detect available features
let features = FeatureFlags::from_protocol_version(protocol_version);
if features.has_unchanged_runtime_objects {
    // Use the unchanged_loaded_runtime_objects field
}
```

## Issues Addressed

### P0 (Critical)

1. **BCS Format Assumptions** (`validation.rs`)
   - Added `BcsFormat` enum to explicitly track package vs. Move object formats
   - Replaced hard-coded string comparisons with explicit enum pattern matching
   - Added `validate_bcs_format()` for cross-validation between type string and bytes

2. **Proto Schema Versioning** (`validation.rs`)
   - Added `ProtoSchemaValidator` for runtime schema compatibility checks
   - Schema version constants in `proto_hashes` module
   - Validation warnings/errors for schema mismatches

3. **Type Argument Validation** (`validation.rs`)
   - Added `validate_type_arguments()` for eager validation
   - `validate_type_arguments_strict()` returns errors for any invalid types
   - Catches malformed type strings before VM execution

### P1 (Important)

1. **Version Metadata** (`state_source.rs`, `validation.rs`)
   - Added `ObjectSource` enum for tracking data provenance
   - Added `ObjectVersionMetadata` for version validation
   - Extended `ObjectData` with optional metadata field
   - Version validation against transaction expectations

2. **Module Dependency Graph** (`resolver.rs`)
   - Added `get_dependency_graph()` for full dependency graph
   - Added `validate_dependencies()` for pre-execution validation
   - Added `get_missing_dependency_details()` for detailed error reporting
   - Added `can_execute()` to check if a specific module can run

3. **gRPC Streaming Reconnection** (`client.rs`)
   - Added `ReconnectConfig` for configurable reconnection behavior
   - Added `ReconnectingCheckpointStream` with automatic reconnection
   - Exponential backoff with configurable limits
   - Tracks last checkpoint for resume position

4. **Gas Metering** (`vm.rs`)
   - Already implemented: `MeteredGasMeter` with budget enforcement
   - `GasMeterImpl` used in all execution paths
   - Gas consumption tracked in `ExecutionOutput`

## Usage

### Validated Object Conversion

```rust
use crate::grpc::{GrpcObject, TransformationContext, DataSource};

// Create context with expected versions
let mut ctx = TransformationContext::new()
    .with_source(DataSource::GrpcArchive);
ctx.add_expected_version("0x123...", 42);

// Convert GrpcObject to validated form
let grpc_obj = client.get_object("0x123...").await?;
let validated = grpc_obj.to_validated(&ctx)?;

// Validated object is ready for VM execution
println!("Version valid: {}", validated.version_metadata.version_valid);
```

### Dependency Validation

```rust
use crate::benchmark::resolver::LocalModuleResolver;

let resolver = LocalModuleResolver::with_sui_framework()?;

// Check missing dependencies before execution
if let Err(e) = resolver.validate_dependencies() {
    eprintln!("Missing packages: {}", e);
    // Fetch missing packages
}

// Get detailed missing dependency info
let missing = resolver.get_missing_dependency_details();
for (pkg_addr, dependent_modules) in missing {
    eprintln!("Package {} required by: {:?}", pkg_addr, dependent_modules);
}
```

### Reconnecting Stream

```rust
use crate::grpc::{GrpcClient, ReconnectConfig, ReconnectingCheckpointStream};

let client = GrpcClient::mainnet().await?;
let config = ReconnectConfig {
    max_retries: 20,
    base_delay_ms: 500,
    max_delay_ms: 30_000,
    verbose: true,
};

let mut stream = ReconnectingCheckpointStream::new(client, config).await?;

while let Some(result) = stream.next().await {
    match result {
        Ok(checkpoint) => println!("Checkpoint {}", checkpoint.sequence_number),
        Err(e) => eprintln!("Error: {}", e),
    }
}
```

## Architecture

```text
┌─────────────────────────────────────────────────────────────────┐
│                        gRPC Layer                                │
│  ┌─────────────┐    ┌─────────────────────┐                     │
│  │ GrpcClient  │ →  │ GrpcObject          │                     │
│  │             │    │ + to_validated()    │                     │
│  │             │    │ + validate_bcs()    │                     │
│  └─────────────┘    └─────────────────────┘                     │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Validation Layer                              │
│  ┌──────────────────────┐  ┌──────────────────────┐             │
│  │ TransformationContext│  │ ProtoSchemaValidator │             │
│  │ - expected_versions  │  │ - schema validation  │             │
│  │ - data source        │  │                      │             │
│  └──────────────────────┘  └──────────────────────┘             │
│                                                                  │
│  ┌──────────────────────┐  ┌──────────────────────┐             │
│  │ BcsFormat            │  │ TypeValidation       │             │
│  │ - Package            │  │ - validate_type_args │             │
│  │ - MoveObject         │  │ - strict validation  │             │
│  │ - Unknown            │  │                      │             │
│  └──────────────────────┘  └──────────────────────┘             │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                     State Layer                                  │
│  ┌──────────────────────┐  ┌──────────────────────┐             │
│  │ ObjectData           │  │ ObjectVersionMetadata│             │
│  │ + metadata           │  │ - expected_version   │             │
│  │ + validate_version() │  │ - version_valid      │             │
│  │ + source()           │  │ - source             │             │
│  └──────────────────────┘  └──────────────────────┘             │
│                                                                  │
│  ┌──────────────────────┐  ┌──────────────────────┐             │
│  │ LocalModuleResolver  │  │ Dependency Graph     │             │
│  │ + validate_deps()    │  │ - missing detection  │             │
│  │ + can_execute()      │  │ - cycle detection    │             │
│  └──────────────────────┘  └──────────────────────┘             │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Move VM Execution                            │
│  ┌──────────────────────┐  ┌──────────────────────┐             │
│  │ VMHarness            │  │ GasMeterImpl         │             │
│  │ + execute_function() │  │ - budget enforcement │             │
│  │ + get_trace()        │  │ - metered costs      │             │
│  └──────────────────────┘  └──────────────────────┘             │
└─────────────────────────────────────────────────────────────────┘
```

## Key Files Modified

| File | Changes |
|------|---------|
| `src/grpc/validation.rs` | New - Transformation validation module |
| `src/grpc/client.rs` | Added BcsFormat usage, to_validated(), ReconnectingCheckpointStream |
| `src/grpc/mod.rs` | Added validation module exports |
| `src/benchmark/state_source.rs` | Added ObjectSource, ObjectVersionMetadata, extended ObjectData |
| `src/benchmark/resolver.rs` | Added dependency graph and validation functions |

## Testing

The validation layer includes unit tests in `validation.rs`:

- BCS format detection
- Type argument validation
- Version metadata validation
- Transformation context

Run with: `cargo test grpc::validation`
