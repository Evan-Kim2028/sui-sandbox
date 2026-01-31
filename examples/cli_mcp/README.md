# CLI + MCP Example Suite

This folder contains CLI equivalents for the original Rust examples, implemented
using **`sui-sandbox tool` (MCP interface)** and, where applicable, the legacy
CLI (`sui-sandbox replay`). Each script validates success and writes JSON outputs
into `/tmp/sui-sandbox-cli-mcp-examples/outputs`.

## Requirements

- `cargo build --release --bin sui-sandbox`
- `.env` with `SUI_GRPC_ENDPOINT` / `SUI_GRPC_API_KEY` (optional but recommended)

The scripts auto-load `.env` and default to:

- gRPC: `https://fullnode.mainnet.sui.io:443`
- GraphQL: `https://graphql.mainnet.sui.io/graphql`

## Scripts

- `00_mcp_workflow.sh` – MCP CLI workflow (wrapper around `examples/mcp_cli_workflow.sh`)
- `01_ptb_basics.sh` – Split + transfer coins via `execute_ptb`
- `02_fork_state.sh` – Load DeepBook state + deploy local package + call both
- `03_cetus_position_fees.sh` – Load Cetus packages + config object + interface
- `04_cetus_swap_replay.sh` – Replay Cetus swap (legacy + MCP)
- `05_deepbook_replay.sh` – Replay DeepBook flash loan (legacy + MCP)
- `06_deepbook_orders.sh` – Replay DeepBook order transactions (legacy + MCP)
- `07_multi_swap_flash_loan.sh` – Replay multi-swap flash loan (legacy + MCP)
- `08_scallop_deposit.sh` – Replay Scallop deposit (legacy + MCP)
- `09_historical_replay_demo.sh` – Replay historical demo tx (legacy + MCP)
- `10_version_tracking_test.sh` – Replay version tracking tx (legacy + MCP)
- `11_analyze_protocols.sh` – Lightweight CLI analog (package load + version search)

## Notes

- Replay scripts run **both** the legacy CLI replay (GraphQL) and MCP replay
  (gRPC) to verify parity.
- `11_analyze_protocols.sh` is a lightweight analog to the Rust analysis example;
  it loads packages and searches for `version` symbols using MCP tools.

## Running

From repo root:

```bash
./examples/cli_mcp/01_ptb_basics.sh
```

Outputs are stored under:

```
/tmp/sui-sandbox-cli-mcp-examples/outputs
```
