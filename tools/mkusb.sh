#!/bin/bash
# mkusb.sh — Create a bootable UEFI USB for KarteOS x86_64
#
# Usage:
#   tools/mkusb.sh image             # Create a USB image file
#   tools/mkusb.sh /dev/sdX          # Write directly to USB drive (DANGEROUS)
#
# Layout (GPT):
#   Partition 1 (ESP, FAT32, 128MB): EFI/BOOT/BOOTX64.EFI
#   Partition 2 (ext4, rest): User programs (ls, cat, shell, etc.)

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
error() { echo -e "${RED}ERROR:${NC} $1" >&2; exit 1; }
info()  { echo -e "${GREEN}[mkusb]${NC} $1"; }
warn()  { echo -e "${YELLOW}[mkusb] WARNING:${NC} $1"; }

TARGET="$1"
[ -z "$TARGET" ] && error "Usage: $0 image  OR  $0 /dev/sdX"

IMAGE_MODE=false
if [ "$TARGET" = "image" ]; then
    IMAGE_MODE=true
    TARGET="$PROJECT_DIR/target/karte-os-usb.img"
    USB_SIZE_MB=${USB_SIZE_MB:-512}
fi

# ── Verify Build ──

EFI_LOADER="$PROJECT_DIR/target/x86_64-unknown-uefi/release/efi-loader.efi"

[ ! -f "$EFI_LOADER" ] && error "EFI loader not found. Run 'make uefi-x86' first."
[ ! -f "$PROJECT_DIR/user/shell.elf" ] && error "shell.elf not found. Run 'cd user && make ARCH=x86_64' first."

info "EFI loader: $EFI_LOADER ($(du -h "$EFI_LOADER" | cut -f1))"

# ── Build user programs ──
info "Building user programs..."
cd "$PROJECT_DIR/user" && make ARCH=x86_64 > /dev/null 2>&1 || warn "Some user programs failed to build"

# ── Create or Use Device ──

if $IMAGE_MODE; then
    info "Creating ${USB_SIZE_MB}MB UEFI USB image: $TARGET"
    dd if=/dev/zero of="$TARGET" bs=1M count="$USB_SIZE_MB" status=progress
    LOOP_DEV=$(losetup --find --show --partscan "$TARGET")
    info "Loop device: $LOOP_DEV"
    BLOCK_DEV="$LOOP_DEV"
else
    [ ! -b "$TARGET" ] && error "$TARGET is not a block device"
    BLOCK_DEV="$TARGET"
    info "Target device: $BLOCK_DEV"
    if [ "$(lsblk -no MOUNTPOINTS "$BLOCK_DEV" 2>/dev/null | grep -c '/')" -gt 0 ]; then
        error "Device has mounted partitions! Unmount first."
    fi
    echo -e "${YELLOW}WARNING: This will ERASE ALL DATA on $BLOCK_DEV${NC}"
    read -p "Type 'YES' to continue: " confirm
    [ "$confirm" = "YES" ] || error "Aborted."
fi

cleanup() {
    if $IMAGE_MODE && [ -n "$LOOP_DEV" ]; then
        losetup -d "$LOOP_DEV" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ── Partition (GPT for UEFI) ──

info "Partitioning ($BLOCK_DEV) with GPT..."
sgdisk --clear \
    --new=1:2048:+128M --typecode=1:EF00 --change-name=1:"EFI System" \
    --new=2:0:0        --typecode=2:8300 --change-name=2:"KarteOS Root" \
    "$BLOCK_DEV" 2>/dev/null || error "Partitioning failed (need sgdisk)"

sleep 1; partprobe "$BLOCK_DEV" 2>/dev/null || true; sleep 1

if $IMAGE_MODE; then
    PART1="${LOOP_DEV}p1"; PART2="${LOOP_DEV}p2"
else
    if [ -b "${BLOCK_DEV}1" ]; then
        PART1="${BLOCK_DEV}1"; PART2="${BLOCK_DEV}2"
    elif [ -b "${BLOCK_DEV}p1" ]; then
        PART1="${BLOCK_DEV}p1"; PART2="${BLOCK_DEV}p2"
    else error "Cannot find partition devices"
    fi
fi

info "ESP:  $PART1"
info "Root: $PART2"

# ── Format ──

info "Formatting ESP (FAT32)..."
mkfs.fat -F 32 -n "KARTEBOOT" "$PART1" 2>/dev/null || error "FAT32 format failed"

info "Formatting root (ext4)..."
mkfs.ext4 -F -L "KARTEOS" "$PART2" 2>/dev/null || error "ext4 format failed"

# ── Mount ──

ESP_MNT=$(mktemp -d); ROOT_MNT=$(mktemp -d)
mount "$PART1" "$ESP_MNT" || error "Failed to mount ESP"
mount "$PART2" "$ROOT_MNT" || { umount "$ESP_MNT"; error "Failed to mount root"; }

cleanup_mount() {
    umount "$ESP_MNT" 2>/dev/null || true
    umount "$ROOT_MNT" 2>/dev/null || true
    rmdir "$ESP_MNT" "$ROOT_MNT" 2>/dev/null || true
    cleanup
}
trap cleanup_mount EXIT

# ── Install EFI Loader ──

info "Installing EFI bootloader..."
mkdir -p "$ESP_MNT/EFI/BOOT"
cp "$EFI_LOADER" "$ESP_MNT/EFI/BOOT/BOOTX64.EFI"
info "  BOOTX64.EFI ($(du -h "$EFI_LOADER" | cut -f1))"

# ── Install User Programs ──

info "Installing user programs..."
mkdir -p "$ROOT_MNT/bin" "$ROOT_MNT/etc" "$ROOT_MNT/dev" "$ROOT_MNT/tmp" "$ROOT_MNT/home"

for elf in "$PROJECT_DIR/user/"*.elf; do
    [ -f "$elf" ] || continue
    name=$(basename "$elf" .elf)
    cp "$elf" "$ROOT_MNT/$name"
    cp "$elf" "$ROOT_MNT/bin/$name"
    info "  $name"
done

# ── Summary ──

echo ""
info "=========================================="
info "  UEFI USB created successfully!"
info "=========================================="
info "ESP:  $(du -sh "$ESP_MNT" | cut -f1) used"
info "Root: $(du -sh "$ROOT_MNT" | cut -f1) used"
echo ""

if $IMAGE_MODE; then
    info "Image: $TARGET ($(ls -lh "$TARGET" | awk '{print $5}'))"
    echo ""
    info "Write to USB:"
    info "  sudo dd if=$TARGET of=/dev/sdX bs=4M status=progress"
    info "  (replace /dev/sdX with your USB device — use lsblk to find it)"
else
    info "USB drive $BLOCK_DEV is ready!"
fi
echo ""
info "Boot: Insert USB → Power on → Select USB boot (F12/F2/Esc)"
info "      Ensure UEFI mode is enabled in BIOS (not Legacy/CSM)"
