# Schema and Stability Rules (`schema_version = 1`)

This tool emits **canonical, diff-friendly JSON** derived from Move bytecode tables and/or Sui RPC normalized interfaces.

The goal is that:

- the schema is explicit and versioned
- output is deterministic (stable ordering) for upgrade diffs
- comparison tooling can quantify robustness (mismatch counts + samples)

## 1) Bytecode-Derived Interface JSON (`--emit-bytecode-json`)

Top-level object:

- `schema_version: u64` (currently `1`)
- `package_id: string` (`0x...` hex)
- `module_names: string[]` (sorted)
- `modules: object` mapping `module_name -> Module`

### Module

`modules.<module>`:

- `address: string` (0x + 64 hex chars; normalized)
- `structs: object` mapping `StructName -> Struct`
- `functions: object` mapping `FunctionName -> Function`

### Struct

`modules.<module>.structs.<Struct>`:

- `abilities: string[]` (lowercased; canonical order: `copy`, `drop`, `store`, `key` if present)
- `type_params: TypeParam[]`
- `is_native: bool` (true if `StructFieldInformation::Native`)
- `fields: Field[]` (bytecode declaration order)

`TypeParam`:

- `constraints: string[]` (lowercased; sorted+deduped)
- `is_phantom: bool`

`Field`:

- `name: string`
- `type: Type`

### Function

`modules.<module>.functions.<fun>`:

- `visibility: "public" | "friend" | "private"`
- `is_entry: bool`
- `is_native: bool` (true if `code` is `None`)
- `type_params: FunTypeParam[]`
- `params: Type[]` (signature order)
- `returns: Type[]` (signature order)
- `acquires: StructRef[]` (sorted; currently by name)

`FunTypeParam`:

- `constraints: string[]` (lowercased; sorted+deduped)

`StructRef`:

- `address: string` (normalized)
- `module: string`
- `name: string`

## 2) Canonical Type Representation

Both the RPC normalized types and the bytecode-derived types are compared by normalizing into a shared canonical form.

`Type` is a JSON object with `kind` plus optional fields:

- primitives: `{"kind":"bool"|"u8"|"u16"|"u32"|"u64"|"u128"|"u256"|"address"|"signer"}`
- vector: `{"kind":"vector","type": Type}`
- reference: `{"kind":"ref","mutable": bool,"to": Type}`
- type parameter: `{"kind":"type_param","index": u64}`
- datatype (struct): `{"kind":"datatype","address": string,"module": string,"name": string,"type_args": Type[]}`

**Address normalization rule**: addresses are emitted/compared as `0x` + 64 lowercase hex chars.

## 3) Rigorous Compare Report (`--compare-bytecode-rpc`)

When enabled, the CLI compares:

- RPC normalized `modules.<m>.structs` and `modules.<m>.exposedFunctions`
- vs bytecode-derived `modules.<m>.structs` and `modules.<m>.functions`

Note: RPC does not expose private non-entry functions; the rigorous compare only checks what RPC exposes.

Compare report JSON (`--emit-compare-report`):

- `package_id: string`
- `summary: { ... }` (counts)
- `mismatches: [{path, reason, rpc?, bytecode?}]`

`mismatches[*].rpc` / `mismatches[*].bytecode` are included only when:

- single-package mode, and `--emit-compare-report` is used (or corpus mode with `--corpus-interface-compare-include-values`)

## 4) Determinism / Diff-Stability Rules

- JSON objects are canonicalized by sorting keys recursively.
- `module_names` is sorted.
- Module/struct/function maps are emitted in sorted key order (via JSON canonicalization).
- Field order is preserved from bytecode declaration order.
- Function param/return order is preserved from signature order.
- `acquires` is sorted for stability.

## 5) Versioning

Any breaking change to field names, shapes, or stability rules requires incrementing `schema_version`.

## 6) Corpus Outputs (`--bytecode-corpus-root ...`)

Corpus mode writes:

- `index.jsonl`: `{package_id, package_dir}` (stable, sorted by package id)
- `corpus_report.jsonl`: one `CorpusRow` per package
- `problems.jsonl`: subset of `corpus_report.jsonl` rows that fail any enabled checks
- `corpus_summary.json`: aggregate counts + pointers to artifacts
- `run_metadata.json`: argv/timestamps/RPC URL and best-effort `sui-packages` git metadata

### CorpusRow

- `package_id: string`
- `package_dir: string`
- `local: LocalBytecodeCounts` (counts from local `.mv` parsing, or zeros in `--corpus-module-names-only`)
- `local_vs_bcs: ModuleSetDiff` (local `.mv` module names vs local `bcs.json` moduleMap keys)
- `local_bytes_check?: LocalBytesCheck` (present when `--corpus-local-bytes-check` succeeds)
- `local_bytes_check_error?: string` (present when `--corpus-local-bytes-check` is enabled but fails to run)
- `rpc?: SanityCounts` + `rpc_vs_local?: ModuleSetDiff` (present when `--corpus-rpc-compare` is enabled)
- `interface_compare?: InterfaceCompareSummary` + `interface_compare_sample?` (present when `--corpus-interface-compare` is enabled)
- `error?: string` (fatal per-package error)

`LocalBytesCheck` verifies byte-for-byte integrity between:

- `bytecode_modules/<module>.mv`
- vs `bcs.json` `moduleMap[<module>]` bytes (base64 or byte-array forms)

Fields:

- `mv_modules`, `bcs_modules`: module counts in each source
- `exact_match_modules`: number of modules whose `len` and `sha256` match
- `mismatches_total`: total mismatching modules (includes missing modules)
- `missing_in_bcs`, `missing_in_mv`: module name lists
## 7) Evaluation Bundle JSON (A2A Layer)

The A2A layer produces a standardized evaluation bundle. See the JSON Schema at `docs/schemas/evaluation_bundle.schema.json`.

Top-level object:
- `schema_version`: `1`
- `benchmark`: `"phase2_inhabit"`
- `metrics`: Object containing `avg_hit_rate`, `errors`, `packages_total`, etc.
- `artifacts`: Map of result file paths (relative to run root).
- `errors`: List of per-package error detail objects.
