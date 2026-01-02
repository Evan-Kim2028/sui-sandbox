#!/bin/bash
set -e

# Setup directories
export SMI_TEMP_DIR="$(pwd)/benchmark/.tmp_smoke"
CORPUS_DIR="$(pwd)/benchmark/.tmp_smoke_corpus"
MOCK_BIN="$(pwd)/benchmark/.tmp_mock_extractor.sh"
RESULTS_DIR="$(pwd)/benchmark/results"
LOGS_DIR="$(pwd)/benchmark/logs"

mkdir -p "$SMI_TEMP_DIR"
mkdir -p "$RESULTS_DIR"
mkdir -p "$LOGS_DIR"

# 1. Create Mock Corpus
PKG_DIR="$CORPUS_DIR/0x00/mock_pkg"
mkdir -p "$PKG_DIR/bytecode_modules"
echo '{"id": "0x123"}' > "$PKG_DIR/metadata.json"
# Add a dummy file so directory isn't empty (though python only checks is_dir)
touch "$PKG_DIR/bytecode_modules/dummy.mv"

# 2. Create Mock Rust Extractor
cat > "$MOCK_BIN" <<EOF
#!/bin/bash
# Ignore args, just output valid empty interface JSON
echo '{"modules": {}, "structs": {}, "functions": {}}'
EOF
chmod +x "$MOCK_BIN"

echo "Starting smoke test..."
echo "Temp dir: $SMI_TEMP_DIR"
echo "Corpus: $CORPUS_DIR"
echo "Mock Bin: $MOCK_BIN"

# 3. Run Benchmark
# Note: we use 'mock-empty' agent which creates empty plans.
# We expect it to run, invoke our mock extractor, generate 0 hits, save results.
cd benchmark
uv run smi-inhabit \
    --corpus-root "$CORPUS_DIR" \
    --rust-bin "$MOCK_BIN" \
    --agent "mock-empty" \
    --samples 1 \
    --out "$RESULTS_DIR/smoke_test.json" \
    --log-dir "$LOGS_DIR" \
    --run-id "smoke_test_$(date +%s)" \
    --simulation-mode "dry-run" \
    --checkpoint-every 1

echo "Smoke test complete. Checking results..."

if [ -f "$RESULTS_DIR/smoke_test.json" ]; then
    echo "SUCCESS: Results file created."
    cat "$RESULTS_DIR/smoke_test.json" | head -n 20
    
    # Verify log output for A2A events (we can't easily grep stdout here as it was consumed, 
    # but the runner execution implies success).
else
    echo "FAILURE: Results file not found."
    exit 1
fi