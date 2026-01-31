# Sandbox API Reference

Complete reference for all sandbox operations.

## Request Format

All requests are JSON objects with an `action` field:

```json
{"action": "<action_name>", ...parameters}
```

## Response Format

```json
{
  "success": true|false,
  "data": {...},           // Present on success
  "error": "...",          // Present on failure
  "error_category": "...", // Error classification
  "effects": {...},        // Transaction effects (for execute_ptb)
  "events": [...],         // Emitted events (for execute_ptb)
  "gas_used": 12345        // Gas consumed (for execute_ptb)
}
```

---

## Discovery

### list_available_tools

Get all available operations with descriptions and examples.

```json
{"action": "list_available_tools"}
```

**Use this first** to discover what's available.

---

## Module Operations

### load_module

Load compiled Move bytecode from files.

```json
{
  "action": "load_module",
  "bytecode_path": "./path/to/bytecode",
  "module_name": "my_module"  // optional filter
}
```

### list_modules

List all loaded modules.

```json
{"action": "list_modules"}
```

### compile_move

Compile Move source code.

```json
{
  "action": "compile_move",
  "source": "module 0x1::test { ... }",
  "address_name": "test",
  "address_value": "0x1"
}
```

### deploy_package_from_mainnet

Fetch and deploy a package from Sui mainnet.

```json
{
  "action": "deploy_package_from_mainnet",
  "address": "0x<package_address>"
}
```

---

## Introspection

### list_functions

List functions in a module.

```json
{
  "action": "list_functions",
  "module_path": "0x2::coin"
}
```

### get_function_info

Get detailed function signature.

```json
{
  "action": "get_function_info",
  "module_path": "0x2::coin",
  "function_name": "split"
}
```

**Response includes:**

- `params`: Parameter types
- `returns`: Return types
- `type_params`: Generic parameters with constraints
- `visibility`: public/private/friend
- `is_entry`: Whether it's an entry function

### list_structs

List structs in a module.

```json
{
  "action": "list_structs",
  "module_path": "0x2::coin"
}
```

### get_struct_info

Get struct field information.

```json
{
  "action": "get_struct_info",
  "module_path": "0x2::coin",
  "struct_name": "Coin"
}
```

### find_constructors

Find functions that return a given type.

```json
{
  "action": "find_constructors",
  "type_path": "0x2::coin::Coin"
}
```

### search_types

Search for types by pattern.

```json
{
  "action": "search_types",
  "pattern": "*Coin*",
  "ability_filter": "store"  // optional: key, store, copy, drop
}
```

### search_functions

Search for functions by pattern.

```json
{
  "action": "search_functions",
  "pattern": "*transfer*",
  "entry_only": true  // optional
}
```

---

## Object Management

### create_object

Create an object with specific field values.

```json
{
  "action": "create_object",
  "object_type": "0x2::coin::Coin<0x2::sui::SUI>",
  "fields": {
    "id": "0x123...",
    "balance": {"value": 1000000000}
  },
  "object_id": "0x..."  // optional, auto-generated if omitted
}
```

### create_test_object

Create a test object with minimal configuration.

```json
{
  "action": "create_test_object",
  "type_path": "0x2::coin::Coin<0x2::sui::SUI>",
  "owner": "0x<owner_address>",
  "initial_value": 1000000000  // optional
}
```

### list_objects

List all objects in sandbox.

```json
{"action": "list_objects"}
```

### inspect_object

Get object details.

```json
{
  "action": "inspect_object",
  "object_id": "0x..."
}
```

### fetch_object_from_mainnet

Fetch an object from Sui mainnet.

```json
{
  "action": "fetch_object_from_mainnet",
  "object_id": "0x..."
}
```

---

## PTB Execution

### execute_ptb

Execute a Programmable Transaction Block.

```json
{
  "action": "execute_ptb",
  "inputs": [
    {"Pure": {"value": 1000, "value_type": "u64"}},
    {"Object": {"object_id": "0x...", "mode": "mutable"}},
    {"Object": {"object_id": "0x...", "mode": "shared", "mutable": false}}
  ],
  "commands": [
    {
      "MoveCall": {
        "package": "0x2",
        "module": "coin",
        "function": "split",
        "type_args": ["0x2::sui::SUI"],
        "args": [{"Input": 1}, {"Input": 0}]
      }
    }
  ]
}
```

#### Input Types

| Type | Format |
|------|--------|
| Pure | `{"Pure": {"value": <json>, "value_type": "<type>"}}` |
| Object | `{"Object": {"object_id": "0x...", "mode": "mutable\|immutable\|receiving\|shared", "mutable": true|false}}` |
| Gas | `{"Gas": {"budget": 10000000}}` |
| Witness | `{"Witness": {"witness_type": "0x...::Type"}}` |

#### Command Types

| Command | Format |
|---------|--------|
| MoveCall | `{"MoveCall": {"package": "0x...", "module": "...", "function": "...", "type_args": [...], "args": [...]}}` |
| TransferObjects | `{"TransferObjects": {"objects": [...], "recipient": <arg>}}` |
| SplitCoins | `{"SplitCoins": {"coin": <arg>, "amounts": [...]}}` |
| MergeCoins | `{"MergeCoins": {"target": <arg>, "sources": [...]}}` |
| MakeMoveVec | `{"MakeMoveVec": {"element_type": "...", "elements": [...]}}` |
| Publish | `{"Publish": {"modules": ["<base64>", ...], "dependencies": ["0x...", ...]}}` |
| Receive | `{"Receive": {"object_id": "0x...", "object_type": "..."}}` |

#### Argument References

| Reference | Format |
|-----------|--------|
| Input | `{"Input": <index>}` |
| Result | `{"Result": {"cmd": <cmd_index>, "idx": <return_index>}}` |

### validate_ptb

Validate a PTB without executing.

```json
{
  "action": "validate_ptb",
  "inputs": [...],
  "commands": [...]
}
```

**Returns:**

- Function existence check
- Argument count validation
- Type argument count validation
- Visibility check (public/entry)
- Reference validity (no forward refs, bounds check)

### call_function

Direct function call (simpler than PTB for single calls).

```json
{
  "action": "call_function",
  "package": "0x2",
  "module": "coin",
  "function": "value",
  "type_args": ["0x2::sui::SUI"],
  "args": [{"object_id": "0x..."}]
}
```

---

## Events

### list_events

List all events across all transactions.

```json
{"action": "list_events"}
```

### get_last_tx_events

Get events from the last transaction.

```json
{"action": "get_last_tx_events"}
```

### get_events_by_type

Filter events by type prefix.

```json
{
  "action": "get_events_by_type",
  "type_prefix": "0x2::coin"
}
```

### clear_events

Clear all recorded events.

```json
{"action": "clear_events"}
```

---

## Coins

### register_coin

Register a custom coin type.

```json
{
  "action": "register_coin",
  "coin_type": "0xabc::my_coin::MY_COIN",
  "decimals": 9,
  "symbol": "MYCOIN",
  "name": "My Coin"
}
```

### get_coin_metadata

Get coin metadata.

```json
{
  "action": "get_coin_metadata",
  "coin_type": "0x2::sui::SUI"
}
```

### list_coins

List all registered coins.

```json
{"action": "list_coins"}
```

---

## Clock & Time

### get_clock

Get current simulated time.

```json
{"action": "get_clock"}
```

### set_clock

Set simulated time.

```json
{
  "action": "set_clock",
  "timestamp_ms": 1700000000000
}
```

---

## Shared Objects

### list_shared_objects

List all shared objects.

```json
{"action": "list_shared_objects"}
```

### get_shared_object_info

Get shared object details including lock status.

```json
{
  "action": "get_shared_object_info",
  "object_id": "0x..."
}
```

### list_shared_locks

List all shared object locks.

```json
{"action": "list_shared_locks"}
```

### get_lamport_clock

Get current Lamport timestamp.

```json
{"action": "get_lamport_clock"}
```

### advance_lamport_clock

Advance Lamport timestamp.

```json
{"action": "advance_lamport_clock"}
```

---

## Utilities

### encode_bcs

Encode a value to BCS bytes.

```json
{
  "action": "encode_bcs",
  "type_str": "u64",
  "value": 12345
}
```

### decode_bcs

Decode BCS bytes to value.

```json
{
  "action": "decode_bcs",
  "type_str": "u64",
  "bytes": "39300000000000"
}
```

### validate_type

Check if a type string is valid.

```json
{
  "action": "validate_type",
  "type_str": "0x2::coin::Coin<0x2::sui::SUI>"
}
```

### generate_id

Generate a new unique object ID.

```json
{"action": "generate_id"}
```

### parse_address

Parse an address string.

```json
{
  "action": "parse_address",
  "address": "0x2"
}
```

### compute_hash

Compute hash of data.

```json
{
  "action": "compute_hash",
  "data": "hello",
  "algorithm": "sha256"
}
```

---

## State Management

### get_state

Get current sandbox state summary.

```json
{"action": "get_state"}
```

### reset

Reset sandbox to initial state.

```json
{"action": "reset"}
```

### save_state

Save the current simulation state to a file. This enables "save game" functionality where you can persist your setup (imported packages, objects, custom modules) and resume later.

```json
{
  "action": "save_state",
  "path": "./my-simulation.json",
  "description": "Cetus pool setup with 1000 SUI",
  "tags": ["defi", "cetus"]
}
```

**What gets saved:**

- All objects (BCS bytes + metadata)
- User-deployed modules (bytecode)
- Coin registry (custom coins)
- Dynamic fields (Table/Bag data)
- Pending receives (send-to-object pattern)
- Simulation config (epoch, gas budget, clock settings)
- Sender address, ID counter, timestamp
- Metadata (description, tags, timestamps)
- **Fetcher config** (network, endpoint, archive mode) - auto-reconnects on load

**What does NOT get saved:**

- Framework modules (0x1, 0x2, 0x3) - always reloaded
- Active RPC connections - recreated from fetcher config on load
- Transaction counters - reset on load

**Auto-reconnection:** If the saved state had mainnet/testnet fetching enabled, loading the state will automatically re-establish the connection with the same settings.

### load_state

Load a previously saved simulation state from a file.

```json
{
  "action": "load_state",
  "path": "./my-simulation.json"
}
```

**Note:** Loading state merges with current state. To start fresh from a save file, use `reset` first or start a new session.

### set_sender

Set the sender address for subsequent transactions. Useful for multi-user scenarios.

```json
{
  "action": "set_sender",
  "address": "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
}
```

### get_sender

Get the current sender address.

```json
{"action": "get_sender"}
```

---

## State Persistence (CLI)

The sandbox supports persistent state via CLI flags, enabling "save game" workflows:

```bash
# Ephemeral mode (default) - state lost on exit
sui-sandbox sandbox-exec --interactive

# Persistent mode - auto-loads and auto-saves state
sui-sandbox sandbox-exec --interactive --state-file ./my-sim.json

# Read-only mode - loads state but doesn't save changes
sui-sandbox sandbox-exec --interactive --state-file ./my-sim.json --no-save-state
```

### Typical Workflow

#### Session 1: Set up simulation with mainnet state

```bash
sandbox-exec --interactive --state-file ./defi-sim.json --enable-fetching
```

```json
{"action": "import_package_from_mainnet", "package_id": "0x...cetus"}
{"action": "import_object_from_mainnet", "object_id": "0x...pool"}
{"action": "compile_move", "package_name": "strategy", "module_name": "my_strategy", "source": "module strategy::my_strategy { ... }"}
```

Exit - state saved automatically.

#### Session 2: Resume where you left off

```bash
sandbox-exec --interactive --state-file ./defi-sim.json
```

All packages, objects, and custom modules are restored.

---

## Module Analysis

### disassemble_function

Get bytecode disassembly for a function.

```json
{
  "action": "disassemble_function",
  "module_path": "0x2::coin",
  "function_name": "split"
}
```

### disassemble_module

Get full module disassembly.

```json
{
  "action": "disassemble_module",
  "module_path": "0x2::coin"
}
```

### module_summary

Get module summary (function count, struct count, etc.).

```json
{
  "action": "module_summary",
  "module_path": "0x2::coin"
}
```

### get_module_dependencies

Get module's dependencies.

```json
{
  "action": "get_module_dependencies",
  "module_path": "0x2::coin"
}
```

---

## System Objects

### get_system_object_info

Get information about system objects.

```json
{
  "action": "get_system_object_info",
  "object_name": "clock"  // clock, random, deny_list, system_state
}
```

---

## Framework

### is_framework_cached

Check if Sui framework is cached locally.

```json
{"action": "is_framework_cached"}
```

### ensure_framework_cached

Download and cache Sui framework if needed.

```json
{"action": "ensure_framework_cached"}
```

---

## Native Function Coverage

The sandbox implements Sui's native functions using fastcrypto (Mysten Labs' crypto library), providing 1:1 compatibility with mainnet.

### Real Implementations (fastcrypto)

All cryptographic operations use the same library as Sui validators.

**Hash Functions:**

- `hash::sha2_256` - Real SHA2-256
- `hash::sha3_256` - Real SHA3-256
- `hash::keccak256` - Real Keccak-256
- `hash::blake2b256` - Real Blake2b-256

**Signature Verification:**

- `ed25519::ed25519_verify` - Real Ed25519 verification
- `ecdsa_k1::secp256k1_verify` - Real secp256k1 verification
- `ecdsa_k1::secp256k1_ecrecover` - Real public key recovery
- `ecdsa_k1::decompress_pubkey` - Real pubkey decompression
- `ecdsa_r1::secp256r1_verify` - Real secp256r1 verification
- `ecdsa_r1::secp256r1_ecrecover` - Real public key recovery
- `bls12381::bls12381_min_pk_verify` - Real BLS12-381 verification
- `bls12381::bls12381_min_sig_verify` - Real BLS12-381 verification

**ZK Proof Verification:**

- `groth16::prepare_verifying_key_internal` - Real key preparation (BN254, BLS12-381)
- `groth16::verify_groth16_proof_internal` - Real proof verification (BN254, BLS12-381)

**Group Operations (BLS12-381):**

- `group_ops::internal_validate` - Real element validation (Scalar, G1, G2, GT)
- `group_ops::internal_add` - Real group element addition
- `group_ops::internal_sub` - Real group element subtraction
- `group_ops::internal_mul` - Real scalar multiplication
- `group_ops::internal_div` - Real division (scalar inverse multiplication)
- `group_ops::internal_hash_to` - Real hash-to-curve (G1, G2)
- `group_ops::internal_multi_scalar_mul` - Real multi-scalar multiplication
- `group_ops::internal_pairing` - Real pairing operation
- `group_ops::internal_sum` - Real element sum

**Core Operations (move-stdlib-natives):**

- `vector::*` - All vector operations (real)
- `bcs::to_bytes` - BCS serialization (real)
- `string::*` - UTF-8 string operations (real)
- `type_name::*` - Type name introspection (real)

### Simulated (Correct Behavior, In-Memory State)

**Transfer Operations:**

- `transfer::transfer` - Ownership transfer (tracks state)
- `transfer::public_transfer` - Public transfer
- `transfer::share_object` - Share object
- `transfer::freeze_object` - Freeze object
- `transfer::receive` - Receive transferred object

**Object Operations:**

- `object::new` - Create new UID (generates IDs)
- `object::delete` - Delete UID
- `object::id` - Get object ID

**Dynamic Fields:**

- `dynamic_field::add` - Add dynamic field (stores in BTreeMap)
- `dynamic_field::borrow` - Borrow field
- `dynamic_field::borrow_mut` - Borrow field mutable
- `dynamic_field::remove` - Remove field
- `dynamic_field::exists_` - Check existence
- `dynamic_object_field::*` - All object field variants

**Events:**

- `event::emit` - Captured in memory

**Transaction Context:**

- `tx_context::sender` - Returns configured sender
- `tx_context::digest` - Returns generated digest
- `tx_context::epoch` - Returns configured epoch
- `tx_context::fresh_object_address` - Counter-based generation

### Still Mocked (Not Yet Implemented)

**VRF:**

- `ecvrf::ecvrf_verify` - Always returns true

### Configurable

**Clock:**

- `clock::timestamp_ms` - Returns configurable timestamp (default: 2024-01-01 00:00:00 UTC)
- Set via `set_clock` action

**Randomness:**

- `random::new_generator` - Returns deterministic generator
- `random::generate_*` - Produces reproducible values based on configured seed
- Intentionally deterministic for test reproducibility

**Gas:**

- Gas metering runs but limits are configurable
- Default: permissive limits for exploration
- Use `sui_dryRunTransactionBlock` for exact mainnet gas estimates

### Not Implemented (Out of Scope)

**Validator/System Operations:**

- `sui_system::request_add_validator`
- `sui_system::request_withdraw_stake`
- `validator::*` governance operations

These are governance operations that only make sense in validator context.
