#!/bin/bash
set -e

REPO_ROOT="$(git rev-parse --show-toplevel)"
FIXTURE_SRC="$REPO_ROOT/tests/fixture"
CORPUS_OUT="$REPO_ROOT/benchmark/.docker_test_corpus"

echo "Building fixture package at $FIXTURE_SRC..."
# Check if sui is installed
if ! command -v sui &> /dev/null; then
    echo "Error: 'sui' binary not found. Please install Sui CLI."
    exit 1
fi

# Build the Move package
# We capture output to avoid noise, unless it fails
if ! (cd "$FIXTURE_SRC" && sui move build > /dev/null); then
    echo "Error: 'sui move build' failed."
    exit 1
fi

echo "Creating corpus structure at $CORPUS_OUT..."
rm -rf "$CORPUS_OUT" 2>/dev/null || true
mkdir -p "$CORPUS_OUT/0x00/fixture"

# Copy bytecode modules
# sui build output is usually build/PackageName/bytecode_modules
BUILD_DIR="$FIXTURE_SRC/build/fixture"
if [ ! -d "$BUILD_DIR/bytecode_modules" ]; then
    echo "Error: Bytecode modules not found in $BUILD_DIR. Build structure might have changed."
    exit 1
fi

cp -r "$BUILD_DIR/bytecode_modules" "$CORPUS_OUT/0x00/fixture/"

# Create metadata.json
# Using 0x1 as per Move.toml
echo '{"id": "0x0000000000000000000000000000000000000000000000000000000000000001"}' > "$CORPUS_OUT/0x00/fixture/metadata.json"

echo "Corpus prepared successfully."
