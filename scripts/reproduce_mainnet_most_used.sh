#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Reproduce a corpus run against MystenLabs/sui-packages.

Defaults:
  - clones ../sui-packages if missing
  - uses packages/mainnet_most_used as corpus
  - writes large outputs to out/ (gitignored)
  - writes a shareable summary to results/*_submission_summary.json

Usage:
  ./scripts/reproduce_mainnet_most_used.sh [options]

Options:
  --sui-packages-dir <path>   Path to a sui-packages checkout (default: ../sui-packages)
  --sui-packages-rev <rev>    Optional git rev/commit/tag to checkout (for reproducibility)
  --corpus <name>             Corpus under sui-packages/packages/ (default: mainnet_most_used)
  --mode <local|full>         local = no RPC; full = RPC + interface compare (default: full)
  --out-dir <path>            Output dir for corpus artifacts (default: out/corpus_interface_all_1000)
  --summary <path>            Output path for submission summary (default: results/mainnet_most_used_submission_summary.json)
  --rpc-url <url>             RPC URL (default: https://fullnode.mainnet.sui.io:443)
  --concurrency <n>           RPC concurrency in full mode (default: 1)
  --help                      Show this help

Examples:
  ./scripts/reproduce_mainnet_most_used.sh --mode local
  ./scripts/reproduce_mainnet_most_used.sh --sui-packages-rev <commit>
EOF
}

sui_packages_dir="../sui-packages"
sui_packages_rev=""
corpus="mainnet_most_used"
mode="full"
out_dir="out/corpus_interface_all_1000"
summary_path="results/mainnet_most_used_submission_summary.json"
rpc_url="https://fullnode.mainnet.sui.io:443"
concurrency="1"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --sui-packages-dir)
      sui_packages_dir="$2"
      shift 2
      ;;
    --sui-packages-rev)
      sui_packages_rev="$2"
      shift 2
      ;;
    --corpus)
      corpus="$2"
      shift 2
      ;;
    --mode)
      mode="$2"
      shift 2
      ;;
    --out-dir)
      out_dir="$2"
      shift 2
      ;;
    --summary)
      summary_path="$2"
      shift 2
      ;;
    --rpc-url)
      rpc_url="$2"
      shift 2
      ;;
    --concurrency)
      concurrency="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! -d "$sui_packages_dir/.git" ]]; then
  echo "cloning MystenLabs/sui-packages into: $sui_packages_dir" >&2
  git clone https://github.com/MystenLabs/sui-packages.git "$sui_packages_dir"
fi

if [[ -n "$sui_packages_rev" ]]; then
  echo "checking out sui-packages rev: $sui_packages_rev" >&2
  git -C "$sui_packages_dir" fetch --all --tags
  git -C "$sui_packages_dir" checkout "$sui_packages_rev"
fi

corpus_root="$sui_packages_dir/packages/$corpus"
if [[ ! -d "$corpus_root" ]]; then
  echo "corpus not found: $corpus_root" >&2
  exit 2
fi

mkdir -p "$(dirname "$summary_path")"
mkdir -p "$out_dir"

echo "building (locked)..." >&2
cargo build --release --locked

common_args=(
  --bytecode-corpus-root "$corpus_root"
  --out-dir "$out_dir"
  --corpus-local-bytes-check
  --emit-submission-summary "$summary_path"
)

if [[ "$mode" == "local" ]]; then
  cargo run --release --locked -- "${common_args[@]}" --corpus-module-names-only --concurrency 16
  exit 0
fi

if [[ "$mode" != "full" ]]; then
  echo "invalid --mode (expected local|full): $mode" >&2
  exit 2
fi

cargo run --release --locked -- "${common_args[@]}" \
  --rpc-url "$rpc_url" \
  --corpus-rpc-compare --corpus-interface-compare \
  --concurrency "$concurrency" \
  --retries 12 --retry-initial-ms 500 --retry-max-ms 10000
