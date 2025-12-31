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
├── AGENTS.md
├── Cargo.toml
├── docs/
├── src/
│   └── main.rs
├── scripts/
└── results/
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
