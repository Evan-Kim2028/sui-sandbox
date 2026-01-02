# Methodology, Verification, and Limitations

This project is a **bytecode-first** analyzer for Sui Move packages, combined with an automated transaction inhabitation benchmark.

---

## 1. Bytecode Extraction Methodology

The authoritative source of a published package is its compiled Move bytecode (`.mv`). We parse `.mv` directly to emit a **canonical, deterministic JSON** representation of the package interface.

### Why parsing `.mv` works (first principles)
Sui Move modules compile into a deterministic binary format (“CompiledModule”) defined by Move’s bytecode spec. That binary contains the full set of declarations for a module:
- **Module identity** (address + name)
- **Structs** (abilities, type params, field names, field types, native-ness)
- **Functions** (visibility, entry, type params, parameter/return types, acquires list, native-ness)

This tool parses those tables using `move-binary-format::file_format::CompiledModule` (from MystenLabs’ Sui/Move dependency), the standard Rust implementation of the Move bytecode format.

### Verification loops (“robustness”)
We validate the extracted representation with multiple feedback loops:

- **Local bytes integrity**: Verifies that the `.mv` bytes match the `bcs.json` module map in the corpus.
- **RPC sanity**: Compares module name sets and declaration counts with what the Sui RPC reports for the same package ID.
- **Rigorous interface compare**: Performs a field-by-field comparison between RPC-normalized modules and bytecode-derived modules.

---

## 2. Benchmark Methodology (Phase II: Inhabitation)

The goal of the Phase II inhabitation benchmark is to measure an agent's ability to **construct valid transactions** that result in the creation of specific Move `key` structs (objects) defined in a package.

### Planning Intelligence Focus
The benchmark is designed to measure **planning and inhabitation intelligence**, not JSON formatting ability:
- **Automatic Normalization**: Common LLM formatting mistakes (e.g., stringified integers, missing `0x` prefixes) are automatically corrected before simulation.
- **Planning-Only Metrics**: We compute `planning_only_hit_rate` which excludes packages that failed due to pure formatting errors.
- **Causality Validation**: We score PTB causality (whether result references point to earlier calls) independent of execution success.

### Scoring Definition
- **Targets (truth)**: All structs in the package whose abilities include `key`, derived from the bytecode interface.
- **Created types (evidence)**: Captured from transaction simulation results (dry-run).
- **Primary metric (base-type hit rate)**: We score by matching **base types**, ignoring type arguments (e.g., `Coin<SUI>` matches `Coin<FOO>`).

### The Mechanical Baseline (`baseline-search`)
We use a deterministic, non-LLM baseline establishing the benchmark's "floor":
1. **Candidate Selection**: Identifies all `public entry` functions.
2. **Recursive Constructor Discovery**: Scans for constructors (functions returning the target type) up to 3 levels deep.
3. **PTB Chaining**: Uses Sui Programmable Transaction Blocks to chain constructors and target functions.

---

## 3. Limitations and Edge Cases

- **Private Visibility**: Our bytecode extractor captures **private** functions, which help identify constructors that RPC-based tools might miss.
- **Inventory Dependency**: Many functions require existing objects. The benchmark results depend on the sender's on-chain inventory.
- **Generic Type Arguments**: The baseline heuristic fills type params with `0x2::sui::SUI`, which may not always be appropriate.
- **Simulation Strictness**: We prefer strict `dry-run` simulation for "official" scoring to ensure transaction ground-truth.

---

## Related Documentation

- [SCHEMA.md](SCHEMA.md) - Exact JSON schema and stability rules.
- [RUNBOOK.md](RUNBOOK.md) - Reproducible commands for extraction and verification.
- [A2A_COMPLIANCE.md](../benchmark/docs/A2A_COMPLIANCE.md) - Protocol implementation and testing strategy.
- [GETTING_STARTED.md](../benchmark/GETTING_STARTED.md) - Quick start for running benchmarks.
