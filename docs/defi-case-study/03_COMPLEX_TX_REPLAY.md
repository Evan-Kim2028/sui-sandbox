# Case Study 3: Complex Transaction Replay

This case study documents the automated sampling and replay of complex mainnet transactions.

## Overview

We built an automated test that:

1. Streams recent transactions from Sui mainnet via gRPC
2. Filters out simple framework transactions (SplitCoins, MergeCoins, TransferObjects)
3. Attempts to replay complex DeFi transactions in the local Move VM

## Transaction Classification

### Simple Transactions (Filtered Out)

- Only use Sui framework packages (0x1, 0x2, 0x3)
- Basic operations: `coin::split`, `coin::join`, `pay::*`, `transfer::*`
- Approximately 6% of sampled transactions

### Complex Transactions (Targeted)

- Use third-party DeFi protocols
- Include MoveCall commands to non-framework packages
- Approximately 94% of sampled transactions

## Protocol Distribution

From a sample of 50 recent transactions:

| Category | Count | Examples |
|----------|-------|----------|
| DeFi (pool) | 20+ | DeepBook, Cetus pools |
| DEX (unknown) | 5+ | Various swap protocols |
| Other | 18+ | Misc DeFi, oracles |
| NFT | 2 | Minting operations |

## Replay Infrastructure

### Components Used

1. **gRPC Client** (`GrpcClient::mainnet()`)
   - Streams live checkpoints from `fullnode.mainnet.sui.io:443`
   - Provides transaction data with full PTB structure

2. **Archive Client** (`GrpcClient::archive()`)
   - Fetches historical objects from `archive.mainnet.sui.io:443`
   - Required for object state at transaction time

3. **DataFetcher** (`DataFetcher::mainnet()`)
   - Fetches package bytecode via GraphQL
   - Enables on-demand package deployment

4. **SimulationEnvironment**
   - Local Move VM for transaction execution
   - Supports PTB execution with full effects

### Replay Flow

```text
1. Stream checkpoint from gRPC
   |
2. Filter for complex PTB transactions
   |
3. For each transaction:
   |
   +-- Extract unique packages from MoveCall commands
   |
   +-- Fetch packages via GraphQL
   |
   +-- Deploy packages to SimulationEnvironment
   |
   +-- Fetch input objects from archive
   |
   +-- Convert gRPC transaction to PTB format
   |
   +-- Execute in Move VM
   |
   +-- Record success/failure
```

## Results

### Typical Failure Categories

1. **Linker Errors (Most Common)**

   ```text
   LINKER_ERROR: Cannot find ModuleId { address: ..., name: "usdc" }
   ```

   - Cause: Type argument packages not loaded (e.g., coin types)
   - Solution: Recursively load packages referenced in type arguments

2. **Owner Validation Failures**

   ```text
   ABORTED: balance_manager::validate_owner at offset 8
   ```

   - Cause: Simulated sender differs from actual object owner
   - Solution: Set correct sender address in simulation context

3. **Missing Dynamic Fields**

   ```text
   ABORTED: dynamic_field lookup failed
   ```

   - Cause: Child objects not loaded (requires on-demand fetching)
   - Solution: Implement child object callback mechanism

4. **Time Validation Failures**

   ```text
   ABORTED: timestamp check failed
   ```

   - Cause: Clock object not at correct timestamp
   - Solution: Construct Clock with transaction's timestamp_ms

### Success Rate Analysis

| Issue Type | Frequency | Difficulty to Fix |
|------------|-----------|-------------------|
| Missing type packages | ~60% | Medium (recursive fetch) |
| Owner validation | ~20% | Easy (set sender) |
| Dynamic field missing | ~15% | Hard (child callback) |
| Other aborts | ~5% | Varies |

## Recommendations for Full Replay Support

### Short-term Improvements

1. **Recursive Type Loading**
   - Parse type arguments to extract package addresses
   - Fetch and deploy transitively referenced packages

2. **Sender Context**
   - Extract sender from transaction
   - Set as simulation context sender

3. **Clock Object Construction**
   - Use transaction timestamp_ms
   - Format: 32-byte UID + 8-byte timestamp (little-endian)

### Medium-term Improvements

1. **On-demand Child Fetching**
   - Implement callback for dynamic_field::borrow
   - Fetch children from archive when requested

2. **Historical State Caching**
   - Cache objects at their transaction-time versions
   - Enable faster replay of recent transactions

### Long-term Goals

1. **Full Transaction Effects Matching**
   - Compare replay effects with original effects
   - Validate created/mutated/deleted objects match

2. **Continuous Replay Testing**
   - Stream and replay transactions in real-time
   - Monitor for regression in replay success rate

## Running the Tests

```bash
# Sample and classify transactions
cargo test --test complex_tx_replay test_sample_and_filter_complex_transactions -- --ignored --nocapture

# Check input object availability
cargo test --test complex_tx_replay test_replay_complex_transactions -- --ignored --nocapture

# Full VM replay (includes execution)
cargo test --test complex_tx_replay test_full_vm_replay -- --ignored --nocapture
```

## Code References

- Test file: `tests/complex_tx_replay.rs`
- gRPC client: `src/grpc/client.rs`
- Simulation environment: `src/benchmark/simulation.rs`
- PTB types: `src/benchmark/ptb.rs`
