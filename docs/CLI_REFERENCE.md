# Sui Move Interface Extractor: CLI Reference

This runbook documents the intended workflow for running scans/validation and keeping results reproducible as the `sui-packages` dataset evolves.

## Before you run

1. Ensure the bytecode dataset checkout exists:
   - `<sui-packages-checkout>/`
2. Know your corpus root:
   - `<sui-packages-checkout>/packages/mainnet_most_used` (1000 packages; mostly symlinks)
3. For RPC-heavy runs, keep concurrency low to avoid rate limiting:
   - recommended: `--concurrency 1`

## Standard runs

### A) Full 1000-package rigorous validation

```bash
cd packages/sui-move-interface-extractor
cargo run --release -- \
  --bytecode-corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --out-dir out/corpus_interface_all_1000 \
  --corpus-local-bytes-check \
  --corpus-rpc-compare \
  --corpus-interface-compare \
  --concurrency 1 \
  --retries 12 --retry-initial-ms 500 --retry-max-ms 10000 \
  --emit-submission-summary results/mainnet_most_used_summary.json
```

Inspect:

- `out/corpus_interface_all_1000/corpus_summary.json`
- `out/corpus_interface_all_1000/problems.jsonl`
- `out/corpus_interface_all_1000/run_metadata.json` (dataset attribution)
- `results/mainnet_most_used_summary.json` (sanitized, shareable)

### B) Deterministic sampling (for iteration)

```bash
cd packages/sui-move-interface-extractor
cargo run --release -- \
  --bytecode-corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --out-dir out/corpus_interface_sample200 \
  --corpus-local-bytes-check \
  --corpus-sample 200 --corpus-seed 1 \
  --corpus-rpc-compare --corpus-interface-compare \
  --concurrency 1
```

This writes:

- `out/corpus_interface_sample200/sample_ids.txt`

Re-run the exact same set later using:

```bash
cargo run --release -- \
  --bytecode-corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --out-dir out/rerun_same_sample \
  --corpus-ids-file out/corpus_interface_sample200/sample_ids.txt \
  --corpus-local-bytes-check \
  --corpus-rpc-compare --corpus-interface-compare \
  --concurrency 1
```

## Interpreting results

- `problems.jsonl` is a filtered subset of packages that failed any enabled check.
- When `--corpus-interface-compare` is enabled, a package is considered “interface OK” only if:
  - structs match (abilities, type params, field names/types)
  - RPC exposed functions match (visibility, entry, type params, params/returns)

Note: the rigorous compare only matches what RPC exposes (`exposedFunctions`). Bytecode-derived JSON includes private functions too; RPC does not, so those aren’t compared in corpus mode.

### Making failures actionable

In corpus mode, each `CorpusRow` may include:

- `local_bytes_check`: per-module byte integrity summary (when `--corpus-local-bytes-check` is enabled)
- `local_bytes_check_error`: why the byte check couldn't run (parse/read errors)
- `interface_compare`: summary counts
- `interface_compare_sample`: up to `--corpus-interface-compare-max-mismatches` mismatch samples (path + reason)

To include the raw values for each mismatch sample, pass:

- `--corpus-interface-compare-include-values`

## Debugging mismatches

1. Take the failing package id(s) from `problems.jsonl` (or use `--corpus-ids-file` on a hand-made list).
2. Run a single-package compare report:

```bash
cd packages/sui-move-interface-extractor
cargo run --release -- \
  --package-id <0x...> \
  --compare-bytecode-rpc \
  --emit-compare-report out/compare_<id>.json
```

3. Inspect `out/compare_<id>.json` and focus on the first mismatch paths.

## Recording “what dataset was this run against?”

Every corpus run writes `run_metadata.json` (including best-effort `sui-packages` git HEAD when available).

Optionally maintain a human-readable snapshot log here:

- `docs/DATASET_SNAPSHOTS.md`

## Performance knobs

- For RPC-heavy runs (normalized interface + rigorous compare), use `--concurrency 1` to avoid 429s.
- For local-only scans where you only need module-name checks, use:
  - `--corpus-module-names-only`
  - (not compatible with RPC compare / interface compare)
- For local-only scans where you want a strong integrity check, use:
  - `--corpus-local-bytes-check`

For schema details and determinism rules, see **[JSON Schema](SCHEMA.md)**.
For benchmark execution, see **[Benchmark Guide](BENCHMARK_GUIDE.md)**.

## Bytecode Interface Extraction

The `--emit-bytecode-json` flag deserializes Move bytecode (.mv files) into a deterministic JSON interface. This is the core transformation that powers all benchmark evaluation.

### How it works: From Bytecode to JSON

The Rust extractor reads compiled Move bytecode and extracts type information through the following process:

1. **Read Binary Module:** 
   - Loads `.mv` files from `bytecode_modules/` directory
   - Uses `CompiledModule::deserialize_with_defaults()` to parse binary format

2. **Extract Type Tables:**
   - Reads struct definitions with field types and abilities
   - Reads function signatures (params, returns, visibility)
   - Maps Move type parameters to JSON representation

3. **Canonicalize Output:**
   - Normalizes addresses to 64-character hex (`0x` prefix)
   - Sorts struct fields by declaration order
   - Sorts abilities: `["copy", "drop", "store", "key"]` when present
   - Canonicalizes JSON keys recursively for diff stability

### Example: Concrete Transformation

**Input:** Binary bytecode file at `bytecode_modules/admin.mv` (cannot be human-read)

**Process:** Rust deserializes binary Move VM format into structured JSON

**Output:**
```json
{
  "schema_version": 1,
  "package_id": "0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7",
  "module_names": ["admin"],
  "modules": {
    "admin": {
      "address": "0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7",
      "structs": {
        "AdminCap": {
          "abilities": ["key", "drop", "store"],
          "type_params": [],
          "is_native": false,
          "fields": [
            {
              "name": "id",
              "type": {
                "kind": "u64"
              }
            }
          ]
        }
      },
      "functions": {
        "create_admin_cap": {
          "visibility": "public",
          "is_entry": true,
          "is_native": false,
          "type_params": [],
          "params": [],
          "returns": [
            {
              "kind": "datatype",
              "address": "0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7",
              "module": "admin",
              "name": "AdminCap",
              "type_args": []
            }
          ],
          "acquires": []
        }
      }
    }
  }
}
```

### Key Extracted Information

For each module, the interface JSON provides:

- **Structs:** Complete type definitions with fields and abilities
  - `abilities`: Which Move abilities are declared (`key`, `drop`, `store`, `copy`)
  - `fields`: Ordered list of field names and types
  - `is_native`: Whether struct is built-in to Move VM

- **Functions:** Complete signatures for all functions
  - `visibility`: `public`, `friend`, or `private`
  - `is_entry`: Whether function can be called in transaction
  - `params`: Input parameter types
  - `returns`: Output types (struct constructors return target types)

- **Type System:** Canonical representation of all Move types
  - Primitives: `u8`, `u64`, `bool`, `address`, etc.
  - Vectors: `{"kind": "vector", "type": T}`
  - Structs: `{"kind": "datatype", "address": "0x...", "module": "...", "name": "...", "type_args": [...]}`

### Why Bytecode-First?

This approach ensures ground truth is independent of:
- **Source code formatting** (whitespace, comments, style)
- **Compilation artifacts** (temporary locals, optimizer transformations)
- **RPC availability** (works offline, no network dependencies)

The extracted JSON represents exactly what the Move VM will execute on-chain.

### Reference Implementation

See `src/bytecode.rs` for the deserialization logic:
- `read_local_compiled_modules()`: Loads .mv files
- `build_bytecode_module_json()`: Extracts struct/function tables
- `build_bytecode_interface_value_from_compiled_modules()`: Builds complete package interface
- `signature_token_to_json()`: Converts Move types to canonical JSON
