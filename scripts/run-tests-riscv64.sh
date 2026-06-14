#!/bin/bash
# scripts/run-tests-riscv64.sh — Run KarteOS RISC-V integration tests in QEMU
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

KERNEL_DIR="$PROJECT_DIR/kernel"
OUTPUT_LOG="/tmp/karte-os-riscv64-test.log"
TIMEOUT_SECS=45
QEMU_CMD="qemu-system-riscv64"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --timeout) TIMEOUT_SECS="$2"; shift 2 ;;
        --output)  OUTPUT_LOG="$2"; shift 2 ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

echo "══════════════════════════════════════════"
echo "  KarteOS RISC-V Integration Tests"
echo "══════════════════════════════════════════"

# Build user programs for RISC-V
echo "Building RISC-V user programs..."
cd "$PROJECT_DIR/user" && make ARCH=riscv64 clean > /dev/null 2>&1
make ARCH=riscv64 > /dev/null 2>&1

# Build test kernel
echo "Building RISC-V test kernel..."
cd "$KERNEL_DIR"
rm -rf target/riscv64gc-unknown-none-elf/release/.fingerprint/karte-os-kernel-*
cargo build --release --target riscv64gc-unknown-none-elf \
    -p karte-os-kernel --features test_mode \
    > /dev/null 2>&1

echo "Running QEMU (timeout: ${TIMEOUT_SECS}s)..."
echo ""

rm -f "$OUTPUT_LOG"
timeout $TIMEOUT_SECS $QEMU_CMD \
    -machine virt -cpu rv64 -bios default -nographic \
    -m 128M -smp 1 \
    -kernel target/riscv64gc-unknown-none-elf/release/karte-os-kernel \
    -drive id=blk0,file=disk.img,format=raw,if=none \
    -device virtio-blk-device,drive=blk0 \
    -netdev user,id=net0 \
    -device virtio-net-device,netdev=net0 \
    > "$OUTPUT_LOG" 2>&1 || true

echo ""
echo "══════════════════════════════════════════"
echo "  Test Output:"
echo "══════════════════════════════════════════"

# Show test output (filter out OpenSBI noise)
grep -a 'ok -\|not ok -\|──\|Test Results\|TEST_RESULT\|\[test\]' "$OUTPUT_LOG" || true

echo ""
echo "══════════════════════════════════════════"

# Parse results
OK_COUNT=$(grep -ac '  ok -' "$OUTPUT_LOG" 2>/dev/null || true)
NOTOK_COUNT=$(grep -ac '  not ok -' "$OUTPUT_LOG" 2>/dev/null || true)
OK_COUNT=${OK_COUNT:-0}
NOTOK_COUNT=${NOTOK_COUNT:-0}
TOTAL=$((OK_COUNT + NOTOK_COUNT))

echo "  Passed: ${OK_COUNT}/${TOTAL}"
echo "  Failed: ${NOTOK_COUNT}/${TOTAL}"
echo ""

if grep -qa 'TEST_RESULT: ALL_PASSED' "$OUTPUT_LOG" 2>/dev/null; then
    echo "✅ ALL TESTS PASSED"
    # Restore normal kernel build
    cd "$KERNEL_DIR"
    cargo build --release --target riscv64gc-unknown-none-elf \
        -p karte-os-kernel > /dev/null 2>&1 || true
    exit 0
else
    echo "❌ SOME TESTS FAILED"
    echo ""
    echo "  Full output: $OUTPUT_LOG"
    echo ""
    echo "  Last 40 lines:"
    tail -40 "$OUTPUT_LOG"
    exit 1
fi
