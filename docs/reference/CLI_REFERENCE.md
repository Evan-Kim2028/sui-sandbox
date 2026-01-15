# Sui Move Interface Extractor: CLI Reference

This document provides a comprehensive reference for all CLI commands and options.

## Command Overview

| Command | Description |
|---------|-------------|
| `benchmark-local` | Type inhabitation testing with Tier A/B validation |
| `tx-replay` | Fetch and replay mainnet transactions locally |
| `ptb-eval` | Evaluate PTBs with self-healing error recovery |
| `sandbox-exec` | Interactive sandbox execution (JSON protocol for LLM agents) |
| *(default)* | Bytecode extraction and corpus validation |

---

## Quick Reference

```bash
# Type inhabitation benchmark
sui_move_interface_extractor benchmark-local --target-corpus ./bytecode --output results.jsonl

# Replay mainnet transactions
sui_move_interface_extractor tx-replay --recent 100 --cache-dir .tx-cache --download-only

# Self-healing PTB evaluation
sui_move_interface_extractor ptb-eval --cache-dir .tx-cache --enable-fetching

# Interactive sandbox for LLM
sui_move_interface_extractor sandbox-exec --interactive
```

---

## Corpus Validation (Default Mode)

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

## Local Type Inhabitation Benchmark (`benchmark-local`)

The `benchmark-local` command runs a no-chain evaluation loop. It validates if functions can be resolved and executed using only local bytecode and synthetic state. This enables deterministic, reproducible benchmarks without network access.

See [NO_CHAIN_TYPE_INHABITATION_SPEC.md](NO_CHAIN_TYPE_INHABITATION_SPEC.md) for the full technical specification.

### Usage

```bash
# Basic validation run (Tier A only - fast)
./target/release/sui_move_interface_extractor benchmark-local \
  --target-corpus /path/to/bytecode_modules \
  --output results.jsonl \
  --tier-a-only

# Full validation with VM execution (Tier A + B)
./target/release/sui_move_interface_extractor benchmark-local \
  --target-corpus /path/to/bytecode_modules \
  --output results.jsonl

# High-fidelity validation with mock state
./target/release/sui_move_interface_extractor benchmark-local \
  --target-corpus /path/to/bytecode_modules \
  --output results.jsonl \
  --restricted-state
```

### Key Options

| Flag | Description | Default |
|------|-------------|---------|
| `--target-corpus <PATH>` | Directory containing `.mv` modules to validate | Required |
| `--output <PATH>` | Destination JSONL file for results | Required |
| `--tier-a-only` | Skip VM execution (Tier B); only run bytecode/BCS validation | `false` |
| `--restricted-state` | Pre-populate VM with mock objects (`Coin`, `UID`) for Tier B | `false` |

### Validation Pipeline

The benchmark runs a multi-stage validation pipeline:

**Tier A (Preflight Validation)** - Deterministic checks without execution:
- **A1**: Bytecode-resolved call target (module/function exists, visibility)
- **A2**: Full type/layout resolution (struct definitions, abilities)
- **A3**: BCS validity for pure args (encode/decode roundtrip)
- **A4**: Object-arg typing (mutable/shared/owned matching)
- **A5**: Transaction consistency (type args, argument kinds)

**Tier B (VM Execution)** - Local Move VM harness:
- **B1**: Synthetic state harness (mock objects for common types)
- **B2**: Execution harness (success vs abort, abort code/location)

### Output Schema (JSONL)

Each line in the output file is a JSON object:

```json
{
  "target_package": "0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7",
  "target_module": "admin",
  "target_function": "create_admin_cap",
  "status": "tier_b_hit",
  "failure_stage": null,
  "failure_reason": null,
  "tier_a_details": {
    "validation_time_ms": 12,
    "bcs_roundtrip_verified": true
  },
  "tier_b_details": {
    "execution_success": true,
    "abort_code": null,
    "gas_used": 1000
  }
}
```

**Status Values:**
- `tier_a_hit`: Passed Tier A validation (preflight only)
- `tier_b_hit`: Passed both Tier A and Tier B (full validation)
- `miss`: Failed at some stage

**Failure Stages:** `A1`, `A2`, `A3`, `A4`, `A5`, `B1`, `B2`

### When to Use

| Use Case | Recommended Flags |
|----------|-------------------|
| Fast iteration | `--tier-a-only` |
| Full validation | (no extra flags) |
| Common Sui functions | `--restricted-state` |
| Air-gapped CI/CD | Any (no network required) |

### Performance

- **Tier A only**: ~6000 validations in <500ms
- **Tier A + B**: Varies by function complexity
- **Deterministic**: Same inputs = same outputs every time

## Environment Configuration

The following environment variables can be used to override default binary paths, which is particularly useful in Docker environments:

- `SMI_RUST_BIN`: Explicit path to the `sui_move_interface_extractor` binary.
- `SMI_TX_SIM_BIN`: Explicit path to the `smi_tx_sim` binary.

If these variables are set, the system will strictly honor them and fail if the file is not found.

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

---

## Transaction Replay (`tx-replay`)

Fetch real Sui mainnet transactions and replay them locally using the SimulationEnvironment.

### Usage

```bash
# Download recent transactions to cache
sui_move_interface_extractor tx-replay \
  --recent 100 \
  --cache-dir .tx-cache \
  --download-only

# Replay from cache
sui_move_interface_extractor tx-replay \
  --cache-dir .tx-cache \
  --from-cache \
  --parallel

# Replay single transaction
sui_move_interface_extractor tx-replay \
  --digest <TX_DIGEST> \
  --verbose

# Filter to framework-only transactions
sui_move_interface_extractor tx-replay \
  --cache-dir .tx-cache \
  --from-cache \
  --framework-only
```

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `--digest <DIGEST>` | Single transaction digest to replay | - |
| `--recent <N>` | Fetch N recent transactions | - |
| `--cache-dir <PATH>` | Directory for cached transactions | - |
| `--download-only` | Download to cache without replaying | `false` |
| `--from-cache` | Replay from cache instead of RPC | `false` |
| `--parallel` | Use parallel replay with rayon | `false` |
| `--threads <N>` | Number of parallel threads | CPU count |
| `--framework-only` | Only replay framework transactions (0x1, 0x2, 0x3) | `false` |
| `--rpc-url <URL>` | Custom RPC endpoint | Mainnet |
| `--testnet` | Use testnet instead of mainnet | `false` |
| `--verbose` | Show detailed output | `false` |
| `--validate` | Compare local vs on-chain effects | `false` |
| `--replay` | Full replay with package fetching | `false` |

### Workflow

1. **Download Phase**: Fetch transactions from RPC and cache as JSON
2. **Replay Phase**: Load cached transactions and execute via SimulationEnvironment
3. **Validation Phase**: Compare local execution effects with on-chain effects

### Output

Parallel replay shows summary statistics:
```
PARALLEL REPLAY RESULTS
========================================
Total transactions: 100
Successful: 95 (95.0%)
Status match: 92 (92.0%)
Time: 1234 ms (81.2 tx/s)
```

---

## PTB Evaluation (`ptb-eval`)

Evaluate cached transactions with self-healing error recovery. When execution fails, the evaluator attempts to diagnose and fix the issue automatically.

### Usage

```bash
# Basic evaluation
sui_move_interface_extractor ptb-eval \
  --cache-dir .tx-cache

# With self-healing and mainnet fetching
sui_move_interface_extractor ptb-eval \
  --cache-dir .tx-cache \
  --max-retries 3 \
  --enable-fetching \
  --show-healing

# Filter by transaction type
sui_move_interface_extractor ptb-eval \
  --cache-dir .tx-cache \
  --framework-only \
  --limit 50
```

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `--cache-dir <PATH>` | Directory containing cached transactions | `.tx-cache` |
| `--output <PATH>` | Output file for results (JSONL) | - |
| `--max-retries <N>` | Maximum self-healing retry attempts | 3 |
| `--enable-fetching` | Fetch missing packages from mainnet | `false` |
| `--framework-only` | Only evaluate framework transactions | `false` |
| `--third-party-only` | Only evaluate non-framework transactions | `false` |
| `--limit <N>` | Limit evaluation to N transactions | - |
| `--verbose` | Show detailed error information | `false` |
| `--show-healing` | Show self-healing actions taken | `false` |

### Self-Healing Actions

When a transaction fails, the evaluator attempts:

| Error | Healing Action |
|-------|----------------|
| Missing package | Deploy package from mainnet |
| Missing object | Create object with inferred type |
| Type mismatch | Re-resolve types and retry |

### Output Schema

```json
{
  "digest": "...",
  "status": "success",
  "retry_count": 1,
  "healing_actions": ["DeployPackage(0x...)"],
  "error_category": null
}
```

---

## Sandbox Execution (`sandbox-exec`)

Interactive sandbox for LLM agents using a JSON protocol over stdin/stdout.

### Usage

```bash
# Interactive mode (JSON lines)
sui_move_interface_extractor sandbox-exec --interactive

# Single request
echo '{"action": "list_modules"}' | \
  sui_move_interface_extractor sandbox-exec --input - --output -

# With state persistence
sui_move_interface_extractor sandbox-exec \
  --interactive \
  --state-file sandbox.state \
  --enable-fetching
```

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `--input <PATH>` | Input file (or `-` for stdin) | `-` |
| `--output <PATH>` | Output file (or `-` for stdout) | `-` |
| `--interactive` | JSON lines mode (continuous) | `false` |
| `--enable-fetching` | Fetch packages from mainnet on demand | `false` |
| `--bytecode-dir <PATH>` | Directory for compiled modules | - |
| `--state-file <PATH>` | Persist state between calls | - |
| `--verbose` | Show execution details to stderr | `false` |

### JSON Protocol

#### Request Format

```json
{"action": "<action_name>", ...params}
```

#### Available Actions

**Module Introspection:**
- `list_modules` - List all loaded modules
- `list_functions` - List functions in a module
- `list_structs` - List structs in a module
- `get_function_info` - Get detailed function signature
- `get_struct_info` - Get struct definition

**Type Operations:**
- `validate_type` - Check if a type is valid
- `encode_bcs` - Encode value to BCS bytes
- `decode_bcs` - Decode BCS bytes to value
- `search_types` - Search for types by pattern
- `search_functions` - Search for functions by pattern

**Execution:**
- `execute_ptb` - Execute Programmable Transaction Block
- `call_function` - Call a single function
- `create_object` - Create an object with given fields

**Package Management:**
- `load_module` - Load bytecode module
- `compile_move` - Compile Move source code
- `deploy_package` - Deploy a package

**State:**
- `get_state` - Get current sandbox state
- `reset_state` - Reset to initial state

#### Example Session

```json
// Request: List modules
{"action": "list_modules"}

// Response
{"success": true, "modules": ["0x1::string", "0x2::coin", ...]}

// Request: Execute PTB
{
  "action": "execute_ptb",
  "inputs": [{"Pure": [1, 0, 0, 0, 0, 0, 0, 0]}],
  "commands": [{
    "MoveCall": {
      "package": "0x2",
      "module": "coin",
      "function": "value",
      "type_args": ["0x2::sui::SUI"],
      "args": [{"Input": 0}]
    }
  }]
}

// Response
{
  "success": true,
  "effects": {
    "created": [],
    "mutated": [],
    "return_values": [[...]]
  }
}
```

### Integration with Python

```python
import subprocess
import json

def call_sandbox(request):
    result = subprocess.run(
        ["sui_move_interface_extractor", "sandbox-exec", "--input", "-", "--output", "-"],
        input=json.dumps(request),
        capture_output=True,
        text=True
    )
    return json.loads(result.stdout)

# Example usage
response = call_sandbox({"action": "list_modules"})
print(response["modules"])
```

---

## SimulationEnvironment Configuration

The SimulationEnvironment can be configured via CLI flags on `benchmark-local`:

| Flag | Description | Default |
|------|-------------|---------|
| `--use-ptb` | Use PTB execution (recommended) | `false` |
| `--strict-crypto` | Disable permissive crypto mocks | `false` |
| `--clock-base-ms <MS>` | Base timestamp for mock clock | 2024-01-01 |
| `--random-seed <HEX>` | Seed for deterministic random | zeros |

### Execution Modes

| Mode | Description | Use Case |
|------|-------------|----------|
| Default | Direct VM execution | Fast iteration |
| `--use-ptb` | PTB via SimulationEnvironment | Production benchmarks |
| `--restricted-state` | Pre-populated mock objects | Common Sui patterns |

---

## See Also

- [ARCHITECTURE.md](ARCHITECTURE.md) - System architecture overview
- [LOCAL_BYTECODE_SANDBOX.md](LOCAL_BYTECODE_SANDBOX.md) - Sandbox internals
- [BENCHMARK_GUIDE.md](BENCHMARK_GUIDE.md) - Benchmark execution guide
