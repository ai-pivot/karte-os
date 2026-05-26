#!/bin/bash
# run-tests.sh — Run KarteOS kernel tests in QEMU and parse results
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
KERNEL_ELF="$PROJECT_DIR/target/riscv64gc-unknown-none-elf/release/karte-os-kernel"
OUTPUT_LOG="/tmp/qemu-test-output.log"
TIMEOUT_SECS=30

echo "══════════════════════════════════════════"
echo "  KarteOS Integration Test Runner"
echo "══════════════════════════════════════════"
echo ""

# Check kernel binary exists
if [ ! -f "$KERNEL_ELF" ]; then
    echo "❌ Kernel binary not found: $KERNEL_ELF"
    echo "   Run: cargo build --release -p karte-os-kernel --features test_mode"
    exit 1
fi

echo "Kernel: $KERNEL_ELF"
echo "Timeout: ${TIMEOUT_SECS}s"
echo ""

# Run QEMU with test kernel
echo "Starting QEMU..."
timeout $TIMEOUT_SECS qemu-system-riscv64 \
    -machine virt \
    -cpu rv64 \
    -nographic \
    -bios default \
    -m 128M \
    -smp 1 \
    -kernel "$KERNEL_ELF" \
    > "$OUTPUT_LOG" 2>&1 \
    || true

echo ""
echo "══════════════════════════════════════════"
echo "  Raw QEMU Output:"
echo "══════════════════════════════════════════"

# Show test output (filter out OpenSBI noise)
grep -a 'ok -\|not ok -\|──\|Test Results\|TEST_RESULT\|test\]' "$OUTPUT_LOG" || true

echo ""
echo "══════════════════════════════════════════"

# Parse results
OK_COUNT=$(grep -ac '  ok -' "$OUTPUT_LOG" 2>/dev/null || true)
NOTOK_COUNT=$(grep -ac '  not ok -' "$OUTPUT_LOG" 2>/dev/null || true)
OK_COUNT=${OK_COUNT:-0}
NOTOK_COUNT=${NOTOK_COUNT:-0}
TOTAL=$((OK_COUNT + NOTOK_COUNT))

echo ""
echo "  Tests Passed: ${OK_COUNT}/${TOTAL}"
echo "  Tests Failed: ${NOTOK_COUNT}/${TOTAL}"
echo ""

# Check final result
if grep -qa "TEST_RESULT: ALL_PASSED" "$OUTPUT_LOG"; then
    echo "✅ ALL TESTS PASSED"
    exit 0
elif grep -qa "TEST_RESULT:" "$OUTPUT_LOG"; then
    FAILED_RESULT=$(grep -a "TEST_RESULT:" "$OUTPUT_LOG" | head -1)
    echo "❌ $FAILED_RESULT"
    exit 1
else
    echo "❌ Test runner did not complete. Possible timeout or crash."
    echo ""
    echo "Full output:"
    cat "$OUTPUT_LOG"
    exit 1
fi
