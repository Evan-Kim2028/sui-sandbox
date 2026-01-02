# Benchmark Methodology — Phase II (Type Inhabitation)

This document describes the technical implementation and scoring rules for the Phase II inhabitation benchmark.

## Core Objective
The goal of Phase II is to measure an agent's ability to **construct valid transactions** that result in the creation of specific Move `key` structs (objects) defined in a package.

## The Mechanical Baseline (`baseline-search`)
We use a deterministic, non-LLM baseline to establish the "floor" of the benchmark. This agent follows these rules:

### 1. Candidate Selection
The agent identifies all "runnable" functions in a package. A function is runnable if:
- It is `public entry`.
- All its parameters can be constructed (see below).
- Generic type parameters are present (it automatically fills them with `0x2::sui::SUI`).

### 2. Recursive Constructor Discovery
If a function requires a struct type `T` that is not a supported primitive, the agent scans the package for a **Constructor**:
- A `public` function that returns exactly one value of type `T`.
- The agent recursively attempts to construct the parameters of this constructor.
- **Search Depth**: Limited to **3 levels** to prevent infinite recursion and excessively long transactions.

### 3. PTB Chaining
The baseline agent uses the **Programmable Transaction Block (PTB)** features of Sui to chain these calls:
1. Calls the Constructor(s).
2. Uses the `Result(i)` of the constructor as an argument for the next step.
3. Finally calls the Target function.

## Type Construction Rules
The following types have specialized construction logic:
- **`0x1::string::String`**: Constructed via `0x1::string::utf8(vector<u8>)`.
- **`0x1::ascii::String`**: Constructed via `0x1::ascii::string(vector<u8>)`.
- **`0x2::url::Url`**: Constructed via `0x2::url::new_unsafe_from_bytes(vector<u8>)`.
- **`0x1::option::Option<T>`**: Constructed via `0x1::option::none<T>()`.

## Tiered Metrics
We distinguish between the ability to *plan* a transaction and the ability to *execute* it:

| Tier | Metric | Meaning |
| :--- | :--- | :--- |
| **Selection** | `n` packages | The logic found at least one sequence of calls that *should* work. |
| **Build** | `tx_build_ok` | The binary successfully generated valid BCS bytes for the transaction. |
| **Execution** | `dry_run_ok` | The transaction successfully simulated on-chain without aborting. |
| **Score** | `hit_rate` | The percentage of target key-types actually created during execution. |

## Known Limitations
- **Inventory Dependency**: Many functions require existing objects (e.g. `&mut Pool`). The baseline currently uses a "Placeholder" system that only works if the runner has a matching object in its inventory.
- **Semantic Data**: The baseline uses "dumb" defaults (`u64: 1`, `string: "sui"`). AI agents are expected to beat the baseline by inferring more appropriate values.
