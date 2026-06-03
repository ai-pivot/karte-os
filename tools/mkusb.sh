#!/bin/bash
# mkusb.sh — Create a bootable USB image for KarteOS x86_64
#
# Usage:
#   tools/mkusb.sh /dev/sdX          # Write directly to USB drive
#   tools/mkusb.sh image             # Create a USB disk image file
#
# Requirements:
#   - sfdisk, mkfs.fat, mkfs.ext4, grub-install, mtools
#   - KarteOS kernel already built (make build-x86)
#
# The USB layout:
#   Partition 1 (64MB, FAT32, bootable): GRUB bootloader + kernel
#   Partition 2 (rest, ext4): Root filesystem with OS files

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

error() { echo -e "${RED}ERROR:${NC} $1" >&2; exit 1; }
info()  { echo -e "${GREEN}[mkusb]${NC} $1"; }
warn()  { echo -e "${YELLOW}[mkusb] WARNING:${NC} $1"; }

# ── Parse Arguments ──

TARGET="$1"
[ -z "$TARGET" ] && error "Usage: $0 /dev/sdX  OR  $0 image"

IMAGE_MODE=false
if [ "$TARGET" = "image" ]; then
    IMAGE_MODE=true
    TARGET="$PROJECT_DIR/target/karte-os-usb.img"
    USB_SIZE_MB=${USB_SIZE_MB:-512}
fi

# ── Verify Build ──

KERNEL="$PROJECT_DIR/target/x86_64-unknown-none/release/karte-os-kernel"
SHELL_ELF="$PROJECT_DIR/user/shell.elf"

[ ! -f "$KERNEL" ] && error "Kernel not found. Run 'make build-x86' first."
[ ! -f "$SHELL_ELF" ] && error "shell.elf not found. Run 'cd user && make ARCH=x86_64' first."

info "Kernel: $KERNEL"
info "Shell:  $SHELL_ELF"

# ── Build user programs for x86_64 ──

info "Building user programs..."
cd "$PROJECT_DIR/user" && make ARCH=x86_64 > /dev/null 2>&1 || true

# ── Create Image or Use Device ──

if $IMAGE_MODE; then
    info "Creating ${USB_SIZE_MB}MB USB image: $TARGET"
    dd if=/dev/zero of="$TARGET" bs=1M count="$USB_SIZE_MB" status=progress
    LOOP_DEV=$(losetup --find --show --partscan "$TARGET")
    info "Loop device: $LOOP_DEV"
    BLOCK_DEV="$LOOP_DEV"
else
    # Real device
    if [ ! -b "$TARGET" ]; then
        error "$TARGET is not a block device"
    fi
    BLOCK_DEV="$TARGET"
    info "Target device: $BLOCK_DEV"
    
    # Safety check
    if [ "$(lsblk -no MOUNTPOINTS "$BLOCK_DEV" 2>/dev/null | grep -c '/')" -gt 0 ]; then
        error "Device $BLOCK_DEV has mounted partitions! Unmount first."
    fi
    
    # Confirm
    echo -e "${YELLOW}WARNING: This will ERASE ALL DATA on $BLOCK_DEV${NC}"
    read -p "Type 'YES' to continue: " confirm
    [ "$confirm" = "YES" ] || error "Aborted."
fi

cleanup() {
    if $IMAGE_MODE && [ -n "$LOOP_DEV" ]; then
        info "Detaching loop device..."
        losetup -d "$LOOP_DEV" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ── Partition ──

info "Partitioning $BLOCK_DEV..."

# Create partition table: GPT for modern UEFI + BIOS compatibility
# Actually use MBR (DOS) for maximum GRUB compatibility on real hardware
sfdisk "$BLOCK_DEV" <<EOF || error "Partitioning failed"
label: dos
unit: sectors

${BLOCK_DEV}p1 : start=2048, size=131072, type=0c, bootable
${BLOCK_DEV}p2 : start=133120, type=83
EOF

# Wait for kernel to recognize partitions
sleep 1
partprobe "$BLOCK_DEV" 2>/dev/null || true
sleep 1

# Determine partition devices
if $IMAGE_MODE; then
    PART1="${LOOP_DEV}p1"
    PART2="${LOOP_DEV}p2"
else
    # Try with and without 'p' prefix
    if [ -b "${BLOCK_DEV}1" ]; then
        PART1="${BLOCK_DEV}1"
        PART2="${BLOCK_DEV}2"
    elif [ -b "${BLOCK_DEV}p1" ]; then
        PART1="${BLOCK_DEV}p1"
        PART2="${BLOCK_DEV}p2"
    else
        error "Cannot find partition devices"
    fi
fi

info "Partition 1 (boot): $PART1"
info "Partition 2 (root): $PART2"

# ── Format ──

info "Formatting partitions..."
mkfs.fat -F 32 -n "KARTEOS_BOOT" "$PART1" || error "Failed to format FAT32"
mkfs.ext4 -F -L "KARTEOS_ROOT" "$PART2" || error "Failed to format ext4"

# ── Mount ──

BOOT_MNT=$(mktemp -d)
ROOT_MNT=$(mktemp -d)

mount "$PART1" "$BOOT_MNT" || error "Failed to mount boot partition"
mount "$PART2" "$ROOT_MNT" || { umount "$BOOT_MNT"; error "Failed to mount root partition"; }

cleanup_mount() {
    umount "$BOOT_MNT" 2>/dev/null || true
    umount "$ROOT_MNT" 2>/dev/null || true
    rmdir "$BOOT_MNT" "$ROOT_MNT" 2>/dev/null || true
    cleanup
}
trap cleanup_mount EXIT

# ── Install GRUB ──

info "Installing GRUB bootloader..."
mkdir -p "$BOOT_MNT/boot/grub"
cat > "$BOOT_MNT/boot/grub/grub.cfg" << 'GRUBCFG'
set timeout=3
set default=0
set gfxpayload=text

menuentry "KarteOS" {
    multiboot2 /boot/karte-os-kernel
    boot
}

menuentry "KarteOS (verbose)" {
    multiboot2 /boot/karte-os-kernel
    boot
}

menuentry "Reboot" {
    reboot
}

menuentry "Shutdown" {
    halt
}
GRUBCFG

# Install kernel
cp "$KERNEL" "$BOOT_MNT/boot/karte-os-kernel"
info "Kernel installed ($(du -h "$KERNEL" | cut -f1))"

# Install GRUB for BIOS (MBR) boot
grub-install --target=i386-pc --boot-directory="$BOOT_MNT/boot" "$BLOCK_DEV" 2>/dev/null || {
    warn "grub-install failed. You may need to install GRUB manually."
    warn "Try: sudo grub-install --target=i386-pc --boot-directory=$BOOT_MNT/boot $BLOCK_DEV"
}

# ── Install Root Filesystem ──

info "Installing root filesystem..."

# Copy user programs to ext4 root (without .elf extension)
for elf in "$PROJECT_DIR/user/"*.elf; do
    [ -f "$elf" ] || continue
    name=$(basename "$elf" .elf)
    cp "$elf" "$ROOT_MNT/$name"
    info "  Installed: $name"
done

# Create essential directories
mkdir -p "$ROOT_MNT/bin"
mkdir -p "$ROOT_MNT/etc"
mkdir -p "$ROOT_MNT/dev"
mkdir -p "$ROOT_MNT/tmp"
mkdir -p "$ROOT_MNT/home"

# Copy programs to /bin as well
for elf in "$PROJECT_DIR/user/"*.elf; do
    [ -f "$elf" ] || continue
    name=$(basename "$elf" .elf)
    cp "$elf" "$ROOT_MNT/bin/$name"
done

# Create /etc/init.sh (basic startup script)
cat > "$ROOT_MNT/etc/init.sh" << 'INIT'
#!/bin/sh
# KarteOS initialization script
echo "Welcome to KarteOS!"
echo ""
INIT

# Create /etc/hostname
echo "karteos" > "$ROOT_MNT/etc/hostname"

# Create /etc/motd
cat > "$ROOT_MNT/etc/motd" << 'MOTD'
  _        _   ___  ____
 | |      / \ / _ \/ ___|
 | |     / _ \ | | \___ \
 | |___ / ___ \ |_| |___) |
 |_____/_/   \_\___/|____/

  KarteOS v0.2.0 — A modern dual-architecture OS

  Type 'help' for available commands.
MOTD

info "Root filesystem populated"

# ── Show Summary ──

echo ""
info "══════════════════════════════════════════"
info "  USB image created successfully!"
info "══════════════════════════════════════════"
echo ""
info "Boot partition: $(du -sh "$BOOT_MNT" | cut -f1) used"
info "Root partition: $(du -sh "$ROOT_MNT" | cut -f1) used"
echo ""

if $IMAGE_MODE; then
    info "Image file: $TARGET"
    info "To write to USB: dd if=$TARGET of=/dev/sdX bs=4M status=progress"
    info "  (replace /dev/sdX with your USB device)"
else
    info "USB drive $BLOCK_DEV is ready to boot!"
fi

echo ""
info "Boot your PC from this USB drive (may need to change"
info "boot order in BIOS/UEFI settings). GRUB will show a"
info "menu to boot KarteOS."
echo ""

# ── Cleanup ──
umount "$BOOT_MNT"
umount "$ROOT_MNT"
rmdir "$BOOT_MNT" "$ROOT_MNT"

info "Done!"
