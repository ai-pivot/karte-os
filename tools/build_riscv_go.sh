#!/bin/bash
# Build disk image with Go binaries for RISC-V KarteOS testing
set -e

KARTE_DIR="/home/user/src/karte-os"
cd "$KARTE_DIR"

echo "=== Building RISC-V user programs ==="
cd user && make ARCH=riscv64 clean > /dev/null 2>&1 && make ARCH=riscv64

echo "=== Building kernel ==="
cargo build --release -p karte-os-kernel --target riscv64gc-unknown-none-elf

echo "=== Creating disk image ==="
# Create fresh disk and deploy everything
cd "$KARTE_DIR"
tools/mkdisk.sh deploy

# Also deploy pre-compiled Go binaries (not built by user/Makefile)
echo "=== Deploying Go binaries ==="
for gobin in hello-go xbot-cli-static; do
    if [ -f "tools/disk_root/$gobin" ]; then
        tools/mkdisk.sh put "tools/disk_root/$gobin"
    fi
done

echo "=== Verifying Go binaries on disk ==="
tools/mkdisk.sh list | grep -i "go\|xbot"

echo "=== Done! Run with: make shell ==="
echo "Disk image: $KARTE_DIR/disk.img"
echo ""
echo "To test:"
echo "  cd $KARTE_DIR && make shell"
echo "  At shell prompt: hello-go"
echo "  (or run the Go binary directly)"
