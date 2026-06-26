#!/bin/bash
# flash_usb.sh — 刷写 KarteOS UEFI 启动盘（裸 FAT32，"superfloppy" 方式）
#
# 用法: bash tools/flash_usb.sh /dev/sdX
#
# 禁止在 USB 上创建分区表（GPT/MBR）。裸 FAT32 从扇区 0 开始，兼容性最好。
#
# 流程：
#   1. 编译 kernel + efi_loader
#   2. 用 mformat 创建裸 FAT32 镜像文件
#   3. 挂载镜像验证 MD5
#   4. dd 镜像到整个 USB 设备（/dev/sdX，不是 /dev/sdX1）
#   5. 从 USB 读取验证 MD5

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
err() { echo -e "${RED}ERROR:${NC} $1" >&2; exit 1; }
ok()  { echo -e "${GREEN}✓${NC} $1"; }
step() { echo -e "${YELLOW}=== $1 ===${NC}"; }

DEV="${1:-/dev/sda}"
PROJ="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$PROJ/target/boot.img"
EFI="$PROJ/target/x86_64-unknown-uefi/release/efi-loader.efi"
KERNEL_ELF="$PROJ/target/x86_64-unknown-none/release/karte-os-kernel"
KERNEL_BIN="$PROJ/target/x86_64-unknown-none/release/kernel.bin"

export PATH="/home/user/.cargo/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

[ -b "$DEV" ] || err "$DEV is not a block device. Usage: bash $0 /dev/sdX"
[ "$(id -u)" -eq 0 ] && err "Don't run as root. Script will sudo only dd."

echo "Target: $DEV ($(lsblk -dno MODEL,SIZE "$DEV" 2>/dev/null))"

# ── Step 1: Build ──
step "Step 1: Build kernel + EFI loader"
cd "$PROJ"
(cd user && make ARCH=x86_64 >/dev/null 2>&1) || true
cargo +nightly build --release --target x86_64-unknown-none \
    -p karte-os-kernel -Z build-std=core,alloc --features xhci_enum 2>&1 | tail -1
objcopy -O binary "$KERNEL_ELF" "$KERNEL_BIN"
KERNEL_BIN_PATH="$KERNEL_BIN" cargo +nightly build --release \
    --target x86_64-unknown-uefi -p efi-loader -Z build-std=core 2>&1 | tail -1
ok "Build complete ($(du -h "$EFI" | cut -f1) EFI binary)"

# ── Step 2: Create raw FAT32 image (no partitions!) ──
step "Step 2: Create raw FAT32 image (no partition table)"
rm -f "$IMG"
dd if=/dev/zero of="$IMG" bs=1M count=256 2>/dev/null
mformat -t 512 -h 2 -s 63 -F -H 2048 -i "$IMG" ::
mmd -i "$IMG" ::/EFI ::/EFI/BOOT
mcopy -i "$IMG" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
sync
ok "Image: $IMG ($(du -h "$IMG" | cut -f1))"

# ── Step 3: Verify image MD5 ──
step "Step 3: Verify image content"
EFI_MD5=$(md5sum "$EFI" | cut -d' ' -f1)
IMG_MD5=$(mtype -i "$IMG" ::/EFI/BOOT/BOOTX64.EFI 2>/dev/null | md5sum | cut -d' ' -f1)
[ "$IMG_MD5" = "$EFI_MD5" ] || err "Image MD5 mismatch: img=$IMG_MD5 efi=$EFI_MD5"
ok "Image MD5: $IMG_MD5"

# ── Step 4: Flash USB ──
step "Step 4: Flash USB (dd to entire device, NOT partition)"
sudo umount ${DEV}* 2>/dev/null || true
sudo dd if="$IMG" of="$DEV" bs=4M status=progress
sync
ok "USB written"

# ── Step 5: Verify USB by mounting ──
step "Step 5: Verify USB (mount + MD5)"
VERIFY=/tmp/usbverify_$$
mkdir -p "$VERIFY"
sudo mount "$DEV" "$VERIFY" 2>/dev/null || {
    # Try with a delay
    sleep 2
    sudo mount "$DEV" "$VERIFY" 2>/dev/null || err "Cannot mount $DEV"
}
USB_MD5=$(md5sum "$VERIFY/EFI/BOOT/BOOTX64.EFI" | cut -d' ' -f1)
sudo umount "$VERIFY"
rmdir "$VERIFY" 2>/dev/null || true
[ "$USB_MD5" = "$EFI_MD5" ] || err "USB MD5 mismatch: usb=$USB_MD5 efi=$EFI_MD5"
ok "USB MD5 verified: $USB_MD5"

# ── Done ──
echo ""
ok "=========================================="
ok "  USB $DEV ready! All checks passed."
ok "=========================================="
echo "Reboot → Select USB boot (UEFI mode)"
