# Dataset snapshots (`sui-packages`)

The `sui-packages` dataset is periodically updated.

To keep results reproducible, every corpus run of this tool writes `run_metadata.json` into the `--out-dir` containing:

- start/end timestamps
- argv
- `rpc_url`
- `bytecode_corpus_root`
- best-effort `git rev-parse HEAD` for the dataset checkout (when the corpus root is inside a git repo)

This file is the source of truth for attribution.

## Optional: human-readable log

If you want a quick “what did we run and when?” log, append entries here.

Template:

```text
Date:
Tool commit:
Dataset path:
Dataset HEAD:
Corpus root:
Command:
Out dir:
Notes:
```
