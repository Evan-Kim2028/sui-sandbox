# Contributing to sui-sandbox

## Project Overview

**Purpose**: Rust library and CLI for local Sui Move development, replay, and analysis.

**Core capabilities**:

- Local Move VM simulation via `SimulationEnvironment`
- Transaction replay from mainnet with historical state reconstruction
- PTB (Programmable Transaction Block) execution
- Package introspection and interface inspection

**Design goals**:

- Prefer **bytecode ground truth** (Move binary format) over source/decompilation
- Provide **reproducible, inspectable** outputs and logs
- Keep replay fidelity high (prefer complete object hydration)

## Repo Structure

```text
.
├── Cargo.toml           # Workspace root
├── src/                 # Main Rust library
│   └── lib.rs           # Library exports
├── src/bin/             # CLI entry points
│   └── sui_sandbox.rs   # Main CLI
├── crates/              # Workspace crates (core VM, transport, etc.)
├── docs/                # Documentation
├── tests/               # Core tests + fast suite
├── crates/sui-sandbox-integration-tests/ # Heavier + network-gated tests
├── examples/            # Example programs
└── .env.example         # Environment template
```

## Key Guardrails

- Keep output deterministic: maintain stable sorting and JSON canonicalization
- Avoid hard-coding local workspace paths in docs or code; show examples as placeholders

## Development Workflow

### Commands

```bash
cargo fmt
cargo clippy
cargo test
```

### Focused Test Runs

```bash
# Fast CLI smoke tests
cargo test -p sui-sandbox --test fast_suite

# Heavier integration tests (offline)
cargo test -p sui-sandbox-integration-tests

# Network tests (opt-in)
cargo test -p sui-sandbox-integration-tests --features network-tests -- --ignored --nocapture
```

### CI Harness

Suggested CI steps to validate CLI behavior:

```bash
# Enforce formatting
cargo fmt --all -- --check

# Lint all targets
cargo clippy --all-targets --all-features

# Core regression suite
cargo test -p sui-sandbox --test fast_suite

# Optional (slower) integration suite
cargo test -p sui-sandbox-integration-tests
```

Tip: set `SUI_SANDBOX_HOME` to a temp directory in CI so cache/logs/projects stay isolated.
      For multi-network CI, use separate `SUI_SANDBOX_HOME` values per network.

### Testing Philosophy

- Prefer unit tests for:
  - Type normalization
  - Replay data integrity
  - Address normalization/stability rules
- Avoid network tests in CI by default. Gate networked tests behind the `network-tests` feature and `#[ignore]`.

## Style

- Rust: keep functions small, avoid panics in library-like code paths; return `anyhow::Result` with context
- Prefer explicit structs for JSON schemas (and canonicalize output before writing)
- Keep docs current when adding new flags or outputs

## Environment Configuration

Copy `.env.example` to `.env` and configure:

```bash
cp .env.example .env
```

Key variables:

- `SUI_GRPC_ENDPOINT` - gRPC endpoint for mainnet data
- `SUI_GRPC_API_KEY` - API key for gRPC (if required by your provider)
- `SUI_SANDBOX_HOME` - Override default sandbox home (cache, projects, logs)
- `OPENROUTER_API_KEY` - For LLM-based features (optional)
- `SMI_SENDER` - Public address for dry-run simulation

See `.env.example` for full documentation.

## PyO3 Native Python Bindings

The `sui_sandbox` Python module provides native bindings to the Rust sandbox.

Full guide:

- [guides/PYTHON_BINDINGS.md](guides/PYTHON_BINDINGS.md)

### Quick Build Commands

```bash
cd /path/to/sui-sandbox
python -m pip install --upgrade pip maturin
maturin build --manifest-path crates/sui-python/Cargo.toml --release -o dist

# Install the built wheel
python -m pip install --force-reinstall dist/sui_sandbox-*.whl

# Or dev install into active environment
maturin develop --manifest-path crates/sui-python/Cargo.toml --release
```

### Version Management

Version must stay aligned in:

- `crates/sui-python/Cargo.toml`
- `crates/sui-python/pyproject.toml`

### Testing the Bindings

```bash
python -c "import sui_sandbox; print('Version:', sui_sandbox.__version__)"
```

## Documentation Standards

### Executable Examples

**Every code example must:**

- Be copy-paste executable from the repository root
- Use clearly marked placeholders: `<CORPUS_ROOT>`, `<PACKAGE_ID>`
- Work on supported platforms (macOS, Linux)

### Cross-Reference Validation

**Internal links:**

- All `[text](path.md)` links must resolve to existing files
- All `[text](#section)` anchors must exist
- Use relative paths over absolute

### Documentation Review Checklist

Before merging any doc changes:

- [ ] All code examples are tested and verified
- [ ] All links resolve (internal + external)
- [ ] Placeholders are clearly marked
- [ ] Commands use correct flag names and defaults

## Test Fixtures

Test fixtures live in `tests/fixture/`:

```
tests/fixture/
├── build/
│   └── fixture/
│       ├── bytecode_modules/    # Compiled .mv files
│       └── sources/             # Move source files
├── Move.toml                    # Package configuration
└── sources/                     # Additional sources
```

### Creating New Fixtures

1. **Write Move source** in `tests/fixture/sources/`:

```move
module fixture::my_module;

public fun my_function(x: u64): u64 {
    x + 1
}
```

2. **Compile** with `sui move build`:

```bash
cd tests/fixture && sui move build
```

3. **Verify** the fixture loads:

```bash
cargo test benchmark_local_can_load_fixture_modules
```

### Fixture Categories

| Category | Purpose |
|----------|---------|
| `build/fixture/` | Success cases - modules that should execute |
| `build/failure_cases/` | Failure cases - trigger specific error stages |

## Related Documentation

- **[Architecture](ARCHITECTURE.md)** - System architecture overview
- **[CLI Reference](reference/CLI_REFERENCE.md)** - Rust CLI commands
