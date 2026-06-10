#!/bin/bash
# 一键准备 x86_64 测试环境
# 包含: 所有用户程序 + xbot-cli-static
# 用法: ./setup-x86-test.sh
# 测试: ./run-x86_64.sh

set -e
cd "$(dirname "$0")"

XBOT_BIN="/home/user/src/xbot/xbot-cli-static"
DISK="${DISK:-disk.img}"
DISK_SIZE="${DISK_SIZE:-512}"  # 512MB for xbot-cli-static (~69MB)
MOUNT="/tmp/karteos-mnt"

echo "════════════════════════════════════════════════"
echo "  KarteOS x86_64 Test Environment Setup"
echo "════════════════════════════════════════════════"

# 1. Build user programs
echo ""
echo "[1/5] Building x86_64 user programs..."
cd user && make ARCH=x86_64 clean > /dev/null 2>&1 && make ARCH=x86_64 > /dev/null 2>&1
cd ..
echo "      Done: $(ls user/*.elf 2>/dev/null | wc -l) programs"

# 2. Build kernel + ISO
echo ""
echo "[2/5] Building x86_64 kernel + ISO..."
rm -f target/x86_64-unknown-none/release/karte-os-kernel
rm -rf target/x86_64-unknown-none/release/.fingerprint/karte-os-kernel-*
cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc -p karte-os-kernel 2>&1 | tail -1

# Create ISO directory
rm -rf target/x86_64-iso
mkdir -p target/x86_64-iso/boot/grub
cp target/x86_64-unknown-none/release/karte-os-kernel target/x86_64-iso/boot/kernel.bin
cat > target/x86_64-iso/boot/grub/grub.cfg << 'EOF'
set timeout=0
set default=0
menuentry "KarteOS" {
    multiboot2 /boot/kernel.bin
    boot
}
EOF
grub-mkrescue -o target/karte-os-x86_64.iso target/x86_64-iso 2>/dev/null
echo "      Done: target/karte-os-x86_64.iso"

# 3. Create disk image (large enough for xbot)
echo ""
echo "[3/5] Creating ${DISK_SIZE}MB ext4 disk image..."
dd if=/dev/zero of="$DISK" bs=1M count="$DISK_SIZE" status=progress 2>/dev/null
mkfs.ext4 -O ^64bit -O ^has_journal -O ^metadata_csum -O ^flex_bg -O ^extra_isize \
    -b 4096 -L karteos "$DISK" >/dev/null 2>&1
echo "      Done: $DISK ($(du -h "$DISK" | cut -f1))"

# 4. Mount and install programs
echo ""
echo "[4/5] Installing programs..."
mkdir -p "$MOUNT"
sudo mount -o loop "$DISK" "$MOUNT"

# Install user programs (strip .elf extension)
count=0
for elf in user/*.elf; do
    [ -f "$elf" ] || continue
    base="$(basename "$elf" .elf)"
    # Skip empty stubs on x86_64
    case "$base" in
        hello|heap_test|file_test|spawn_test)
            size=$(stat -c%s "$elf" 2>/dev/null || echo "0")
            [ "$size" -lt 100 ] && continue
            ;;
    esac
    sudo cp "$elf" "$MOUNT/$base"
    count=$((count + 1))
done
echo "      User programs: $count"

# Install xbot-cli-static
if [ -f "$XBOT_BIN" ]; then
    sudo cp "$XBOT_BIN" "$MOUNT/xbot-cli-static"
    sudo chmod +x "$MOUNT/xbot-cli-static"
    echo "      xbot-cli-static: $(du -h "$XBOT_BIN" | cut -f1)"
else
    echo "      WARNING: $XBOT_BIN not found!"
fi

sudo sync
sudo umount "$MOUNT"
rmdir "$MOUNT" 2>/dev/null || true

# 5. Summary
echo ""
echo "[5/5] Verifying disk contents..."
sudo mount -o loop,ro "$DISK" "$MOUNT" 2>/dev/null || mkdir -p "$MOUNT" && sudo mount -o loop,ro "$DISK" "$MOUNT"
echo "      Files on disk:"
ls -lh "$MOUNT/" | tail -n +2 | awk '{printf "        %-20s %s\n", $NF, $5}'
sudo umount "$MOUNT" 2>/dev/null || true
rmdir "$MOUNT" 2>/dev/null || true

echo ""
echo "════════════════════════════════════════════════"
echo "  ✅ Ready! Run:  ./run-x86_64.sh"
echo "════════════════════════════════════════════════"
