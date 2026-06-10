#!/bin/bash
# scripts/run-tests-x86_64.sh — Run KarteOS x86_64 integration tests in QEMU
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

KERNEL_DIR="$PROJECT_DIR/kernel"
OUTPUT_LOG="/tmp/karte-os-x86_64-test.log"
TIMEOUT_SECS=45

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --timeout) TIMEOUT_SECS="$2"; shift 2 ;;
        --output)  OUTPUT_LOG="$2"; shift 2 ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

echo "══════════════════════════════════════════"
echo "  KarteOS x86_64 Integration Tests"
echo "══════════════════════════════════════════"

# Build user programs for x86_64
echo "Building x86_64 user programs..."
cd "$PROJECT_DIR/user" && make ARCH=x86_64 clean > /dev/null 2>&1
make ARCH=x86_64 > /dev/null 2>&1

# Build test kernel
echo "Building x86_64 test kernel..."
cd "$KERNEL_DIR"
cargo +nightly build --release --target x86_64-unknown-none \
    -Z build-std=core,alloc -p karte-os-kernel --features test_mode \
    > /dev/null 2>&1

# Create ISO
echo "Creating ISO..."
cd "$PROJECT_DIR"
mkdir -p target/x86_64-iso/boot/grub
cp target/x86_64-unknown-none/release/karte-os-kernel target/x86_64-iso/boot/karte-os-kernel
cat > target/x86_64-iso/boot/grub/grub.cfg << 'EOF'
set timeout=0
set default=0
menuentry "KarteOS Test" {
    multiboot2 /boot/karte-os-kernel
    boot
}
EOF
grub-mkrescue -o target/karte-os-x86_64-test.iso target/x86_64-iso > /dev/null 2>&1

echo "Running QEMU (timeout: ${TIMEOUT_SECS}s)..."
echo ""

rm -f "$OUTPUT_LOG"
timeout $TIMEOUT_SECS qemu-system-x86_64 \
    -machine pc -cpu qemu64 -m 512M \
    -cdrom target/karte-os-x86_64-test.iso \
    -serial stdio \
    -display none -no-reboot \
    -drive file=disk.img,format=raw,if=none,id=hd0 \
    -device ich9-ahci,id=ahci -device ide-hd,drive=hd0,bus=ahci.0 \
    -netdev user,id=net0 \
    -device virtio-net-pci,netdev=net0 \
    > "$OUTPUT_LOG" 2>&1 || true

echo ""
echo "══════════════════════════════════════════"
echo "  Test Output:"
echo "══════════════════════════════════════════"

# Show test output
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
    cargo +nightly build --release --target x86_64-unknown-none \
        -Z build-std=core,alloc -p karte-os-kernel > /dev/null 2>&1 || true
    exit 0
else
    echo "❌ SOME TESTS FAILED"
    echo ""
    echo "  Full output: $OUTPUT_LOG"
    exit 1
fi
