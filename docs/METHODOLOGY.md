# Methodology, Verification, and Limitations

This project is a **bytecode-first** analyzer for Sui Move packages.

The core idea is simple:

- the authoritative source of a published package is its compiled Move bytecode (`.mv`)
- we parse `.mv` directly and emit a **canonical, deterministic JSON** representation of the package interface
- we validate that representation with multiple feedback loops (local integrity checks and optional RPC comparisons)

## Why parsing `.mv` works (first principles)

Sui Move modules compile into a deterministic binary format (“CompiledModule”) defined by Move’s bytecode spec.
That binary contains the full set of declarations for a module:

- module identity (address + name)
- structs (abilities, type params, field names, field types, native-ness)
- functions (visibility, entry, type params, parameter/return types, acquires list, native-ness)

This tool parses those tables using `move-binary-format::file_format::CompiledModule` (from MystenLabs’ Sui/Move dependency),
which is the standard Rust implementation of the Move bytecode format.

## What we emit

- `--emit-bytecode-json` writes a canonical “interface JSON” derived from bytecode tables (including **private** functions).
- Determinism is enforced by sorting JSON keys recursively and keeping declaration order where it matters (fields, params, returns).

The exact schema and stability rules are in `docs/SCHEMA.md`.

## Verification loops (“robustness”)

There are three independent checks, from strongest-local to RPC-assisted:

### A) Local bytes integrity (no RPC)

In corpus mode, `--corpus-local-bytes-check` verifies that the dataset is internally consistent:

- `bytecode_modules/<module>.mv` bytes
- match `bcs.json` `moduleMap[<module>]` bytes

We report per-package mismatch samples with lengths and `sha256` digests.

This check answers: “Are we analyzing the same bytes that the dataset claims are the package bytes?”

### B) RPC sanity (counts + module sets)

With `--corpus-rpc-compare`, we fetch RPC normalized modules and check:

- module name sets match (RPC vs local)
- basic counts match within the known limitation that RPC exposes only `exposedFunctions`

This check answers: “Do our local artifacts correspond to what the chain reports?”

### C) Rigorous interface compare (field-by-field)

With `--corpus-interface-compare`, we compare:

- RPC normalized `structs` and `exposedFunctions`
- vs bytecode-derived `structs` and `functions`

Types are compared by normalizing both sides into a shared canonical `Type` form.

Important limitation: RPC does **not** expose private non-entry functions, so this comparison only checks what RPC exposes.

## Interpreting “false positives”

The main sources of mismatches are:

- RPC’s type representation differences (handled by canonicalization; see `docs/SCHEMA.md`)
- dataset skew/staleness (local artifacts don’t correspond to the chain anymore)
- genuine parsing/normalization bugs

The tooling is designed so mismatches include:

- a stable JSON “path”
- a reason
- (optionally) the RPC/bytecode values for fast diagnosis

## What this enables (type inhabitation benchmark)

Given a package `P`, one high-value downstream task is to discover all declared types `T` with `key`.
This tool already extracts:

- the set of `key` structs (from bytecode abilities)
- full function signatures (including private), which can help identify constructors/builders

To turn this into an AgentBeats-style benchmark, the next layer is:

- generate a PTB that attempts to create instances of as many `key` types as possible
- dry-run, inspect effects, and score by distinct `key` types created

This project focuses on the bytecode/interface substrate that makes those evaluations deterministic and diffable.
