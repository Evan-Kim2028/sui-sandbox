# gRPC Archive: `unchanged_loaded_runtime_objects` Field Empty (Mysten Archive Only)

## Summary

When fetching transactions via gRPC from the **Mysten Sui archive** (`archive.mainnet.sui.io:443`), the `unchanged_loaded_runtime_objects` field in transaction effects is always empty, even though the field exists in the proto definition and is crucial for historical transaction replay.

**UPDATE (2026-01-17):** Some third-party gRPC providers **DO return this field correctly**, providing a viable alternative for transaction replay.

## Mysten Archive vs Third-Party gRPC Comparison

| Endpoint | `unchanged_loaded_runtime_objects` | Historical Object Fetch |
|----------|-----------------------------------|------------------------|
| `archive.mainnet.sui.io:443` | Always empty (0) | Modified objects only |
| Third-party gRPC providers | **Populated correctly** | **Full historical support (verified)** |

### Test Results Comparison

**Mysten Archive:**

```text
[gRPC effects] changed=7, unchanged_consensus=1, unchanged_runtime=0
Input object versions: 8 entries
Historical fetch: Only modified objects available
```

**Third-party gRPC:**

```text
[gRPC effects] changed=7, unchanged_consensus=1, unchanged_runtime=2
Input object versions: 10 entries
Historical fetch: 10/10 objects found at exact versions ✓
```

Third-party providers with enhanced effects data provide the 2 additional dynamic field child object versions AND can fetch all historical objects at those versions.

### Verified Historical Fetch Test

```text
=== Testing Historical Object Fetch ===

✓ 0xb663828d621746 @ v755312289 - FOUND (exact version)
✓ 0xd3293e25eec41a @ v755312289 - FOUND (exact version)
✓ 0x2709be18cbecb7 @ v755310891 - FOUND (exact version)
✓ 0xbb8967821ba01b @ v755312289 - FOUND (exact version)
✓ 0xd6a354aa9a1756 @ v755312289 - FOUND (exact version)
✓ 0x00000000000000 @ v699869418 - FOUND (exact version)
✓ 0x21ad2dda54a392 @ v754327245 - FOUND (exact version)
✓ 0x71e6517795bc3b @ v755063054 - FOUND (exact version)  <-- read-only child!
✓ 0x5201ca7537ccbc @ v755312289 - FOUND (exact version)
✓ 0x47dcbbc8561fe3 @ v755312289 - FOUND (exact version)

Success: 10/10
```

## Impact (Mysten Archive Only)

Using only Mysten's archive prevents accurate historical transaction replay for DeFi and other transactions that access dynamic fields:

- **Modified objects**: Available via `changed_objects.input_version` ✓
- **Read-only shared objects**: Available via `unchanged_consensus_objects` ✓
- **Read-only dynamic field children**: NOT available ✗

Current replay success rate for DeFi transactions with Mysten archive: **~5%** (16/20 fail due to stale state)

## Technical Details

### Proto Definition Exists

The field is defined in `effects.proto`:

```protobuf
message TransactionEffects {
  // ... other fields ...
  repeated ObjectReference unchanged_loaded_runtime_objects = 15;
}
```

### Rust Types Show Field Is Used Internally

From `sui-types/src/full_checkpoint_content.rs`:

```rust
pub struct CheckpointTransaction {
    pub effects: TransactionEffects,
    pub events: Option<TransactionEvents>,
    pub unchanged_loaded_runtime_objects: Vec<ObjectKey>,  // Exists separately!
}
```

And from `rpc_proto_conversions.rs`:

```rust
if submask.contains(TransactionEffects::UNCHANGED_LOADED_RUNTIME_OBJECTS_FIELD) {
    effects.set_unchanged_loaded_runtime_objects(
        source
            .unchanged_loaded_runtime_objects
            .iter()
            .map(Into::into)
            .collect(),
    );
}
```

### Test Results

```text
[gRPC effects] changed=7, unchanged_consensus=1, unchanged_runtime=0
[gRPC effects] changed=7, unchanged_consensus=1, unchanged_runtime=0
[gRPC effects] changed=4, unchanged_consensus=3, unchanged_runtime=0
```

Tested with explicit field mask requesting `effects.unchanged_loaded_runtime_objects` - still returns 0.

### Objects Affected

Example from transaction `7aQBpHjvgNguGB4WoS9h8ZPgrAPfDqae25BZn5MxXoWY`:

| Object | Type | In Effects? | Historical Available? |
|--------|------|-------------|----------------------|
| `0xbb8967...` | Modified | `changed_objects` ✓ | Yes, version 755312289 |
| `0x71e651...` | Read-only child | None ✗ | Only current version |

## Reproduction

```rust
let client = GrpcClient::archive().await?;
let tx = client.get_transaction_with_objects(
    "7aQBpHjvgNguGB4WoS9h8ZPgrAPfDqae25BZn5MxXoWY",
    true
).await?;

// tx.input_object_versions only contains 8 entries (changed + consensus)
// unchanged_loaded_runtime_objects is always empty
```

## Root Cause Hypothesis

1. **Archive doesn't store auxiliary data**: The `unchanged_loaded_runtime_objects` might be stored separately from the main effects BCS and the archive endpoint doesn't serve it.

2. **Field not populated during indexing**: The archive may not compute/store this field during checkpoint processing.

3. **Read mask not working**: Despite requesting the field explicitly, it's not being returned.

## Workaround Attempts

1. Explicit `read_mask` with `effects.unchanged_loaded_runtime_objects` - Still empty
2. Using `rawEffects` BCS - Field is auxiliary data, not in effects BCS
3. Fetching objects at guessed versions - Archive only has versions where objects were modified

## Proposed Solutions

### Recommended: Use a Provider with Enhanced Historical Data

Some third-party gRPC providers correctly return `unchanged_loaded_runtime_objects` and support full historical object fetching. These providers typically require API key authentication via `x-api-key` header.

```rust
// Use with_api_key for providers requiring authentication
let client = GrpcClient::with_api_key(
    "https://your-grpc-provider:443",
    Some("your-api-key".to_string()),
).await?;
```

### Alternative Solutions

#### Short-term (if third-party provider unavailable)

- Document limitation in replay tooling
- Use current state for read-only objects (accept lower accuracy)

#### Long-term

1. **Request MystenLabs populate this field** in archive gRPC responses
2. **Use checkpoint-based fetching** if full checkpoint data includes this
3. **Build local index** of all object versions from checkpoint stream

## Related

- [RPC 2.0 Issue #13700](https://github.com/MystenLabs/sui/issues/13700) - General RPC redesign
- [gRPC Overview](https://docs.sui.io/concepts/data-access/grpc-overview) - Official docs

## Environment

- Mysten archive endpoint: `archive.mainnet.sui.io:443` (missing field)
- Third-party gRPC providers with enhanced effects data (field populated correctly)
- Proto version: mainnet-v1.62.1
- Test date: 2026-01-17
