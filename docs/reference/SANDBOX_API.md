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
    {"Object": {"object_id": "0x...", "mode": "mutable"}}
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
| Object | `{"Object": {"object_id": "0x...", "mode": "mutable\|immutable\|receiving"}}` |
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
