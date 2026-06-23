#!/bin/bash
# tools/fix_efi_grub_cfg.sh — Post-process ISO to inject grub.cfg into efi.img
# The issue: grub-mkrescue puts grub.cfg in ISO9660 but NOT in the FAT efi.img.
# On UEFI boot, GRUB reads config from efi.img, not ISO9660.
# This script extracts efi.img, adds grub.cfg, and rebuilds the ISO.

set -e
ISO_DIR="${1:-target/x86_64-iso}"
ISO_OUT="${2:-target/karte-os-x86_64.iso}"

if [ ! -f "$ISO_OUT" ]; then
    echo "Building ISO first..."
    grub-mkrescue -V KARTEOS -o "$ISO_OUT" "$ISO_DIR" 2>/dev/null
fi

TMPDIR=$(mktemp -d)
EFIIMG="$TMPDIR/efi.img"

# Extract efi.img from the ISO (it's stored inside the ISO9660 as a regular file)
echo "Extracting efi.img from ISO..."
mkdir -p /tmp/iso_mnt
sudo mount -o loop "$ISO_OUT" /tmp/iso_mnt
cp /tmp/iso_mnt/efi.img "$EFIIMG"
sudo umount /tmp/iso_mnt

# Add grub.cfg to the FAT image
echo "Injecting grub.cfg into efi.img..."
sudo mount -o loop,rw "$EFIIMG" /tmp/iso_mnt
sudo mkdir -p /tmp/iso_mnt/boot/grub
cat > /tmp/grub_inject.cfg << 'GRUBCFG'
set timeout=0
set default=0
menuentry "KarteOS" {
    multiboot2 /boot/karte-os-kernel
    boot
}
GRUBCFG
sudo cp /tmp/grub_inject.cfg /tmp/iso_mnt/boot/grub/grub.cfg
echo "Config injected:"
cat /tmp/iso_mnt/boot/grub/grub.cfg
sudo umount /tmp/iso_mnt

# Replace efi.img in the ISO build directory
cp "$EFIIMG" "$ISO_DIR/efi.img"

# Rebuild ISO with the fixed efi.img
echo "Rebuilding ISO with fixed efi.img..."
grub-mkrescue -V KARTEOS -o "$ISO_OUT" "$ISO_DIR" 2>/dev/null

rm -rf "$TMPDIR" /tmp/grub_inject.cfg
echo "Done! ISO rebuilt at $ISO_OUT"
