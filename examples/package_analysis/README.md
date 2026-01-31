# Package Analysis (CLI)

This example fetches a package's bytecode, loads it into the sandbox, inspects modules,
then **attempts to call every entry function that requires no arguments**.

It produces a report of successes, failures, and skipped functions.

> ⚠️ Many entry functions require object inputs, type args, or specific on-chain state.
> This script only calls entry functions with **0 params** and **0 type params**.

## Usage

```
./examples/package_analysis/cli_package_analysis.sh <PACKAGE_ID>
```

Example package IDs (from `examples/analyze_protocols.rs`):

- `0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb`
- `0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf`

## What it does

1. `sui-sandbox fetch package <ID> --with-deps`
2. `sui-sandbox view modules <ID>` → list modules
3. For each module:
   - `sui-sandbox view module <ID>::<module>` → list functions
   - Attempt to run entry functions with `params=0` and `type_params=0`
4. Write a report to `/tmp/sui-sandbox-package-analysis/report.jsonl`

## Notes

- To exercise more functions, you need typed arguments and object inputs.
- For deeper testing, add a Rust analysis harness that synthesizes inputs.
