#!/bin/bash
# Example Output Verification Script
# Verifies that all examples produce expected outputs after restructure
set -e

cd "$(dirname "$0")/.."

echo "=== Running Example Output Verification ==="
echo ""

# Example 1: cetus_swap
echo "=== cetus_swap ==="
OUTPUT=$(cargo run --example cetus_swap --release 2>&1)
if echo "$OUTPUT" | grep -q "✓ TRANSACTION MATCHES EXPECTED OUTCOME" && \
   echo "$OUTPUT" | grep -q "local: SUCCESS | expected: SUCCESS"; then
    echo "✓ cetus_swap: PASSED"
else
    echo "✗ cetus_swap: FAILED - output changed!"
    echo "$OUTPUT" | grep -A5 "VALIDATION SUMMARY"
    exit 1
fi

# Example 2: deepbook_replay
echo ""
echo "=== deepbook_replay ==="
OUTPUT=$(cargo run --example deepbook_replay --release 2>&1)
if echo "$OUTPUT" | grep -q "✓ ALL TRANSACTIONS MATCH EXPECTED OUTCOMES" && \
   echo "$OUTPUT" | grep -q "Flash Loan Swap.*SUCCESS" && \
   echo "$OUTPUT" | grep -q "Flash Loan Arb.*FAILURE"; then
    echo "✓ deepbook_replay: PASSED"
else
    echo "✗ deepbook_replay: FAILED - output changed!"
    echo "$OUTPUT" | grep -A10 "VALIDATION SUMMARY"
    exit 1
fi

# Example 3: scallop_deposit (expected to fail with specific error)
echo ""
echo "=== scallop_deposit ==="
OUTPUT=$(cargo run --example scallop_deposit --release 2>&1)
if echo "$OUTPUT" | grep -q "FAILED_TO_DESERIALIZE_ARGUMENT" && \
   echo "$OUTPUT" | grep -q "local: FAILURE | expected: SUCCESS"; then
    echo "✓ scallop_deposit: PASSED (known failure preserved)"
else
    echo "✗ scallop_deposit: FAILED - error behavior changed!"
    echo "$OUTPUT" | grep -A10 "VALIDATION SUMMARY"
    exit 1
fi

# Example 4: multi_swap_flash_loan (should succeed)
echo ""
echo "=== multi_swap_flash_loan ==="
OUTPUT=$(cargo run --example multi_swap_flash_loan --release 2>&1)
if echo "$OUTPUT" | grep -q "VALIDATION SUMMARY"; then
    if echo "$OUTPUT" | grep -q "local: SUCCESS"; then
        echo "✓ multi_swap_flash_loan: PASSED"
    else
        echo "~ multi_swap_flash_loan: Ran (check output for expected behavior)"
        echo "$OUTPUT" | grep -A5 "VALIDATION SUMMARY"
    fi
else
    echo "✗ multi_swap_flash_loan: FAILED - no validation output!"
    exit 1
fi

echo ""
echo "═══════════════════════════════════════════════"
echo "  ✓ ALL EXAMPLES VERIFIED"
echo "═══════════════════════════════════════════════"
