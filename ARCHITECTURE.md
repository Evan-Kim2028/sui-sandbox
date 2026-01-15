# Architecture

This document is the single source of truth for understanding the sui-move-interface-extractor system.

## What This Is

A **Move VM sandbox** that allows:
1. Executing Move code locally without a blockchain
2. Evaluating LLM-generated transactions in a controlled environment
3. Replaying mainnet transactions for regression testing

## System Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         External System                              │
│                   (LLM Orchestrator / Test Runner)                   │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   │ JSON over stdin/stdout
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        sandbox_exec.rs                               │
│                     (Canonical LLM API)                              │
│                                                                      │
│  SandboxRequest → execute_request() → SandboxResponse               │
│                                                                      │
│  Tools: execute_ptb, validate_ptb, load_module, create_object,      │
│         list_functions, get_function_info, deploy_from_mainnet...   │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      SimulationEnvironment                           │
│                        (simulation.rs)                               │
│                                                                      │
│  - Manages objects, packages, state                                  │
│  - Tracks events, effects, gas                                       │
│  - Provides introspection APIs                                       │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         PTBExecutor                                  │
│                           (ptb.rs)                                   │
│                                                                      │
│  - Executes Programmable Transaction Blocks                         │
│  - Chains commands (MoveCall, TransferObjects, SplitCoins, etc.)    │
│  - Tracks object mutations and results                              │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          VMHarness                                   │
│                           (vm.rs)                                    │
│                                                                      │
│  - Wraps the Move VM                                                │
│  - Configures simulation behavior (mocked crypto, clock, etc.)      │
│  - Executes individual function calls                               │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          Move VM                                     │
│                    (move-vm-runtime)                                 │
│                                                                      │
│  - Bytecode execution                                               │
│  - Native function dispatch                                         │
│  - Gas metering                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

## Two Operating Modes

### 1. Interactive Sandbox (LLM Integration)

For LLM agents to explore and execute Move transactions.

```bash
sui-move-interface-extractor sandbox-exec --interactive
```

**Characteristics:**
- Stateful across requests (objects persist)
- No limits on requests/attempts (orchestrator's responsibility)
- No automatic dependency fetching (LLM must explicitly request)
- Errors are detailed and actionable

**Workflow:**
```
LLM builds PTB
    ↓
execute_ptb()
    ↓
Success? → Done
    ↓ (Error)
LLM reads error (e.g., MissingPackage)
    ↓
LLM calls deploy_package_from_mainnet()
    ↓
LLM retries execute_ptb()
```

See: [LLM Integration Guide](docs/guides/LLM_INTEGRATION.md)

### 2. PTB Evaluation Runner (Regression Testing)

For testing sandbox accuracy against real mainnet transactions.

```bash
sui-move-interface-extractor ptb-eval --cache-dir .tx-cache/
```

**Characteristics:**
- Replays cached mainnet transactions
- Has retry logic with automatic dependency fetching
- Measures sandbox fidelity (% of transactions that replay correctly)
- Not for LLM evaluation

**Workflow:**
```
Load cached transaction from mainnet
    ↓
Execute in sandbox
    ↓
Failed due to missing package/object?
    ↓ (Yes)
Fetch from mainnet, retry (up to N times)
    ↓
Report success/failure
```

## What the Sandbox Does NOT Do

| Responsibility | Who Handles It |
|----------------|----------------|
| Counting attempts/rounds | External orchestrator |
| Time limits | External orchestrator |
| Deciding when LLM is "done" | External orchestrator |
| Fetching missing dependencies | LLM (in interactive mode) |
| Modifying PTBs to fix errors | LLM |
| Evaluating LLM performance | External system |

The sandbox is a **passive tool**. It executes what you ask and returns results. All orchestration logic lives outside.

## Key Components

| Component | File | Purpose |
|-----------|------|---------|
| `SandboxRequest` | sandbox_exec.rs | All available operations (JSON API) |
| `SimulationEnvironment` | simulation.rs | State management and execution |
| `PTBExecutor` | ptb.rs | Transaction block execution |
| `VMHarness` | vm.rs | Move VM configuration |
| `SimulationError` | simulation.rs | Structured error types |
| `TransactionEffects` | ptb.rs | Execution results |

## Error Handling

Errors are structured for programmatic handling:

```rust
enum SimulationError {
    MissingPackage { address, module },      // Fetch the package
    MissingObject { id, expected_type },     // Fetch or create the object
    ContractAbort { abort_code, module, function, message },
    TypeMismatch { expected, actual },
    DeserializationFailed { argument_index, expected_type },
    ExecutionError { message },
    SharedObjectLockConflict { object_id },
}
```

See: [Error Codes Reference](docs/reference/ERROR_CODES.md)

## Where to Go Next

| Goal | Document |
|------|----------|
| Get started quickly | [Quickstart](docs/getting-started/QUICKSTART.md) |
| Integrate with an LLM | [LLM Integration Guide](docs/guides/LLM_INTEGRATION.md) |
| Run benchmarks | [Running Benchmarks](docs/guides/RUNNING_BENCHMARKS.md) |
| Replay transactions | [Transaction Replay](docs/guides/TRANSACTION_REPLAY.md) |
| Understand errors | [Error Codes](docs/reference/ERROR_CODES.md) |
| CLI reference | [CLI Reference](docs/reference/CLI_REFERENCE.md) |
| Sandbox API details | [Sandbox API](docs/reference/SANDBOX_API.md) |
