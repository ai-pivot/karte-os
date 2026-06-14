#!/bin/bash
set -e

# ─── RISC-V Quick Boot Script ─────────────────────────────────
# Builds kernel, boots KarteOS on RISC-V QEMU with ext4 disk
# containing xbot-cli-static. Falls back to shell if no disk.

cd "$(dirname "$0")/.."

echo "=== KarteOS RISC-V Quick Boot ==="

# 1. Build user programs (needed for shell.elf embedded in kernel)
echo "[1/4] Building RISC-V user programs..."
cd user
make ARCH=riscv64 clean > /dev/null 2>&1
make ARCH=riscv64 -j$(nproc) > /dev/null 2>&1
cd ..

# 2. Build kernel
echo "[2/4] Building kernel..."
rm -f target/riscv64gc-unknown-none-elf/release/karte-os-kernel
rm -rf target/riscv64gc-unknown-none-elf/release/.fingerprint/karte-os-kernel-*
cargo build --release -p karte-os-kernel --target riscv64gc-unknown-none-elf

# 3. Prepare disk (create + deploy RISC-V programs + xbot if needed)
echo "[3/4] Checking disk..."
if [ ! -f disk.img ]; then
    echo "  Creating disk.img and deploying RISC-V programs..."
    tools/mkdisk.sh deploy-riscv
else
    # Check if any programs are on disk; deploy if empty
    FILE_COUNT=$(tools/mkdisk.sh list 2>/dev/null | grep -c "^" || true)
    if [ "$FILE_COUNT" -lt 3 ]; then
        echo "  Deploying RISC-V programs..."
        tools/mkdisk.sh deploy-riscv
    fi
fi

# Check if xbot-cli-static is on disk
if ! tools/mkdisk.sh list 2>/dev/null | grep -q xbot-cli-static; then
    XBOT_BIN="${XBOT_BIN:-$HOME/src/xbot/xbot-cli-static-riscv64}"
    if [ -f "$XBOT_BIN" ]; then
        echo "  Deploying xbot-cli-static-riscv64 to disk..."
        tools/mkdisk.sh put "$XBOT_BIN" xbot-cli-static
    else
        echo "  xbot-cli-static-riscv64 not found at $XBOT_BIN"
    fi
fi

# Deploy hello-go if available
if [ -f tools/disk_root/hello-go ]; then
    if ! tools/mkdisk.sh list 2>/dev/null | grep -q hello-go; then
        echo "  Deploying hello-go to disk..."
        tools/mkdisk.sh put tools/disk_root/hello-go > /dev/null 2>&1
    fi
fi

# 4. Boot in QEMU
echo "[4/4] Booting KarteOS (RISC-V)..."
echo "  (Ctrl+A then X to quit)"
echo ""

QEMU_FLAGS="-machine virt -cpu rv64 -bios default \
    -m 128M -smp 1 \
    -serial stdio -display none -no-reboot \
    -drive id=blk0,file=disk.img,format=raw,if=none \
    -device virtio-blk-device,drive=blk0 \
    -netdev user,id=net0,hostfwd=tcp::2323-:23,hostfwd=udp::2323-:23 \
    -device virtio-net-device,netdev=net0"

qemu-system-riscv64 $QEMU_FLAGS \
    -kernel target/riscv64gc-unknown-none-elf/release/karte-os-kernel
