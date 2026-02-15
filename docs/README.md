# Documentation Map

Use this file as the docs entrypoint.

Canonical CLI naming used throughout docs:
- `context` (alias: `flow`)
- `adapter` (alias: `protocol`)
- `script` (alias: `run-flow`)
- `pipeline` (alias: `workflow`)

## Start Here

- New to the project:
  - [START_HERE.md](START_HERE.md)
- Want runnable examples first:
  - [../examples/README.md](../examples/README.md)
- Want root quickstart/overview:
  - [../README.md](../README.md)

## Replay and Runtime Workflows

- End-to-end replay workflow:
  - [guides/TRANSACTION_REPLAY.md](guides/TRANSACTION_REPLAY.md)
- Replay failure triage:
  - [guides/REPLAY_TRIAGE.md](guides/REPLAY_TRIAGE.md)
- Data source/fetching patterns:
  - [guides/DATA_FETCHING.md](guides/DATA_FETCHING.md)
- Walrus data specifics:
  - [walrus/README.md](walrus/README.md)
- Local publish/run flow:
  - [guides/GOLDEN_FLOW.md](guides/GOLDEN_FLOW.md)

## Python Bindings

- Python API reference:
  - [../crates/sui-python/README.md](../crates/sui-python/README.md)
- Local build/test/release workflow:
  - [guides/PYTHON_BINDINGS.md](guides/PYTHON_BINDINGS.md)

## Command and Behavior Reference

- CLI commands and flags:
  - [reference/CLI_REFERENCE.md](reference/CLI_REFERENCE.md)
- Environment variables:
  - [reference/ENV_VARS.md](reference/ENV_VARS.md)
- PTB JSON schema:
  - [reference/PTB_SCHEMA.md](reference/PTB_SCHEMA.md)
- Error codes:
  - [reference/ERROR_CODES.md](reference/ERROR_CODES.md)
- Known limitations and parity caveats:
  - [reference/LIMITATIONS.md](reference/LIMITATIONS.md)

## Architecture and Design

- Architecture overview and control flow:
  - [ARCHITECTURE.md](ARCHITECTURE.md)
- Prefetch internals:
  - [architecture/PREFETCHING.md](architecture/PREFETCHING.md)
- Typed workflow engine contract:
  - [architecture/WORKFLOW_ENGINE.md](architecture/WORKFLOW_ENGINE.md)
- Consolidation notes:
  - [architecture/CONSOLIDATION_CANDIDATES.md](architecture/CONSOLIDATION_CANDIDATES.md)
- Issue 26 implementation roadmap:
  - [architecture/ISSUE26_IMPLEMENTATION_ROADMAP.md](architecture/ISSUE26_IMPLEMENTATION_ROADMAP.md)

## Contributor and Maintainer Notes

- Contributing standards:
  - [CONTRIBUTING.md](CONTRIBUTING.md)
- Project-maintainer UX baseline:
  - [ux/PHASE0_BASELINE.md](ux/PHASE0_BASELINE.md)
- UX remediation plan:
  - [ux/PHASE1_ACTIONS.md](ux/PHASE1_ACTIONS.md)
