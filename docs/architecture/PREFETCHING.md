# Data Prefetching Architecture

This document explains how the replay system fetches data required for transaction execution.

## Overview

Transaction replay requires fetching objects at their **historical versions** (the state before the transaction modified them). The system uses a multi-layer prefetch strategy to minimize network round-trips while ensuring all required data is available.

## The Three-Layer Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│                    Transaction Replay                            │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Layer 1: Ground Truth Prefetch                                   │
│                                                                  │
│ Source: unchanged_loaded_runtime_objects from transaction effects│
│ Coverage: ~60-80% of required objects                            │
│ Speed: Fast (single gRPC call for object list)                   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Layer 2: Predictive Prefetch (Optional)                          │
│                                                                  │
│ Source: MM2 bytecode analysis of MoveCall commands               │
│ Coverage: Improves to ~85-95% for complex DeFi transactions      │
│ Speed: Moderate (requires bytecode analysis + targeted fetches)  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Layer 3: On-Demand Fetch                                         │
│                                                                  │
│ Source: Runtime object_runtime callback during VM execution      │
│ Coverage: Catches anything layers 1-2 missed                     │
│ Speed: Slow (interrupts execution for each miss)                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Execute Transaction                           │
└─────────────────────────────────────────────────────────────────┘
```

## Layer 1: Ground Truth Prefetch

The ground truth strategy uses information directly from the transaction's on-chain effects.

### Data Sources

- **Input objects**: Explicitly referenced in the transaction
- **unchanged_loaded_runtime_objects**: Objects read but not modified (from gRPC)
- **Changed objects**: Objects that were created/mutated/deleted

### Implementation

```rust
// From sui_prefetch::eager_prefetch
pub fn ground_truth_prefetch_for_transaction(
    grpc: &GrpcClient,
    rt: &Runtime,
    tx: &GrpcTransaction,
    config: &GroundTruthPrefetchConfig,
) -> GroundTruthPrefetchResult
```

### Limitations

Ground truth only knows about objects that were *actually* accessed in the original execution. It cannot predict:

- Dynamic fields accessed through wrapper functions
- Objects accessed conditionally based on runtime values
- New access patterns if the transaction is modified

## Layer 2: Predictive Prefetch

The predictive layer uses static bytecode analysis to predict which dynamic fields will be accessed.

### Why This Exists

Many DeFi protocols use dynamic fields (tables, bags, linked lists) to store data. For example:

- DeepBook stores balances in `Table<BalanceKey<T>, Balance<T>>`
- Cetus uses skip lists with dynamic field children
- Scallop uses bags for lending positions

Ground truth catches these when replaying the *exact* original transaction, but predictive analysis:

1. Validates our understanding of the code
2. Catches accesses through wrapper functions that ground truth might not enumerate
3. Enables future use cases (modified transactions, what-if analysis)

### How It Works

```
MoveCall command
       │
       ▼
┌─────────────────┐
│ BytecodeAnalyzer│ ──► Walks bytecode instructions
└─────────────────┘     Identifies dynamic_field::* calls
       │                Extracts key/value type patterns
       ▼
┌─────────────────┐
│ CallGraph       │ ──► Builds complete call graph
└─────────────────┘     Propagates "sink" status backwards
       │                Finds transitive callers of dynamic_field
       ▼
┌─────────────────┐
│FieldAccessPred.│ ──► Resolves type parameters
└─────────────────┘     Produces concrete access predictions
       │
       ▼
┌─────────────────┐
│ KeySynthesizer  │ ──► Derives child object IDs from key types
└─────────────────┘     Handles phantom keys (empty BCS encoding)
       │
       ▼
  Fetch predicted objects
```

### Call Graph Analysis

The key innovation is **sink propagation**. A "sink" is any function that directly calls `dynamic_field::*`. We then propagate sink status backwards through the call graph:

```
table::borrow<K,V>()     ← SINK (calls dynamic_field::borrow_child_object)
       ↑
pool::get_balance<T>()   ← TRANSITIVE SINK (calls table::borrow)
       ↑
swap::execute<T>()       ← TRANSITIVE SINK (calls pool::get_balance)
```

This catches dynamic field accesses even when wrapped in multiple layers of abstraction.

### Configuration

```rust
PredictivePrefetchConfig {
    // Use ground truth as primary source
    base_config: GroundTruthPrefetchConfig::default(),

    // Enable bytecode analysis
    enable_mm2_prediction: true,

    // Enable call graph for transitive detection
    use_call_graph: true,

    // How deep to trace transitive calls
    max_transitive_depth: 10,

    // Minimum confidence to use a prediction
    min_confidence: Confidence::Medium,
}
```

## Layer 3: On-Demand Fetch

The final fallback fetches objects during VM execution when they're accessed but not pre-loaded.

### Implementation

The `VMHarness` accepts a `child_fetcher` callback:

```rust
pub type ChildFetcherFn = Box<dyn Fn(&ObjectID, &TypeTag) -> Option<Object> + Send + Sync>;
```

When the VM tries to access a child object that isn't in the local store, this callback is invoked to fetch it from the network.

### Performance Impact

On-demand fetching is slow because it:

1. Interrupts VM execution
2. Makes a synchronous network call
3. Resumes execution

The prefetch layers exist to minimize how often this happens.

## Module Organization

```
crates/sui-sandbox-core/src/
├── mm2/                           # Bytecode analysis (internal infrastructure)
│   ├── bytecode_analyzer.rs       # Instruction-level analysis
│   ├── call_graph.rs              # Transitive call tracking
│   ├── field_access_predictor.rs  # Type resolution and prediction
│   └── key_synthesizer.rs         # Child ID derivation
│
└── predictive_prefetch.rs         # Orchestrates prefetch pipeline

crates/sui-prefetch/src/
└── eager_prefetch.rs              # Ground truth prefetch implementation
```

**Important**: The `mm2/` module and `predictive_prefetch.rs` are **internal infrastructure** for the replay pipeline. They're not user-facing APIs—they exist solely to make data fetching work reliably.

## When to Use Each Strategy

| Scenario | Recommended Strategy |
|----------|---------------------|
| Simple replay (few objects) | Ground truth only |
| DeFi replay (tables, bags) | Ground truth + call graph prediction |
| Modified transactions | Full predictive analysis |
| Maximum reliability | All layers (default) |

## Metrics

The prefetcher tracks statistics to help understand coverage:

```rust
PredictionStats {
    predictions_made: usize,      // Total predictions generated
    predictions_used: usize,      // Predictions that matched ground truth
    predictions_missed: usize,    // Objects accessed but not predicted
    ground_truth_objects: usize,  // Objects from transaction effects
}
```

## See Also

- [Transaction Replay Guide](../guides/TRANSACTION_REPLAY.md) - End-to-end replay workflow
- [Data Fetching Guide](../guides/DATA_FETCHING.md) - GraphQL and gRPC client usage
