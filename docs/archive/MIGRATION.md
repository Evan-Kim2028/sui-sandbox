# Migration Guide

This guide helps you migrate from deprecated features to their recommended replacements.

## Deprecated CLI Flags (v0.5.0)

The following CLI flags are deprecated and will be removed in v0.6.0:

### `--no-mm2` (Legacy Bytecode Analyzer)

**Deprecated:** v0.5.0
**Removal:** v0.6.0

**What it did:** Disabled the MM2 (Move Model 2) static type checking and fell back to legacy bytecode analysis.

**Migration:**

The MM2-based analysis is now the default and recommended approach. It provides:

- More accurate type resolution
- Better generic type handling
- Improved constructor discovery
- Static validation before execution

**Action:** Remove `--no-mm2` from your commands. The default MM2 analysis provides better results.

```bash
# Before (deprecated)
sui-move-interface-extractor benchmark --no-mm2 ...

# After (recommended)
sui-move-interface-extractor benchmark ...
```

If you encounter issues with MM2 analysis, please file a bug report rather than falling back to legacy analysis.

---

### `--no-phase-errors` (Legacy Error Stages)

**Deprecated:** v0.5.0
**Removal:** v0.6.0

**What it did:** Reverted to the legacy A1-A5/B1-B2 error stage taxonomy instead of the new E101-E502 error codes.

**Migration:**

The new error taxonomy provides more granular error categorization:

| Legacy Stage | New Code Range | Description |
|--------------|----------------|-------------|
| A1 | E101-E109 | Module/package loading errors |
| A2 | E201-E209 | Type resolution errors |
| A3 | E301-E309 | Constructor discovery errors |
| A4 | E401-E409 | Value synthesis errors |
| A5 | E501-E509 | PTB construction errors |
| B1 | E601-E609 | VM execution errors |
| B2 | E701-E709 | Post-execution validation errors |

**Action:** Update any error handling code to use the new E-codes:

```python
# Before (deprecated)
if error.stage == "A3":
    handle_constructor_error()

# After (recommended)
if 300 <= error.code < 400:
    handle_constructor_error()
```

---

### `--no-ptb` (Legacy VMHarness Execution)

**Deprecated:** v0.5.0
**Removal:** v0.6.0

**What it did:** Used the legacy VMHarness execution path instead of SimulationEnvironment-based PTB execution.

**Migration:**

The SimulationEnvironment-based execution provides:

- Proper shared object handling
- Correct lamport clock versioning
- Better gas metering
- More accurate transaction effects

**Action:** Remove `--no-ptb` from your commands:

```bash
# Before (deprecated)
sui-move-interface-extractor benchmark --no-ptb ...

# After (recommended)
sui-move-interface-extractor benchmark ...
```

---

## Configuration Changes

### Gas Budget Defaults

The default gas budget configuration is now centralized. If you were relying on hardcoded values, use the configuration system:

```python
from smi_bench.constants import DEFAULT_GAS_BUDGET, DEFAULT_GAS_BUDGET_LADDER

# Default: 10_000_000 MIST (0.01 SUI)
# Ladder: "20000000,50000000" for retries
```

### Retry Configuration

Retry settings are now centralized in `smi_bench.constants`:

```python
from smi_bench.constants import (
    DEFAULT_RETRY_MAX_ATTEMPTS,  # 3
    DEFAULT_RETRY_BASE_DELAY,    # 2.0 seconds
    DEFAULT_RETRY_MAX_DELAY,     # 30.0 seconds
)
```

---

## API Changes

### Framework Address Constants

Framework addresses are now available as a centralized constant:

```python
# Before (scattered definitions)
FRAMEWORK_PREFIXES = ("0x1", "0x2", "0x3", ...)

# After
from smi_bench.constants import FRAMEWORK_ADDRESSES

if address in FRAMEWORK_ADDRESSES:
    # Handle framework package
```

---

## Timeline

| Version | Status | Date |
|---------|--------|------|
| v0.5.0 | Deprecation warnings added | Current |
| v0.6.0 | Deprecated features removed | Planned |

---

## Getting Help

If you encounter issues during migration:

1. Check the [CHANGELOG.md](../CHANGELOG.md) for detailed release notes
2. Review the [ARCHITECTURE.md](../ARCHITECTURE.md) for system design
3. Open an issue at <https://github.com/anthropics/claude-code/issues>
