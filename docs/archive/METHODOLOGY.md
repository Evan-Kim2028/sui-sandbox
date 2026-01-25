# Methodology and Verification

This project is a **bytecode-first** analyzer and simulator for Sui Move packages.

---

## 1. Bytecode Extraction Methodology

The authoritative source of a published package is its compiled Move bytecode (`.mv`). We parse `.mv` directly to emit a **canonical, deterministic JSON** representation of the package interface.

### Why parsing `.mv` works (first principles)

Sui Move modules compile into a deterministic binary format ("CompiledModule") defined by Move's bytecode spec. That binary contains the full set of declarations for a module:

- **Module identity** (address + name)
- **Structs** (abilities, type params, field names, field types, native-ness)
- **Functions** (visibility, entry, type params, parameter/return types, acquires list, native-ness)

This tool parses those tables using `move-binary-format::file_format::CompiledModule` (from MystenLabs' Sui/Move dependency), the standard Rust implementation of the Move bytecode format.

### Verification loops ("robustness")

We validate the extracted representation with multiple feedback loops:

- **Local bytes integrity**: Verifies that the `.mv` bytes match the `bcs.json` module map in the corpus.
- **RPC sanity**: Compares module name sets and declaration counts with what the Sui RPC reports for the same package ID.
- **Rigorous interface compare**: Performs a field-by-field comparison between RPC-normalized modules and bytecode-derived modules.

---

## 2. Local Simulation Methodology

The **SimulationEnvironment** provides offline Move VM execution for testing and replay.

### Failure Taxonomy

When executing PTBs, failures are categorized by stage:

| Stage | Name | What Failure Indicates |
|-------|------|------------------------|
| **A1** | Target Resolution | Function/module doesn't exist in bytecode |
| **A2** | Type Layout | Unknown struct, recursive type, or unresolvable generic |
| **A3** | Type Synthesis | No constructor path to create required type |
| **A5** | Type Parameters | Generic type parameter bounds violation |
| **B1** | Constructor Execution | Dependency constructor aborted |
| **B2** | Target Execution | Function aborted (assertion or unsupported native) |

### Synthesizability Ceiling

Some types cannot be synthesized offline:

- Types requiring multi-hop constructor chains beyond current depth
- Types depending on existing chain state (shared objects)
- Types using unsupported natives (signatures, randomness)

This ceiling is a property of the sandbox, not a bug.

---

## 3. Limitations and Edge Cases

- **Private Visibility**: Our bytecode extractor captures **private** functions, which help identify constructors that RPC-based tools might miss.
- **Inventory Dependency**: Many functions require existing objects. Simulation results depend on what objects are available.
- **Generic Type Arguments**: Default type params may use `0x2::sui::SUI`, which may not always be appropriate.
- **Simulation Fidelity**: The sandbox implements a subset of Sui's native functions. See [SANDBOX_API.md](reference/SANDBOX_API.md) for supported operations.

---

## Related Documentation

- **[Architecture](ARCHITECTURE.md)** - System architecture overview
- **[CLI Reference](reference/CLI_REFERENCE.md)** - Rust CLI commands
- **[Sandbox API](reference/SANDBOX_API.md)** - Simulation environment API
- **[JSON Schema](reference/SCHEMA.md)** - Interface schemas
