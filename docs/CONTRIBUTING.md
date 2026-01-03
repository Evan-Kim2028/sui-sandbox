# AGENTS.md — sui-move-interface-extractor

## Project Overview

**Purpose**: Standalone Rust CLI for bytecode-first analysis of Sui Move packages.

**Core outputs**:
- Deterministic, canonical bytecode-derived interface JSON (`--emit-bytecode-json`)
- Deterministic corpus reports (`corpus_report.jsonl`, `problems.jsonl`, `corpus_summary.json`)
- Rigorous comparator vs Sui RPC normalized interfaces (mismatch counts + sampled mismatch paths)

**Design goals**:
- Prefer **bytecode ground truth** (Move binary format) over source/decompilation.
- Produce **diff-friendly** outputs (stable ordering and canonical formatting).
- Provide **verification loops** (RPC cross-check, corpus integrity checks, run attribution metadata).

## Repo Structure

```
.
├── Cargo.toml
├── docs/           # Documentation & Schemas
├── benchmark/      # Python Benchmark Harness
├── src/            # Rust CLI Source
├── scripts/        # Utility Scripts
└── results/        # Summary Outputs
```

## Key Guardrails

- Keep output deterministic: maintain stable sorting and JSON canonicalization.
- Any breaking schema change must bump `schema_version` and update `docs/SCHEMA.md`.
- Corpus runs should always remain attributable:
  - keep writing `run_metadata.json` (argv, rpc_url, timestamps, dataset git HEAD when available).
- Avoid hard-coding local workspace paths in docs or code; show examples as placeholders.

## Development Workflow

### Commands

```bash
cargo fmt
cargo clippy
cargo test
```

### Testing philosophy

- Prefer unit tests for:
  - type normalization
  - comparator behavior (match/mismatch)
  - address normalization/stability rules
- Avoid “network tests” in CI by default. If a networked integration test is added, gate it behind an env var.

## Style

- Rust: keep functions small, avoid panics in library-like code paths; return `anyhow::Result` with context.
- Prefer explicit structs for JSON schemas (and canonicalize output before writing).
- Keep docs current when adding new flags or outputs.

## Extending the Benchmark

The framework is designed to be modular. Follow these guides to add new capabilities.

### 1. Adding a New Agent
To add a new LLM or deterministic agent:
1. **Define the Logic**: Create a new class in `benchmark/src/smi_bench/agents/`.
   - For Phase I: Implement `predict_key_types(truth_key_types)`.
   - For Phase II: Implement `complete_json(prompt)`.
2. **Register in Runners**:
   - For Phase I: Update `runner.py`'s `run()` function to instantiate your agent based on the `--agent` flag.
   - For Phase II: Update `inhabit_runner.py`'s `run()` function.
3. **Add CLI Choice**: Add your agent name to the `choices` list in the `argparse` section of the relevant runner.

### 2. Adding a Normalization Rule
If you find that models consistently make a specific formatting error that should be ignored:
1. **Define the Correction**: Add a new member to the `CorrectionType` enum in `benchmark/src/smi_bench/inhabit/normalize.py`.
2. **Implement the Logic**: Add a new `elif` block in `_normalize_arg()` to detect the pattern and apply the fix.
3. **Log the Event**: Ensure you append a descriptive string to the `corrections` list so the researcher can see that a fix was applied.

### 3. Adding a Simulation Mode
To add a new way of verifying transactions (e.g., a local fork):
1. **Update Rust Simulator**: Add the mode logic to `src/bin/smi_tx_sim.rs`.
2. **Update Python Runner**: Update `run_tx_sim_via_helper()` in `benchmark/src/smi_bench/inhabit/engine.py` to support the new mode string.
3. **Register CLI Flag**: Add the mode to the `choices` of `--simulation-mode` in `inhabit_runner.py`.

## Documentation Testing Standards

All documentation must be executable, verifiable, and maintainable.

### Executable Examples

**Every code example must:**
- Be copy-paste executable from the repository root
- Use clearly marked placeholders: `<CORPUS_ROOT>`, `<PACKAGE_ID>`
- Work on supported platforms (macOS, Linux)
- Specify expected exit codes and outputs

**Validation:**
```bash
# Test A2A documentation examples
python benchmark/scripts/test_doc_examples.py docs/BENCHMARK_GUIDE.md docs/A2A_PROTOCOL.md
```

### Cross-Reference Validation

**Internal links:**
- All `[text](path.md)` links must resolve to existing files
- All `[text](#section)` anchors must exist
- Use relative paths over absolute

**Validation:**
```bash
# Validate Markdown links (offline)
python benchmark/scripts/validate_crossrefs.py --skip-external

# Validate including external links (slower)
python benchmark/scripts/validate_crossrefs.py
```

### Schema Synchronization

When `benchmark/docs/evaluation_bundle.schema.json` changes:
1. Update all documentation examples
2. Update `benchmark/docs/A2A_EXAMPLES.md` reference payloads
3. Update `benchmark/docs/ARCHITECTURE.md` invariants section
4. Add migration notes if breaking changes

**Reference:** See `benchmark/docs/TESTING.md` for complete testing procedures.

### Documentation Review Checklist

Before merging any doc changes:

- [ ] All code examples are tested and verified
- [ ] All links resolve (internal + external)
- [ ] Mermaid diagrams render correctly
- [ ] Placeholders are clearly marked
- [ ] Schema examples match current `.json` files
- [ ] Cross-references are bidirectional where appropriate
- [ ] Version-specific notes are clearly dated
- [ ] Commands use correct flag names and defaults

**Automated checks** (run in CI):
- `benchmark/scripts/test_doc_examples.py` - Validates command executability
- `benchmark/scripts/validate_crossrefs.py` - Validates Markdown links
- Schema validation - Ensures examples match current schema definition

**Related documentation:**
- **[Testing](TESTING.md)** - Complete testing guide.
- **[A2A Protocol](A2A_PROTOCOL.md)** - Request/response examples.
- **[Benchmark Guide](BENCHMARK_GUIDE.md)** - Canonical benchmark entrypoint.
