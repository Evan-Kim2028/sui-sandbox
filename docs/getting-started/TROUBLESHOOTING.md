# Troubleshooting Benchmarks

This guide covers common issues encountered when running `smi-bench` benchmarks.

## 1. Missing Credentials

**Issue**: Benchmark starts but fails to make LLM calls or RPC requests.

**Diagnostic**:

```bash
uv run smi-bench-doctor
```

**Solution**:
Ensure `benchmark/.env` exists and contains:

- `OPENROUTER_API_KEY` (for most agents)
- `SMI_API_KEY` (if using `real-openai-compatible`)

## 2. Timeout Exceeded

**Issue**: Benchmark logs show `per-call timeout exceeded` or `sim_attempts: 0`.

**Explanation**:
Phase II has a hard wall-clock budget per package (`--per-package-timeout-seconds`). If the LLM response time plus the simulation time exceeds this, the package fails.

**Solution**:

1. Increase the timeout: `--per-package-timeout-seconds 600`.
2. Disable "Thinking" models if using OpenRouter to reduce latency.
3. Check network latency to the Sui RPC URL.

## 3. JSON Parse Errors

**Issue**: The agent returns a parse error or results are empty.

**Diagnostic**:
Check the events log for raw model output:

```bash
tail -f benchmark/logs/<run_id>/events.jsonl
```

**Solution**:
Often caused by the LLM returning malformed JSON or including markdown blocks (```json ...```) when not expected. The harness includes normalization logic to handle common formatting issues.

## 4. Bytecode Extraction Failures

**Issue**: Errors related to `sui_move_interface_extractor` not found or failing.

**Solution**:
The benchmark relies on the Rust binary. Ensure it is built in release mode:

```bash
cargo build --release --locked
```

The Python scripts expect the binary at `target/release/sui_move_interface_extractor`.

## 5. Corpus Not Found

**Issue**: Benchmark fails with "corpus not found" or similar error.

**Solution**:
Clone the sui-packages repository:

```bash
git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages
```

Then specify the corpus root:

```bash
--corpus-root ../sui-packages/packages/mainnet_most_used
```

## 6. gRPC Connection Failures

**Issue**: Transaction replay fails with gRPC connection errors.

**Solution**:

1. Check network connectivity to `archive.mainnet.sui.io:443`
2. Verify no firewall is blocking gRPC traffic
3. Try the GraphQL endpoint as fallback if available

## 7. Rate Limiting

**Issue**: OpenRouter or RPC endpoint returns rate limit errors.

**Solution**:

1. Reduce `--parallel` to 1 for multi-model runs
2. Lower `--run-samples` to reduce request volume
3. Add delays between runs with `--per-package-timeout-seconds`
