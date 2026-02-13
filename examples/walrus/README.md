# Walrus-First CLI Examples

These wrappers are part of the main examples tree at `examples/walrus/`
(migrated from the legacy `examples_walrus/` path).

These examples demonstrate the **zero-setup** replay workflow using Walrus checkpoint
data supplemented by the public Sui GraphQL/JSON-RPC endpoints. No API keys, no gRPC
archive endpoints, no environment variables — just the CLI binary and a checkpoint number.

## How it works

The `--checkpoint <N>` flag enables the **Walrus-first** replay path:

1. **Walrus** fetches complete checkpoint data (transactions, input objects, effects)
   from decentralized storage — zero authentication
2. **GraphQL** (public, free) fetches missing packages and their transitive dependencies
3. **GraphQL + JSON-RPC** (public, free) provide dynamic field child objects on demand
   during VM execution

This hybrid approach gives you:
- **Zero authentication** — no API keys needed
- **Full package resolution** — dependency closure via GraphQL
- **Dynamic field support** — child objects fetched on demand during execution
- **Single command** — replaces 250-350 lines of Rust boilerplate

## Examples

| Script | Old Rust Example | Description | Status |
|--------|-----------------|-------------|--------|
| `walrus_replay.sh` | `deepbook_replay.rs` (316 lines) | DeepBook flash loan (expected failure) | status_match: true |
| `cetus_swap.sh` | `cetus_swap.rs` (~250 lines) | Cetus LEIA/SUI swap | needs gRPC for deleted DFs |
| `multi_swap_flash_loan.sh` | `multi_swap_flash_loan.rs` (~300 lines) | Multi-DEX arbitrage | untested |
| `deepbook_orders.sh` | `deepbook_orders.rs` (~350 lines) | DeepBook cancel order (BigVector DFs) | needs gRPC for deleted DFs |
| `analyze_replay.sh` | *(no equivalent)* | Analyze replay state without executing | works |

## Quick start

```bash
# Build with Walrus support
cargo build --bin sui-sandbox --features walrus

# Run the DeepBook flash loan example (verified: status_match=true)
bash examples/walrus/walrus_replay.sh

# Or directly:
cargo run --bin sui-sandbox --features walrus -- \
  replay D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva \
  --checkpoint 235248874 --compare --verbose
```

## Finding checkpoint numbers

To replay a transaction via Walrus, you need its checkpoint number. Get it from the
Sui JSON-RPC:

```bash
curl -s -X POST https://fullnode.mainnet.sui.io:443 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"sui_getTransactionBlock","params":["<DIGEST>",{}]}' \
  | jq '.result.checkpoint'
```

## Data flow

```
Walrus Checkpoint           GraphQL (free)              JSON-RPC (free)
  └── transactions          └── packages                └── child objects
  └── input objects         └── dependency closure           (fallback)
  └── output objects        └── dynamic field metadata
  └── effects               └── child objects
```

The Walrus-first path fetches the checkpoint, extracts the transaction and its input
objects, then supplements with GraphQL for packages and dynamic field children. For
objects that GraphQL can't find (e.g., deleted/restructured), JSON-RPC `sui_getObject`
is tried as a fallback.

## Comparison with original examples

The original Rust examples in `examples/` use the library API directly:
- 250-350 lines of boilerplate per example
- Require `SUI_GRPC_API_KEY` environment variable
- Set up gRPC clients, GraphQL clients, child fetchers, version maps
- Handle dynamic field prefetching, dependency closure, address aliasing

The Walrus CLI examples replace all of that with a single command line.

## Limitations

The Walrus-first + GraphQL hybrid path handles most replay scenarios, but has
limitations for older transactions where on-chain state has changed significantly:

- **Deleted dynamic field objects**: If a transaction accesses dynamic fields in objects
  that have been deleted or restructured since the checkpoint (e.g., Cetus skip list
  nodes that were removed during pool restructuring), the child fetcher cannot find them
  via GraphQL or JSON-RPC (latest). Use the full gRPC archive path for these.
- **Protocol version**: Not available from checkpoint metadata. The VM uses defaults,
  which should match for recent transactions.
- **Recent transactions work best**: The closer the checkpoint is to the current state,
  the more likely dynamic field objects still exist and can be fetched.
