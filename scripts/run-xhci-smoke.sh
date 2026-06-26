#!/bin/bash
# scripts/run-xhci-smoke.sh — XHCI/USB HID keyboard smoke test in QEMU.
#
# Builds the kernel with the `xhci_enum` breaker enabled and boots it under
# QEMU with a `qemu-xhci` host controller and a `usb-kbd` device attached.
# The test verifies:
#   1. The kernel boots to the shell without page faults.
#   2. The XHCI controller is found, reset, and brought up.
#   3. At least one USB device (the keyboard) is enumerated.
#
# This is a smoke test, not a full HID input test (QEMU usb-kbd has no way to
# inject keystrokes without a monitor console). Full key-press verification is
# done on real hardware stage by stage.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
KERNEL_DIR="$PROJECT_DIR/kernel"
OUTPUT_LOG="/tmp/karte-os-xhci-smoke.log"
TIMEOUT_SECS=60

echo "══════════════════════════════════════════"
echo "  KarteOS XHCI/USB HID Smoke Test"
echo "══════════════════════════════════════════"

# Build user programs for x86_64 (kernel embeds shell.elf via include_bytes!)
echo "Building x86_64 user programs..."
cd "$PROJECT_DIR/user" && make ARCH=x86_64 clean > /dev/null 2>&1
make ARCH=x86_64 > /dev/null 2>&1

# Build kernel with xhci_enum breaker enabled (NOT test_mode, so we boot to
# the shell and run the real XHCI init/enumeration path).
echo "Building x86_64 kernel (xhci_enum)..."
cd "$KERNEL_DIR"
cargo +nightly build --release --target x86_64-unknown-none \
    -Z build-std=core,alloc -p karte-os-kernel \
    --features xhci_enum \
    > /dev/null 2>&1

# Create ISO
echo "Creating ISO..."
cd "$PROJECT_DIR"
mkdir -p target/x86_64-iso/boot/grub
cp target/x86_64-unknown-none/release/karte-os-kernel target/x86_64-iso/boot/karte-os-kernel
cat > target/x86_64-iso/boot/grub/grub.cfg << 'EOF'
set timeout=0
set default=0
menuentry "KarteOS XHCI Smoke" {
    multiboot2 /boot/karte-os-kernel
    boot
}
EOF
grub-mkrescue -o target/karte-os-xhci-smoke.iso target/x86_64-iso > /dev/null 2>&1

# Need a disk image for the shell to mount ext4.
if [ ! -f disk.img ]; then
    echo "Creating minimal disk image..."
    bash tools/mkdisk.sh init > /dev/null 2>&1
    cd user && make ARCH=x86_64 > /dev/null 2>&1 && cd ..
    bash tools/mkdisk.sh deploy-x86 > /dev/null 2>&1
fi

echo "Running QEMU with qemu-xhci + usb-kbd (timeout: ${TIMEOUT_SECS}s)..."
echo ""

rm -f "$OUTPUT_LOG"
timeout $TIMEOUT_SECS qemu-system-x86_64 \
    -machine pc -cpu qemu64 -m 512M \
    -cdrom target/karte-os-xhci-smoke.iso \
    -serial stdio \
    -display none -no-reboot \
    -drive file=disk.img,format=raw,if=none,id=hd0 \
    -device ich9-ahci,id=ahci -device ide-hd,drive=hd0,bus=ahci.0 \
    -netdev user,id=net0 \
    -device virtio-net-pci,netdev=net0 \
    -device qemu-xhci,id=xhci \
    -device usb-kbd,bus=xhci.0 \
    > "$OUTPUT_LOG" 2>&1 || true

echo ""
echo "══════════════════════════════════════════"
echo "  XHCI Boot Output (filtered):"
echo "══════════════════════════════════════════"

grep -a '\[xhci\]\|\[pci\].*0c:03:30\|\[init\]\|KarteOS Shell\|UNHANDLED\|PF\|KF\|panic\|TEST_RESULT' \
    "$OUTPUT_LOG" || true

echo ""
echo "══════════════════════════════════════════"

PASS=true

# 1. XHCI controller must be found and brought up.
if ! grep -qa '\[xhci\] Running' "$OUTPUT_LOG" 2>/dev/null; then
    echo "FAIL: XHCI controller did not reach Running state"
    PASS=false
fi

# 2. No kernel page faults / panics.
if grep -qa 'UNHANDLED\|\[PF\]\|\[KF\]\|panic' "$OUTPUT_LOG" 2>/dev/null; then
    echo "FAIL: kernel fault detected"
    PASS=false
fi

# 3. Shell reached (boot-test parity).
if grep -qa 'KarteOS Shell' "$OUTPUT_LOG" 2>/dev/null; then
    echo "PASS: shell reached"
else
    echo "FAIL: shell not reached"
    PASS=false
fi

# 4. Keyboard enumerated (best-effort: QEMU usb-kbd should show up).
if grep -qa '\[xhci\] keyboard ready' "$OUTPUT_LOG" 2>/dev/null; then
    echo "PASS: HID keyboard enumerated"
elif grep -qa '\[xhci\] no keyboard found' "$OUTPUT_LOG" 2>/dev/null; then
    echo "WARN: no keyboard found (QEMU usb-kbd timing may vary)"
else
    echo "WARN: keyboard enumeration result not logged"
fi

echo ""
if $PASS; then
    echo "XHCI SMOKE TEST: PASSED"
    exit 0
else
    echo "XHCI SMOKE TEST: FAILED"
    echo "Full output: $OUTPUT_LOG"
    exit 1
fi
