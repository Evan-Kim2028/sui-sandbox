# Benchmark Harness (`benchmark/`) - DEPRECATED

> **⚠️ DEPRECATED**: This Python benchmark infrastructure is no longer maintained. All active development has moved to the Rust side (`src/benchmark/`, `tests/`). This directory is scheduled for deletion.
>
> For current work, see:
>
> - `tests/execute_cetus_swap.rs` - Cetus DEX replay case study
> - `tests/execute_deepbook_swap.rs` - DeepBook replay case study
> - `docs/defi-case-study/` - DeFi case study documentation
> - `src/benchmark/` - Rust sandbox implementation

---

This directory contains the automated benchmarking harness for Sui Move packages.

## Start here

- **Run Phase II quickly:** `GETTING_STARTED.md`
- **Single model runner:** `scripts/run_model.sh`
- **Multi-model runner:** `scripts/run_multi_model.sh`

## Phase overview

- **Phase I (Key-Struct Discovery):** Predict which structs in a package have the `key` ability based on field shapes.
- **Phase II (Type Inhabitation):** Plan valid transaction sequences (Programmable Transaction Blocks) to create target Move objects.

## Key resources

- `../docs/METHODOLOGY.md` - Detailed scoring rules and extraction logic.
- `docs/FEEDBACK_PIPELINE_AUDIT.md` - Framework hardening notes.
- `docs/ARCHITECTURE.md` - Maintainers' map of the harness internals.

## Quick command reference

```bash
# Single-model Phase II targeted
cd benchmark
./scripts/run_model.sh --env-file ./.env --model openai/gpt-5.2

# Multi-model Phase II targeted (start conservative to avoid RPC rate limits)
./scripts/run_multi_model.sh --env-file ./.env --models "openai/gpt-5.2,google/gemini-3-flash-preview" --parallel 1
```
