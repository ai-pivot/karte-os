#!/bin/bash
set -e

# ─── x86_64 Quick Boot Script ─────────────────────────────────
# Builds kernel, creates ISO, boots KarteOS with ext4 disk
# containing xbot-cli-static. Falls back to shell if no disk.

cd "$(dirname "$0")"

echo "=== KarteOS x86_64 Quick Boot ==="

# 1. Build user programs (needed for shell.elf embedded in kernel)
echo "[1/5] Building x86_64 user programs..."
cd user
make ARCH=x86_64 clean > /dev/null 2>&1
make ARCH=x86_64 -j$(nproc) > /dev/null 2>&1
# Stub .elf files for RISC-V assembly programs (cfg-gated on x86_64)
touch hello.elf heap_test.elf file_test.elf spawn_test.elf
cd ..

# 2. Build kernel
echo "[2/5] Building kernel..."
cargo +nightly build --release --target x86_64-unknown-none \
    -p karte-os-kernel -Z build-std=core,alloc 2>&1 | tail -1

# 3. Create ISO
echo "[3/5] Creating ISO..."
mkdir -p target/x86_64-iso/boot/grub
cp target/x86_64-unknown-none/release/karte-os-kernel \
   target/x86_64-iso/boot/karte-os-kernel
cat > target/x86_64-iso/boot/grub/grub.cfg << 'GRUBCFG'
set timeout=0
set default=0
menuentry "KarteOS" {
    multiboot2 /boot/karte-os-kernel
    boot
}
GRUBCFG
grub-mkrescue -o target/karte-os-x86_64.iso target/x86_64-iso 2>&1 | tail -1

# 4. Prepare disk (create + put xbot-cli-static if needed)
echo "[4/5] Checking disk..."
if [ ! -f disk.img ]; then
    echo "  Creating empty disk.img..."
    tools/mkdisk.sh init > /dev/null 2>&1
fi
# Check if xbot-cli-static is on disk
if ! tools/mkdisk.sh list 2>/dev/null | grep -q xbot-cli-static; then
    echo "  xbot-cli-static not on disk. Please put it first:"
    echo "    tools/mkdisk.sh put /path/to/xbot-cli-static"
    echo "  Continuing without it (will boot shell.elf)..."
    DISK_FLAGS=""
else
    echo "  xbot-cli-static found on disk."
    DISK_FLAGS="-drive file=disk.img,format=raw,if=none,id=hd0 -device ich9-ahci,id=ahci -device ide-hd,drive=hd0,bus=ahci.0"
fi

# 5. Boot in QEMU
echo "[5/5] Booting KarteOS..."
echo "  (Ctrl+A then X to quit)"
echo ""
qemu-system-x86_64 -machine pc -cpu qemu64 -m 128M \
    -cdrom target/karte-os-x86_64.iso \
    -serial stdio -display none -no-reboot \
    $DISK_FLAGS
