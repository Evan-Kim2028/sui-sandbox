# Walrus-First Workflow Examples

This directory now uses typed workflow specs instead of shell wrappers.
The specs live in `examples/data/` and run via `sui-sandbox workflow run`.

These examples demonstrate the zero-setup Walrus replay path using public data:

1. Walrus checkpoint transaction/object data
2. GraphQL package/dependency fetch
3. GraphQL + JSON-RPC dynamic field fallback

No API keys are required for the Walrus-first path.

## Example specs

| Spec | Description | Notes |
|------|-------------|-------|
| `examples/data/workflow_walrus_cetus_swap.json` | Cetus LEIA/SUI swap replay | may need gRPC for older deleted DFs |
| `examples/data/workflow_walrus_deepbook_orders.json` | DeepBook cancel order replay | BigVector child objects can be missing on latest RPC |
| `examples/data/workflow_walrus_multi_swap_flash_loan.json` | Multi-DEX flash-loan replay | replay parity varies by historical object availability |

## Quick start

```bash
sui-sandbox workflow validate --spec examples/data/workflow_walrus_cetus_swap.json
sui-sandbox workflow run --spec examples/data/workflow_walrus_cetus_swap.json

sui-sandbox workflow validate --spec examples/data/workflow_walrus_deepbook_orders.json
sui-sandbox workflow run --spec examples/data/workflow_walrus_deepbook_orders.json --continue-on-error

sui-sandbox workflow validate --spec examples/data/workflow_walrus_multi_swap_flash_loan.json
sui-sandbox workflow run --spec examples/data/workflow_walrus_multi_swap_flash_loan.json
```

Direct replay/analyze is still available:

```bash
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare --verbose
sui-sandbox analyze replay <DIGEST> --source walrus --checkpoint <CP> --json --verbose
```

## Limitation summary

- Deleted/restructured dynamic field children may be unavailable from latest GraphQL/JSON-RPC.
- Older transactions with heavy dynamic-field reads can require full archive gRPC replay.
- Protocol version is inferred for Walrus-first replay and is best for recent transactions.
