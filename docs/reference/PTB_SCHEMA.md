# Programmable Transaction Block (PTB) Schema

This document captures the supported PTB JSON formats in this repo. There are
two slightly different schemas depending on entrypoint:

- **CLI `sui-sandbox ptb`**: compact `calls` + optional `inputs`
- **MCP `execute_ptb`**: explicit `inputs` + `commands` (see `SANDBOX_API.md`)

Both map to the same underlying PTB executor.

---

## 1) CLI PTB Spec (`sui-sandbox ptb`)

Top-level object:

```json
{
  "inputs": [
    { "u64": 1000 },
    { "imm_or_owned_object": "0xABC..." }
  ],
  "calls": [
    {
      "target": "0xADDR::module::function",
      "type_args": ["0x2::sui::SUI"],
      "args": [
        { "input": 0 },
        { "result": 0 }
      ]
    }
  ]
}
```

### Call Object
- **`target`**: Fully qualified Move function string.
- **`type_args`**: List of TypeTags (e.g., `"0x2::sui::SUI"`).
- **`args`**: List of argument references or inline values.

### Inputs
Inputs are optional; inline arguments are appended automatically. Inline args only
support pure values (no object inputs).

**CLI also accepts MCP `execute_ptb` format** (`inputs` + `commands`) so you can
use the same JSON with either interface.

Input kinds:
- **Pure values**: `u8`, `u16`, `u32`, `u64`, `u128`, `bool`, `address`,
  `vector_u8_utf8`, `vector_u8_hex`, `vector_address`, `vector_u64`
- **Object**: `{ "imm_or_owned_object": "0x..." }`
- **Shared object**: `{ "shared_object": { "id": "0x...", "mutable": true } }`

### Argument References
- **`input`**: `{ "input": 0 }`
- **`result`**: `{ "result": 0 }` (command index)
- **`nested_result`**: `{ "nested_result": [0, 1] }` (cmd index, return index)
- **`gas_coin`**: `{ "gas_coin": true }` (aliases input 0)

---

## 2) MCP `execute_ptb` Format

The MCP `execute_ptb` schema is more explicit and mirrors Sui’s internal PTB
shape. See **`docs/reference/SANDBOX_API.md`** for the full schema and examples.

At a glance:

```json
{
  "action": "execute_ptb",
  "inputs": [
    { "Pure": { "value": 1000, "value_type": "u64" } }
  ],
  "commands": [
    { "MoveCall": { "package": "0x2", "module": "coin", "function": "value", "type_args": ["0x2::sui::SUI"], "args": [{ "Input": 0 }] } }
  ]
}
```

Accepted variants:

- Inputs may use `{ "kind": "pure", "value": ..., "type": "u64" }` **or** `{ "Pure": {...} }`.
- Commands may use `{ "kind": "move_call", ... }` **or** `{ "MoveCall": {...} }`.

---

## 3) Notes and Pitfalls

- **Indexing is 0‑based** for `input`, `result`, and `nested_result`.
- **Shared object mutability** must be correct; `mutable: false` is read-only.
- **Inline args** in the CLI spec always append new inputs after the `inputs` list.
- **Strings** are represented as `vector_u8_utf8` (Move has no native `string`).
