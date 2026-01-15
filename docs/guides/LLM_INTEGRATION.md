# LLM Integration Guide

How to integrate an LLM with the Move VM sandbox.

## Overview

The sandbox provides a JSON-based API for LLMs to:
1. Explore Move packages (introspection)
2. Build and validate transactions
3. Execute transactions and observe effects
4. Handle errors and take corrective action

**Key principle:** The sandbox is a passive tool. It does exactly what you ask, nothing more. The LLM is responsible for:
- Deciding what to execute
- Handling errors
- Fetching missing dependencies
- Iterating until success

## Starting the Sandbox

```bash
# Interactive mode - reads JSON lines from stdin
sui-move-interface-extractor sandbox-exec --interactive

# With mainnet fetching enabled (allows fetching packages/objects on demand)
sui-move-interface-extractor sandbox-exec --interactive --enable-fetching

# With persistent state (survives restarts)
sui-move-interface-extractor sandbox-exec --interactive --state-file ./sandbox.state
```

## Tool Discovery

Every LLM session should start with tool discovery:

```json
{"action": "list_available_tools"}
```

Response includes all available operations with descriptions and examples.

## Typical Workflow

```
1. Introspect
   ├── list_modules
   ├── list_functions
   ├── get_function_info
   └── find_constructors

2. Prepare
   ├── create_object (if needed)
   ├── deploy_package_from_mainnet (if needed)
   └── validate_ptb (optional pre-check)

3. Execute
   └── execute_ptb

4. Handle Result
   ├── Success → extract return values, observe effects
   └── Error → diagnose, fix, retry
```

## Introspection Tools

### List available modules

```json
{"action": "list_modules"}
```

### List functions in a module

```json
{"action": "list_functions", "module_path": "0x2::coin"}
```

### Get function signature

```json
{
  "action": "get_function_info",
  "module_path": "0x2::coin",
  "function_name": "split"
}
```

Response includes:
- Parameter types
- Return types
- Type parameters
- Visibility (public/entry/private)

### Find constructors for a type

```json
{"action": "find_constructors", "type_path": "0x2::coin::Coin"}
```

## Building and Executing PTBs

### PTB Structure

```json
{
  "action": "execute_ptb",
  "inputs": [
    {"Pure": {"value": 1000, "value_type": "u64"}},
    {"Object": {"object_id": "0x123...", "mode": "mutable"}}
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
    },
    {
      "TransferObjects": {
        "objects": [{"Result": {"cmd": 0, "idx": 0}}],
        "recipient": {"Input": 2}
      }
    }
  ]
}
```

### Input Types

| Type | Example | Use Case |
|------|---------|----------|
| `Pure` | `{"Pure": {"value": 100, "value_type": "u64"}}` | Primitive values |
| `Object` | `{"Object": {"object_id": "0x...", "mode": "mutable"}}` | Existing objects |
| `Gas` | `{"Gas": {"budget": 10000000}}` | Gas coin reference |

### Command Types

| Command | Purpose |
|---------|---------|
| `MoveCall` | Call a Move function |
| `TransferObjects` | Transfer objects to an address |
| `SplitCoins` | Split a coin into multiple |
| `MergeCoins` | Merge coins into one |
| `MakeMoveVec` | Create a vector |
| `Publish` | Publish new modules |

### Argument References

- `{"Input": 0}` - Reference input at index 0
- `{"Result": {"cmd": 0, "idx": 0}}` - Reference return value 0 from command 0

## Pre-Validation (Optional)

Before executing, you can validate a PTB:

```json
{
  "action": "validate_ptb",
  "inputs": [...],
  "commands": [...]
}
```

This checks:
- Function exists
- Argument count matches
- Type argument count matches
- Function is public/entry (callable)
- No forward references
- Input indices are valid

**Note:** Validation is optional. The VM catches all errors anyway. Use validation for faster feedback on obvious mistakes.

## Error Handling

### Error Response Structure

```json
{
  "success": false,
  "error": "MissingPackage: 0x123::my_module",
  "error_category": "MissingPackage",
  "data": {
    "failed_command_index": 0,
    "commands_succeeded": 0,
    "error_details": {
      "address": "0x123",
      "module": "my_module"
    }
  }
}
```

### Common Errors and Responses

#### MissingPackage

```json
{"error_category": "MissingPackage", "error_details": {"address": "0x123"}}
```

**LLM should:** Fetch the package

```json
{"action": "deploy_package_from_mainnet", "address": "0x123"}
```

#### MissingObject

```json
{"error_category": "MissingObject", "error_details": {"id": "0x456"}}
```

**LLM should:** Fetch the object or create it

```json
{"action": "fetch_object_from_mainnet", "object_id": "0x456"}
```

#### ContractAbort

```json
{
  "error_category": "ContractAbort",
  "error_details": {
    "abort_code": 3,
    "module": "0x2::coin",
    "function": "split"
  }
}
```

**LLM should:** Understand the abort code and fix the issue (e.g., insufficient balance)

#### TypeMismatch / DeserializationFailed

**LLM should:** Check function signature with `get_function_info` and fix arguments

## State Management

### Objects persist across requests

```json
// Create an object
{"action": "create_object", "type_path": "0x2::coin::Coin<0x2::sui::SUI>", "fields": {...}}

// Use it in a PTB (same session)
{"action": "execute_ptb", "inputs": [{"Object": {"object_id": "0x<created_id>"}}], ...}
```

### Check current state

```json
{"action": "list_objects"}
{"action": "inspect_object", "object_id": "0x..."}
```

### Events from last transaction

```json
{"action": "get_last_tx_events"}
```

## Best Practices

### 1. Always check function signatures before calling

```json
{"action": "get_function_info", "module_path": "0x2::coin", "function_name": "split"}
```

### 2. Handle errors gracefully

Don't assume execution will succeed. Parse errors and take appropriate action.

### 3. Use verbose mode for debugging

When developing, add `--verbose` flag to see detailed execution traces.

### 4. Fetch dependencies proactively

If you know you'll need a package, fetch it before building the PTB:

```json
{"action": "deploy_package_from_mainnet", "address": "0x<known_package>"}
```

### 5. Inspect objects to understand their structure

```json
{"action": "inspect_object", "object_id": "0x...", "show_bcs": true}
```

## Complete Example

```json
// 1. Discover what's available
{"action": "list_functions", "module_path": "0x2::coin"}

// 2. Check the function we want to call
{"action": "get_function_info", "module_path": "0x2::coin", "function_name": "zero"}

// 3. Execute it
{
  "action": "execute_ptb",
  "inputs": [],
  "commands": [
    {
      "MoveCall": {
        "package": "0x2",
        "module": "coin",
        "function": "zero",
        "type_args": ["0x2::sui::SUI"],
        "args": []
      }
    }
  ]
}

// 4. Check what was created
{"action": "list_objects"}
```

## What the Sandbox Does NOT Do

- **No automatic retries** - LLM must decide to retry
- **No automatic dependency fetching** - LLM must explicitly fetch
- **No attempt/round limits** - orchestration is external
- **No PTB modification** - LLM must fix and resubmit
- **No suggestions** - errors are factual, not prescriptive

The sandbox is neutral and unopinionated. It's a tool, not an assistant.
