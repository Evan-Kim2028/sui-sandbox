# Release Log

## 0.14.0 (2026-02-05)

### Highlights

- CLI-first tooling: new `tools` and `analyze` subcommands.
- Replay UX upgrades: PTB-style effects output, `--strict`, and source controls.
- PTB spec cleanup: CLI accepts only the `calls` schema (legacy MCP format removed).
- MCP surface removed: server crate, docs, and CLI tool mode.
- DataFetcher and legacy GraphQL-first prefetch removed.

### Migration Notes

- If you relied on MCP JSON `commands`, update to the CLI `calls` schema.
- If you used MCP server or CLI `tool`, switch to the CLI commands or scripting around `--json`.
- Legacy bincode state files are no longer supported; use `~/.sui-sandbox/state.json`.
- Replace DataFetcher usage with `sui_transport::{graphql, grpc}` or `sui_state_fetcher::HistoricalStateProvider`.

### Docs & Examples

- Docs trimmed to CLI-first guidance; legacy case studies and older examples were removed.
- Walrus and replay docs updated for current flows.
