# Data Fetching Guide

This guide covers how to fetch on-chain data (objects, packages, transactions) from Sui mainnet/testnet using the unified `DataFetcher` API.

## Overview

The data fetching system provides a unified interface with two backends:

| Backend | Best For | Tradeoff |
|---------|----------|----------|
| **gRPC Streaming** | Real-time monitoring, high throughput | Limited effects data (no created/mutated/deleted) |
| **GraphQL** | Queries, packages, replay verification | Polling only, may miss transactions |

**Key features:**

- **gRPC streaming** - Subscribe to checkpoints as they're finalized (no polling gaps)
- **GraphQL queries** - Complete data including full effects for replay
- **Automatic pagination** - Handles cursor-based pagination transparently
- **Transaction parsing** - Full PTB (Programmable Transaction Block) structure

> **See [Choosing a Data Source](#choosing-a-data-source-tradeoffs)** for detailed comparison of when to use each backend.

## Quick Start

```rust
use sui_move_interface_extractor::data_fetcher::DataFetcher;

fn main() -> anyhow::Result<()> {
    // Create a fetcher for mainnet
    let fetcher = DataFetcher::mainnet();

    // Fetch an object
    let obj = fetcher.fetch_object("0x6")?;  // Clock object
    println!("Object version: {}", obj.version);

    // Fetch a package
    let pkg = fetcher.fetch_package("0x2")?;  // Sui framework
    println!("Package has {} modules", pkg.modules.len());

    // Fetch recent transactions
    let txs = fetcher.fetch_recent_ptb_transactions(10)?;
    for tx in &txs {
        println!("{}: {} commands", tx.digest, tx.commands.len());
    }

    Ok(())
}
```

## DataFetcher API

### Creating a Fetcher

```rust
// For mainnet (uses GraphQL)
let fetcher = DataFetcher::mainnet();

// For testnet
let fetcher = DataFetcher::testnet();

// Custom GraphQL endpoint
let fetcher = DataFetcher::new("https://graphql.mainnet.sui.io/graphql");
```

### Fetching Objects

```rust
let obj = fetcher.fetch_object("0x...")?;

// Returns FetchedObjectData:
// - address: String
// - version: u64
// - type_string: Option<String>
// - bcs_bytes: Option<Vec<u8>>
// - is_shared: bool
// - is_immutable: bool
// - source: DataSource (GraphQL, Grpc, or Cache)
```

### Fetching Packages

```rust
let pkg = fetcher.fetch_package("0x2")?;

// Returns FetchedPackageData:
// - address: String
// - version: u64
// - modules: Vec<FetchedModuleData>
// - source: DataSource

for module in &pkg.modules {
    println!("Module: {}", module.name);
    // module.bytecode contains the compiled Move bytecode
}
```

### Fetching Transactions

Three methods available, depending on your needs:

```rust
// 1. Just digests (fast, paginated)
let digests = fetcher.fetch_recent_transactions(100)?;

// 2. Full transaction data (includes system transactions)
let txs = fetcher.fetch_recent_transactions_full(50)?;

// 3. Only programmable transactions (filters out system txs) - RECOMMENDED
let ptb_txs = fetcher.fetch_recent_ptb_transactions(25)?;
```

**Why use `fetch_recent_ptb_transactions`?**

Sui mainnet includes system transactions (epoch changes, randomness updates) that have:

- Empty sender
- Zero gas budget
- No commands

These are valid but not useful for analyzing user activity. The `fetch_recent_ptb_transactions` method filters these out automatically.

### Transaction Structure

```rust
// GraphQLTransaction contains:
pub struct GraphQLTransaction {
    pub digest: String,
    pub sender: String,
    pub gas_budget: Option<u64>,
    pub gas_price: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub checkpoint: Option<u64>,
    pub inputs: Vec<GraphQLTransactionInput>,
    pub commands: Vec<GraphQLCommand>,
    pub effects: Option<GraphQLEffects>,
}
```

**Transaction Inputs:**

```rust
pub enum GraphQLTransactionInput {
    Pure { bytes_base64: String },
    OwnedObject { address: String, version: u64, digest: String },
    SharedObject { address: String, initial_shared_version: u64, mutable: bool },
    Receiving { address: String, version: u64, digest: String },
}
```

**PTB Commands:**

```rust
pub enum GraphQLCommand {
    MoveCall {
        package: String,
        module: String,
        function: String,
        type_arguments: Vec<String>,
        arguments: Vec<GraphQLArgument>,
    },
    SplitCoins { coin: GraphQLArgument, amounts: Vec<GraphQLArgument> },
    MergeCoins { destination: GraphQLArgument, sources: Vec<GraphQLArgument> },
    TransferObjects { objects: Vec<GraphQLArgument>, address: GraphQLArgument },
    MakeMoveVec { type_arg: Option<String>, elements: Vec<GraphQLArgument> },
    Publish { modules: Vec<String>, dependencies: Vec<String> },
    Upgrade { modules: Vec<String>, dependencies: Vec<String>, package: String, ticket: GraphQLArgument },
    Other { typename: String },
}
```

**Command Arguments:**

```rust
pub enum GraphQLArgument {
    Input(u16),           // Reference to a transaction input
    Result(u16),          // Reference to a previous command's result
    NestedResult(u16, u16), // Reference to a nested result (cmd, idx)
    GasCoin,              // The gas coin
}
```

## Searching Objects by Type

```rust
// Search for all objects of a specific type
let coins = fetcher.search_objects_by_type(
    "0x2::coin::Coin<0x2::sui::SUI>",
    100  // limit
)?;

// Pagination is handled automatically
```

## Direct GraphQL Client

For advanced use cases, you can use the GraphQL client directly:

```rust
use sui_move_interface_extractor::graphql::GraphQLClient;

let client = GraphQLClient::mainnet();

// Fetch individual transaction with full details
let tx = client.fetch_transaction("8JTTa6k7Expr15zMS2DpTsCsaMC4aV4Lwxvmraew85gY")?;

// Fetch package bytecode
let pkg = client.fetch_package("0x2")?;
for module in &pkg.modules {
    if let Some(bytecode) = &module.bytecode_base64 {
        // Process bytecode...
    }
}
```

## Custom Pagination

For custom paginated queries, use the `Paginator` helper:

```rust
use sui_move_interface_extractor::data_fetcher::{
    Paginator, PaginationDirection, PageInfo
};

let paginator = Paginator::new(
    PaginationDirection::Forward,  // or Backward
    100,  // total items to fetch
    |cursor, page_size| {
        // Your fetch function that returns (items, PageInfo)
        my_graphql_query(cursor, page_size)
    },
);

// Collect all pages
let all_items = paginator.collect_all()?;

// Or iterate page by page
let mut paginator = Paginator::new(...);
while let Some(page) = paginator.next_page()? {
    process_page(page);
}
```

## Pagination Constants

- **MAX_PAGE_SIZE**: 50 (Sui GraphQL server limit)
- Requests for more items are automatically split into multiple pages
- Uses cursor-based pagination (Relay connection spec)

## Error Handling

```rust
use anyhow::Result;

fn fetch_data() -> Result<()> {
    let fetcher = DataFetcher::mainnet();

    match fetcher.fetch_object("0x...") {
        Ok(obj) => println!("Got object: {}", obj.address),
        Err(e) => {
            // Common errors:
            // - "Object not found"
            // - "GraphQL error: ..."
            // - Network errors
            eprintln!("Failed: {}", e);
        }
    }

    Ok(())
}
```

## GraphQL Features

| Feature | Support |
|---------|---------|
| Package bytecode | ✅ Always available |
| Transaction PTB details | ✅ Full structure |
| Pagination | ✅ Cursor-based |
| Object BCS | ✅ Available |
| Historical data | ✅ Good |

**Recommendation:** Use `DataFetcher` which uses GraphQL for all queries.

## Example: Analyzing Recent Transactions

```rust
use sui_move_interface_extractor::data_fetcher::{DataFetcher, GraphQLCommand};

fn analyze_transactions() -> anyhow::Result<()> {
    let fetcher = DataFetcher::mainnet();
    let txs = fetcher.fetch_recent_ptb_transactions(50)?;

    let mut stats = std::collections::HashMap::new();

    for tx in &txs {
        for cmd in &tx.commands {
            match cmd {
                GraphQLCommand::MoveCall { package, module, function, .. } => {
                    let key = format!("{}::{}::{}", package, module, function);
                    *stats.entry(key).or_insert(0) += 1;
                }
                GraphQLCommand::TransferObjects { .. } => {
                    *stats.entry("TransferObjects".to_string()).or_insert(0) += 1;
                }
                _ => {}
            }
        }
    }

    println!("Top functions called:");
    let mut sorted: Vec<_> = stats.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (func, count) in sorted.iter().take(10) {
        println!("  {}: {}", func, count);
    }

    Ok(())
}
```

## Real-Time Streaming with gRPC

For real-time transaction monitoring, gRPC streaming is the recommended approach. Unlike polling, streaming:

- Receives every checkpoint as it's finalized (no gaps)
- More efficient (single connection vs. repeated requests)
- Lower latency (push vs. pull)

**Public gRPC Endpoints:**

| Endpoint | Streaming | Queries | Use Case |
|----------|-----------|---------|----------|
| `fullnode.mainnet.sui.io:443` | ✅ Yes | ✅ Recent | Real-time monitoring |
| `archive.mainnet.sui.io:443` | ❌ No | ✅ Full history | Historical lookups |
| `fullnode.testnet.sui.io:443` | ✅ Yes | ✅ Recent | Testing |

**Note:** The fullnode endpoint only keeps recent data (~last few epochs). For historical checkpoint queries, use the archive endpoint.

### Setting Up gRPC Streaming

```rust
use sui_move_interface_extractor::data_fetcher::DataFetcher;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create fetcher with gRPC enabled
    let fetcher = DataFetcher::mainnet()
        .with_grpc_endpoint("https://your-provider:9000")
        .await?;

    // Check connection
    let info = fetcher.get_service_info().await?;
    println!("Connected to {} at checkpoint {}", info.chain, info.checkpoint_height);

    // Subscribe to checkpoints
    let mut stream = fetcher.subscribe_checkpoints().await?;

    while let Some(result) = stream.next().await {
        let checkpoint = result?;
        println!("Checkpoint {}: {} transactions",
            checkpoint.sequence_number,
            checkpoint.transactions.len());

        for tx in &checkpoint.transactions {
            if tx.is_ptb() {
                println!("  {} - {} commands", tx.digest, tx.commands.len());
            }
        }
    }

    Ok(())
}
```

### Streaming Data Types

gRPC streaming uses slightly different types than GraphQL queries:

```rust
use sui_move_interface_extractor::data_fetcher::{
    StreamingCheckpoint,
    StreamingTransaction,
    StreamingCommand,
    StreamingInput,
};

// StreamingCheckpoint contains:
// - sequence_number: u64
// - digest: String
// - timestamp_ms: Option<u64>
// - transactions: Vec<StreamingTransaction>

// StreamingTransaction contains:
// - digest: String
// - sender: String
// - gas_budget: Option<u64>
// - inputs: Vec<StreamingInput>
// - commands: Vec<StreamingCommand>
// - status: Option<String>  // "success" or "failure"

// Check if transaction is a user PTB (not system tx)
if tx.is_ptb() {
    // Process user transaction
}
```

### When to Use Each Backend

| Use Case | Recommended Backend |
|----------|---------------------|
| Real-time monitoring | gRPC streaming |
| Fetching specific package | GraphQL |
| Fetching specific object | GraphQL |
| Historical transaction analysis | GraphQL |
| High-volume batch fetching | gRPC batch methods |
| One-time script | GraphQL (simpler setup) |

### Polling Tool (GraphQL Alternative)

If you don't have gRPC access, use the GraphQL polling tool:

```bash
# Poll transactions every 1.5 seconds for 10 minutes
cargo run --bin poll_transactions -- --duration 600 --interval 1500 --output txs.jsonl

# PTB-only mode (skip system transactions)
cargo run --bin poll_transactions -- --ptb-only --verbose
```

### Streaming Tool (gRPC)

With a gRPC endpoint:

```bash
# Set your endpoint
export SUI_GRPC_ENDPOINT="https://your-provider:9000"

# Stream for 1 minute
cargo run --bin stream_transactions -- --duration 60 --output stream.jsonl

# Stream PTB transactions only
cargo run --bin stream_transactions -- --ptb-only --verbose
```

## Choosing a Data Source: Tradeoffs

This is the most important section for deciding which backend to use. Each has distinct tradeoffs.

### Quick Decision Guide

| Your Need | Use This |
|-----------|----------|
| Real-time monitoring (every transaction) | gRPC streaming |
| Analyzing specific transactions | GraphQL |
| Fetching package bytecode | GraphQL |
| Transaction replay/simulation | GraphQL (more complete effects) |
| High-throughput collection | gRPC streaming |
| One-off scripts | GraphQL (simpler) |

### Detailed Comparison

| Feature | gRPC Streaming | GraphQL Polling |
|---------|----------------|-----------------|
| **Access** | Public (`fullnode.mainnet.sui.io:443`) | Public |
| **Real-time** | ✅ Push-based, ~250ms latency | ❌ Pull-based, polling gaps |
| **Completeness** | ✅ Every checkpoint guaranteed | ⚠️ May miss txs between polls |
| **Connection** | ⚠️ Drops every ~30s (auto-reconnect) | ✅ Stateless, reliable |

### Data Completeness Comparison

**This is critical for replay/simulation use cases:**

| Data Field | gRPC Streaming | GraphQL Polling |
|------------|----------------|-----------------|
| `digest` | ✅ | ✅ |
| `sender` | ✅ | ✅ |
| `gas_budget` | ✅ | ✅ |
| `gas_price` | ✅ | ✅ |
| `timestamp_ms` | ✅ | ✅ |
| `checkpoint` | ✅ | ✅ |
| `inputs[]` (full array) | ✅ | ✅ |
| `commands[]` (full array) | ✅ | ✅ |
| `effects.status` | ✅ | ✅ |
| `effects.created[]` | ❌ Not available | ✅ Object addresses |
| `effects.mutated[]` | ❌ Not available | ✅ Object addresses |
| `effects.deleted[]` | ❌ Not available | ✅ Object addresses |

**Key insight:** gRPC streaming provides complete PTB structure (inputs + commands) but limited effects. GraphQL provides full effects including which objects were created/mutated/deleted.

### When Effects Matter

If you need to know **what objects a transaction created or modified**, use GraphQL:

```rust
// GraphQL gives you this:
effects: {
    status: "SUCCESS",
    created: [
        { address: "0xabc...", version: 123, digest: "..." }
    ],
    mutated: [
        { address: "0xdef...", version: 124, digest: "..." }
    ],
    deleted: ["0x789..."]
}

// gRPC only gives you this:
effects: {
    status: "SUCCESS"
}
```

### Performance Comparison

| Metric | gRPC Streaming | GraphQL Polling |
|--------|----------------|-----------------|
| Throughput | ~30-40 tx/sec sustained | ~10-20 tx/sec (rate limited) |
| Latency | ~250ms from finalization | ~1-2s (polling interval) |
| Bandwidth | Efficient (binary protobuf) | Higher (JSON) |
| Rate limits | Connection-based | Request-based (~100/min) |

### Cached Data Format

Both tools save to JSONL with compatible formats:

**gRPC streaming** (`stream_transactions`):

```json
{
  "received_at_ms": 1768503538953,
  "checkpoint": 234926415,
  "digest": "B4NT8zW...",
  "sender": "0xd265...",
  "inputs": [{"SharedObject": {...}}, {"Pure": {...}}],
  "commands": [{"MoveCall": {...}}],
  "effects": {"status": "SUCCESS"}
}
```

**GraphQL polling** (`poll_transactions`):

```json
{
  "fetched_at_ms": 1768496400569,
  "digest": "2EHqmFf...",
  "sender": "0xd265...",
  "inputs": [{"SharedObject": {...}}, {"Pure": {...}}],
  "commands": [{"MoveCall": {...}}],
  "effects": {"status": "SUCCESS", "created": [...], "mutated": [...], "deleted": []}
}
```

### Recommendation Summary

| Scenario | Recommendation | Why |
|----------|----------------|-----|
| **Building a transaction indexer** | gRPC streaming | Need every tx, can fetch effects separately if needed |
| **Replaying transactions** | GraphQL | Need full effects to verify correctness |
| **Monitoring specific contracts** | gRPC streaming | Real-time, filter by package in commands |
| **Analyzing transaction patterns** | Either | Both have complete PTB structure |
| **Fetching historical transactions** | GraphQL | Better for point queries |
| **Building a block explorer** | Both | gRPC for live feed, GraphQL for details |

## Transaction Simulation via gRPC

For transaction simulation (equivalent to `dry_run` or `dev_inspect`), use the gRPC `SimulateTransaction` API:

```rust
use sui_move_interface_extractor::grpc::{
    GrpcClient, ProtoTransaction, ProtoTransactionKind,
    ProtoProgrammableTransaction, ProtoCommand, ProtoMoveCall,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = GrpcClient::new("https://fullnode.mainnet.sui.io:443").await?;

    // Build a transaction (example: simple Move call)
    let transaction = ProtoTransaction {
        sender: Some("0x...".to_string()),
        kind: Some(ProtoTransactionKind {
            kind: Some(/* TransactionKindType::ProgrammableTransaction */),
            data: Some(/* ProgrammableTransaction */),
        }),
        // ... other fields
        ..Default::default()
    };

    // Dev-inspect mode (no ownership checks)
    let result = client.dev_inspect(transaction.clone()).await?;
    println!("Success: {}, Created types: {:?}", result.success, result.created_object_types);

    // Dry-run mode (full validation)
    let result = client.dry_run(transaction, true /* do_gas_selection */).await?;
    if !result.success {
        println!("Error: {:?}", result.error);
    }

    Ok(())
}
```

### Simulation Modes

| Mode | Ownership Checks | Gas Selection | Use Case |
|------|------------------|---------------|----------|
| `dev_inspect` | ❌ No | ❌ No | Testing "what would happen?" |
| `dry_run` | ✅ Yes | ✅ Optional | Pre-flight validation |

### SimulationResult Fields

```rust
pub struct SimulationResult {
    pub success: bool,
    pub error: Option<String>,
    pub transaction: Option<GrpcTransaction>,
    pub command_outputs: Vec<CommandResultOutput>,
    pub created_object_types: Vec<String>,
}
```

## See Also

- [Transaction Replay Guide](TRANSACTION_REPLAY.md) - For replaying transactions in the sandbox
- [Sandbox API Reference](../reference/SANDBOX_API.md) - For executing PTBs locally
- [CLI Reference](../reference/CLI_REFERENCE.md) - Command-line tools
