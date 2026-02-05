# Programmable Transaction Block (PTB) Schema

This document captures the supported PTB JSON format used by the CLI.

---

## CLI PTB Spec (`sui-sandbox ptb`)

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

Accepted variants:

- Inputs may use `{ "kind": "pure", "value": ..., "type": "u64" }` **or** `{ "Pure": {...} }`.
- Commands may use `{ "kind": "move_call", ... }` **or** `{ "MoveCall": {...} }`.

---

## 3) Notes and Pitfalls

- **Indexing is 0‑based** for `input`, `result`, and `nested_result`.
- **Shared object mutability** must be correct; `mutable: false` is read-only.
- **Inline args** in the CLI spec always append new inputs after the `inputs` list.
- **Strings** are represented as `vector_u8_utf8` (Move has no native `string`).
