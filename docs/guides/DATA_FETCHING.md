# Data Fetching Guide

This guide covers how to fetch on-chain data (objects, packages, transactions) from Sui mainnet/testnet.

## Choosing the Right API

| Use Case | Recommended API | Why |
|----------|-----------------|-----|
| **Historical transaction replay** | `sui_state_fetcher::HistoricalStateProvider` | Versioned cache, fetches objects at exact historical versions |
| Current state queries | `sui_transport::graphql::GraphQLClient` | Direct GraphQL access |
| Real-time streaming | `sui_transport::grpc::GrpcClient` | Native streaming client |

## Historical Transaction Replay

**For replaying historical transactions, use `sui_state_fetcher::HistoricalStateProvider`.**

This is critical because objects change between transactions - you need the exact version
of each object as it existed when the transaction was executed.

```rust
use sui_state_fetcher::HistoricalStateProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create provider for mainnet
    let provider = HistoricalStateProvider::mainnet().await?;

    // Fetch everything needed to replay a transaction
    let state = provider.fetch_replay_state("8JTTa...").await?;

    // state.transaction - PTB commands and inputs
    // state.objects - objects at their exact historical versions
    // state.packages - packages with linkage resolved

    Ok(())
}
```

The `HistoricalStateProvider`:

- Uses a **versioned cache** keyed by `(object_id, version)` - essential for historical replay
- Automatically fetches objects at their **input versions** from `unchanged_loaded_runtime_objects`
- Resolves package **linkage tables** for upgraded packages
- Provides an **on-demand fetcher** callback for dynamic field children

See the `sui-state-fetcher` crate documentation for full API details.

---

## Current State Queries (GraphQL)

Use `sui_transport::graphql::GraphQLClient` for latest on-chain state:

```rust
use sui_transport::graphql::GraphQLClient;

fn main() -> anyhow::Result<()> {
    let client = GraphQLClient::mainnet();
    let obj = client.fetch_object("0x6")?;
    let pkg = client.fetch_package("0x2")?;
    let txs = client.fetch_recent_ptb_transactions(10)?;
    println!("object version={} package modules={} txs={}",
        obj.version, pkg.modules.len(), txs.len());
    Ok(())
}
```

## Real-Time Streaming (gRPC)

Use `sui_transport::grpc::GrpcClient` for checkpoint streaming and gRPC-only queries:

```rust
use sui_transport::grpc::GrpcClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = GrpcClient::mainnet();
    let checkpoint = client.get_latest_checkpoint().await?;
    println!("latest checkpoint {:?}", checkpoint.map(|c| c.sequence_number));
    Ok(())
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
use sui_transport::graphql::GraphQLClient;

// Search for all objects of a specific type
let client = GraphQLClient::mainnet();
let coins = client.search_objects_by_type(
    "0x2::coin::Coin<0x2::sui::SUI>",
    100  // limit
)?;

// Pagination is handled automatically
```

## Direct GraphQL Client

For advanced use cases, you can use the GraphQL client directly:

```rust
use sui_transport::graphql::GraphQLClient;

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
use sui_transport::graphql::{Paginator, PaginationDirection, PageInfo};

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
use sui_transport::graphql::GraphQLClient;

fn fetch_data() -> Result<()> {
    let client = GraphQLClient::mainnet();

    match client.fetch_object("0x...") {
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

**Recommendation:** Use `sui_transport::graphql::GraphQLClient` for current-state queries.

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
use sui_transport::grpc::GrpcClient;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = GrpcClient::new("https://your-provider:9000").await?;

    // Check connection
    let info = client.get_service_info().await?;
    println!("Connected to {} at checkpoint {}", info.chain, info.checkpoint_height);

    // Subscribe to checkpoints
    let mut stream = client.subscribe_checkpoints().await?;

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
use sui_transport::grpc::{GrpcCheckpoint, GrpcCommand, GrpcInput, GrpcTransaction};

// GrpcCheckpoint contains:
// - sequence_number: u64
// - digest: String
// - timestamp_ms: Option<u64>
// - transactions: Vec<GrpcTransaction>

// GrpcTransaction contains:
// - digest: String
// - sender: String
// - gas_budget: Option<u64>
// - inputs: Vec<GrpcInput>
// - commands: Vec<GrpcCommand>
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
sui-sandbox tools poll-transactions --duration 600 --interval-ms 1500 --output txs.jsonl

# PTB-only mode (skip system transactions)
sui-sandbox tools poll-transactions --ptb-only --verbose
```

### Streaming Tool (gRPC)

With a gRPC endpoint:

```bash
# Set your endpoint
export SUI_GRPC_ENDPOINT="https://your-provider:9000"

# Stream for 1 minute
sui-sandbox tools stream-transactions --duration 60 --output stream.jsonl

# Stream PTB transactions only
sui-sandbox tools stream-transactions --ptb-only --verbose
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

**gRPC streaming** (`sui-sandbox tools stream-transactions`):

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

**GraphQL polling** (`sui-sandbox tools poll-transactions`):

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
use sui_sandbox::grpc::{
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
- [CLI Reference](../reference/CLI_REFERENCE.md) - Command-line tools
