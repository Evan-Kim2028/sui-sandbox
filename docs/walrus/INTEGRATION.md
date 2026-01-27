# Walrus Integration - Phase 1: Checkpoint Replay

**Status**: ✅ Phase 1 Complete (Proof of Concept)

This document describes the integration of Walrus decentralized storage as a historical checkpoint data source for sui-sandbox.

## Overview

Walrus is a decentralized storage network that archives Sui blockchain checkpoints. This integration allows sui-sandbox to fetch historical checkpoint data from Walrus, providing an alternative to gRPC/GraphQL for certain use cases.

## What Walrus Provides

- **Checkpoint Archival**: Complete checkpoint data (transactions + effects + objects)
- **Public Access**: Free, unauthenticated reads via aggregator
- **Efficient Retrieval**: Byte-range requests for specific checkpoints
- **PostgreSQL Metadata**: Fast queries to locate checkpoints
- **REST API**: Simple HTTP endpoints for data access

## Architecture

```
sui-sandbox
    ↓
WalrusClient (sui-transport)
    ↓
Walrus Caching Server (metadata)
    ↓
Walrus Aggregator (checkpoint data)
    ↓
CheckpointData (BCS-encoded)
```

## Usage

### Basic Checkpoint Fetching

```rust
use sui_transport::walrus::WalrusClient;

// Connect to mainnet archival
let client = WalrusClient::mainnet();

// Get latest archived checkpoint
let latest = client.get_latest_checkpoint()?;
println!("Latest: {}", latest);

// Fetch checkpoint data
let checkpoint = client.get_checkpoint(latest)?;
println!("Transactions: {}", checkpoint.transactions.len());
```

### List Available Blobs

```rust
// List checkpoint blobs
let blobs = client.list_blobs(Some(10))?;

for blob in blobs {
    println!("Blob {} covers checkpoints {}-{}",
        blob.blob_id,
        blob.start_checkpoint,
        blob.end_checkpoint
    );
}
```

### Find Checkpoint Location

```rust
// Find which blob contains a checkpoint
let checkpoint = 12345;
if let Some(blob) = client.find_blob_for_checkpoint(checkpoint)? {
    println!("Checkpoint {} is in blob {}", checkpoint, blob.blob_id);
}
```

## Running Examples

### Example 1: Checkpoint Replay PoC

Fetches the latest checkpoint and analyzes its contents:

```bash
cargo run --example walrus_checkpoint_replay
```

**Output includes:**
- Latest archived checkpoint number
- Blob coverage statistics
- Transaction breakdown (PTBs, system, other)
- Execution results (success/failure rates)
- Object availability analysis
- Simulation feasibility assessment

### Example 2: PTB Analysis

Analyzes PTB requirements and identifies what data is needed:

```bash
cargo run --example walrus_ptb_analysis
```

**Output includes:**
- Required objects and their versions
- Required packages
- Objects available in checkpoint vs needing historical fetch
- Simulation strategy recommendations
- Tools and infrastructure needed

## Cost Model

### Walrus Data Access

- **Aggregator Reads**: FREE (publicly accessible)
- **No Authentication**: Anyone can query checkpoints
- **Rate Limits**: Unknown (appears unlimited currently)
- **Storage Costs**: Paid by archival service (not users)

### Why It's Free

Walrus uses a "storage pays, reads subsidized" model. The archival service pays for storage, and the decentralized network subsidizes reads. This makes historical data access essentially free for consumers.

## Current Capabilities

### ✅ What Works Now (Phase 1)

- Fetch any archived checkpoint by number
- Get latest archived checkpoint
- List available checkpoint blobs
- Extract full transaction data from checkpoints
- Analyze transaction types and requirements
- Examine execution results and effects

### ⚠️ What's Limited

- **Historical Objects**: Checkpoints contain object *references* (ID + version), but not always full object data
- **Package Bytecode**: Still need gRPC/GraphQL for Move package modules
- **Random Access**: Efficient for sequential replay, less so for arbitrary object queries
- **Coverage**: Walrus archives from a certain epoch forward; older data may not be available

## Simulation Feasibility

### Self-Contained Transactions

**Definition**: PTBs that only use objects created/modified within the same checkpoint.

**Status**: ✅ Can simulate immediately
- All required data is in the checkpoint
- No additional fetching needed
- Estimate: ~10-20% of PTBs are self-contained

### Transactions Needing Historical Data

**Definition**: PTBs that reference objects from previous checkpoints.

**Status**: ⚠️ Requires additional infrastructure
- Need object-version index (map object+version → checkpoint)
- Must fetch historical checkpoints
- Extract objects from checkpoint data

**Solution Path**: See Phase 2 below

## Next Steps

### Phase 2: Object-Version Index (2-3 weeks)

Build a PostgreSQL index mapping `(object_id, version) → checkpoint_number`:

```sql
CREATE TABLE object_versions (
    object_id TEXT,
    version BIGINT,
    checkpoint_number BIGINT,
    PRIMARY KEY (object_id, version)
);

CREATE INDEX idx_object_lookup ON object_versions (object_id, version);
```

**Build Process:**
1. Scan archived checkpoints sequentially
2. For each transaction, extract object versions from effects
3. Insert mappings: (object_id, version, checkpoint_number)
4. Index enables O(log n) lookup for any object version

**Estimated Index Size:**
- ~100 bytes per object-version entry
- ~1M objects per checkpoint (varies)
- ~1GB per 10,000 checkpoints

### Phase 3: Intelligent Fetcher (2-3 weeks)

Implement hybrid fetcher with smart routing:

```rust
pub async fn fetch_object_version(
    &self,
    object_id: ObjectID,
    version: u64
) -> Result<VersionedObject> {
    // 1. Try gRPC for recent data (fast, direct)
    if version_is_recent(version) {
        if let Ok(obj) = self.grpc.get_object_at_version(...).await {
            return Ok(obj);
        }
    }

    // 2. Try Walrus for archived data
    if let Some(checkpoint) = self.walrus_index.find_checkpoint(object_id, version).await? {
        let checkpoint_data = self.walrus.get_checkpoint(checkpoint).await?;
        if let Some(obj) = extract_object(checkpoint_data, object_id, version) {
            return Ok(obj);
        }
    }

    // 3. Fallback to GraphQL (current version)
    self.graphql.fetch_object(object_id).await
}
```

### Phase 4: Package Archival (Future)

Options for Move package bytecode:
1. Extend Walrus to archive packages separately
2. Build local package cache from gRPC
3. Use hybrid: common packages cached, rare packages fetched on-demand

## Use Cases

### Perfect for Walrus

1. **Sequential Checkpoint Replay**: Replaying checkpoints in order
2. **Epoch Analysis**: Analyzing entire epochs worth of data
3. **Batch Analytics**: Processing large ranges of checkpoints
4. **Disaster Recovery**: Permanent, decentralized data availability

### Better with gRPC

1. **Random Object Access**: Fetching arbitrary objects by ID
2. **Real-time Data**: Latest checkpoint data (gRPC is faster)
3. **Package Discovery**: Exploring and downloading Move packages

### Hybrid Approach (Recommended)

- **Recent data** (last ~1000 checkpoints): Use gRPC for speed
- **Archived data** (older checkpoints): Use Walrus for availability
- **Packages**: Cache aggressively, fetch from gRPC on miss

## Implementation Details

### WalrusClient API

Located in: `crates/sui-transport/src/walrus.rs`

```rust
pub struct WalrusClient {
    caching_url: String,      // Metadata server
    aggregator_url: String,   // Blob data server
    http_client: ureq::Agent, // Synchronous HTTP client
}

impl WalrusClient {
    pub fn mainnet() -> Self;
    pub fn testnet() -> Self;
    pub fn new(caching_url: String, aggregator_url: String) -> Self;

    // Core API
    pub fn get_latest_checkpoint(&self) -> Result<u64>;
    pub fn get_checkpoint(&self, checkpoint: u64) -> Result<CheckpointData>;
    pub fn get_checkpoint_metadata(&self, checkpoint: u64) -> Result<CheckpointInfoResponse>;

    // Discovery API
    pub fn list_blobs(&self, limit: Option<usize>) -> Result<Vec<BlobInfo>>;
    pub fn find_blob_for_checkpoint(&self, checkpoint: u64) -> Result<Option<BlobInfo>>;

    // Low-level API
    fn fetch_checkpoint_bytes(&self, blob_id: &str, offset: u64, length: u64) -> Result<Vec<u8>>;
}
```

### Endpoints Used

#### Metadata Server (Caching Server)
- `GET /v1/app_info_for_homepage` - Latest checkpoint info
- `GET /v1/app_checkpoint?checkpoint=<N>` - Checkpoint metadata (blob_id, offset, length)
- `GET /v1/app_blobs` - List archived blobs

#### Walrus Aggregator
- `GET /v1/blobs/{blob_id}/byte-range?start={offset}&length={length}` - Fetch checkpoint bytes

## Testing

### Unit Tests

```bash
# Run tests (requires network access)
cd crates/sui-transport
cargo test walrus -- --ignored
```

### Manual Testing

```bash
# Test checkpoint fetching
cargo run --example walrus_checkpoint_replay

# Test PTB analysis
cargo run --example walrus_ptb_analysis
```

## Troubleshooting

### "Transaction not found" or 404 errors

**Cause**: Checkpoint not yet archived or outside Walrus coverage range

**Solution**: Check latest archived checkpoint:
```rust
let latest = client.get_latest_checkpoint()?;
```

### BCS decoding errors

**Cause**: Blob data corruption or version mismatch

**Solution**: Ensure sui-types version matches archival service (mainnet-v1.62.0 currently)

### Slow fetches

**Cause**: Cold blob retrieval from decentralized storage

**Solution**:
- First fetch may be slow (~1-2s)
- Subsequent fetches are cached by aggregator (<100ms)
- Use batch operations when possible

## Performance Characteristics

### Latency

- **Metadata query** (PostgreSQL): 10-50ms
- **Blob fetch** (cold): 500-2000ms
- **Blob fetch** (cached): 50-200ms
- **BCS decode**: 10-100ms

### Throughput

- **Sequential checkpoint replay**: ~100-200 checkpoints/sec (cached)
- **Random checkpoint access**: ~1-5 checkpoints/sec (uncached)
- **Batch fetching**: Limited by network bandwidth

### Caching Strategy

Walrus aggregator caches popular blobs:
- Recent checkpoints: Hot (fast retrieval)
- Older checkpoints: Cold (slower first access)
- Frequently accessed: Warmed (stays in cache)

## Security Considerations

- **Trust Model**: Walrus uses erasure coding and cryptographic verification
- **Data Integrity**: Blob IDs are content-addressed (tamper-evident)
- **Availability**: Decentralized storage (no single point of failure)
- **Privacy**: All checkpoint data is public (no PII concerns)

## References

- Walrus Archival Repo: https://github.com/MystenLabs/walrus-sui-archival
- Walrus Documentation: https://docs.walrus.space/
- Sui Checkpoint Format: See `sui_types::full_checkpoint_content::CheckpointData`

## Credits

Phase 1 implementation by [Your Name] as proof of concept for Walrus integration into sui-sandbox.
