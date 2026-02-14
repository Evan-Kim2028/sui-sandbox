# Package Analysis (CLI)

This folder contains CLI-first examples for package and corpus analysis with `sui-sandbox analyze`.

## 1) Single Package Analysis

Primary direct CLI command:

```bash
sui-sandbox analyze package --package-id <PACKAGE_ID> --list-modules --mm2
```

For local bytecode directories:

```bash
sui-sandbox --json analyze package --bytecode-dir <PACKAGE_DIR> --mm2
```

Example package IDs:

- `0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb`
- `0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf`

Maintainer helper script (not part of curated examples):

- `scripts/internal/cli_package_analysis.sh <PACKAGE_ID>`

## 2) Corpus Object Classification (Sam-style)

Run `analyze objects` across a local corpus and save JSON:

Direct command:

```bash
sui-sandbox --json analyze objects --corpus-dir <CORPUS_DIR> --top 20 > /tmp/objects.json

# Built-in profile
sui-sandbox --json analyze objects --corpus-dir <CORPUS_DIR> --profile strict

# Custom profile file
sui-sandbox --json analyze objects --corpus-dir <CORPUS_DIR> --profile-file ./profiles/team.yaml
```

Notes:

- Use `--list-types` to include per-type rows in JSON output.
- Use `--profile strict|broad|hybrid` to switch built-in profile mode.
- Use `--profile-file ./profiles/team.yaml` to force a custom profile file.
- Named custom profiles are loaded from:
  - `.sui-sandbox/analyze/profiles/<name>.yaml` (repo-local)
  - `${XDG_CONFIG_HOME:-~/.config}/sui-sandbox/analyze/profiles/<name>.yaml` (user-global)
- Output now separates `party_transfer_eligible` (`key + store`) from `party_transfer_observed_in_bytecode` (bytecode evidence of `party_transfer` usage).
- JSON output includes a `profile` section showing effective settings and source path.

## 3) Corpus MM2 Stability Sweep

Run `analyze package --bytecode-dir ... --mm2` over many packages and summarize failures:

```bash
# Smoke check 100 packages
scripts/internal/cli_mm2_corpus_sweep.sh <CORPUS_DIR> 100

# Full sweep (1k corpus)
scripts/internal/cli_mm2_corpus_sweep.sh <CORPUS_DIR> 1000
```

Direct single-package command:

```bash
sui-sandbox --json analyze package --bytecode-dir <PACKAGE_DIR> --mm2
```

Sweep output:

- TSV report under `/tmp/sui-sandbox-mm2-sweep/` with per-package status and MM2 error text.
- Terminal summary with `ok`, `failed`, and `panic` counts.

## What the corpus workflows show

- How to use `analyze objects` for object ownership and usage pattern statistics.
- How to distinguish `party_transfer_eligible` types from party usage observed in Move code.
- How to regression-check MM2 robustness on real package corpora.
- How close your current local analysis is to previously reported baselines.

## Developer Workflow (Recommended)

Use `analyze` as a practical loop, not just as a one-off report:

1. **Start with package introspection**
   - Use this when you are editing or debugging one package.
   - `sui-sandbox analyze package --package-id <PACKAGE_ID> --list-modules --mm2`
   - Or local bytecode: `sui-sandbox --json analyze package --bytecode-dir <PACKAGE_DIR> --mm2`

2. **Get corpus-level baseline**
   - Use this to understand ecosystem patterns and compare with prior runs.
   - `sui-sandbox --json analyze objects --corpus-dir <CORPUS_DIR> --top 20`
   - Focus first on:
     - `object_types_discovered` (coverage sanity)
     - ownership counts
     - `party_transfer_eligible` vs `party_transfer_observed_in_bytecode`

3. **Interpret the party split**
   - `party_transfer_eligible`: objects with `key + store` (can be party-transferred publicly).
   - `party_transfer_observed_in_bytecode`: object types where package bytecode shows party-transfer usage.
   - Large gap (`eligible >> observed_in_bytecode`) means latent capability exists but is rarely exercised in package code.
   - Detection rules:
     - `party_transfer_eligible` is inferred from struct abilities (`key` + `store`) in bytecode type definitions.
     - `party_transfer_observed_in_bytecode` is inferred from transfer call-site patterns (functions containing `party_transfer` under module `transfer`).
   - Caveat: `observed_in_bytecode = 0` does not mean "cannot be party-transferred". PTBs can still party-transfer any `key + store` object even if package Move code does not call party transfer itself.

4. **Interpret dynamic-field counts carefully**
   - `dynamic_field_types` uses semantic bytecode evidence: nearby UID-borrow flow into `0x2::dynamic_field` / `0x2::dynamic_object_field` calls.
   - This is stricter than container-name matching and may undercount wrapper-style patterns (`table`, `bag`, etc.).

5. **Drill into concrete types**
   - Add `LIST_TYPES=1` to get per-type flags in JSON.
   - Inspect examples from:
     - `party_transfer_eligible_not_observed_examples`
     - `party_examples` (party-observed)
   - Then inspect modules/functions with `view module` or `analyze package`.

6. **Run MM2 stability as a regression guard**
   - `scripts/internal/cli_mm2_corpus_sweep.sh <CORPUS_DIR> 100`
   - Scale to full corpus in CI/nightly for reliability tracking.
