#!/bin/bash
# tools/mkdisk.sh — Manage the KarteOS FAT32 disk image
#
# Usage:
#   ./tools/mkdisk.sh init          # Create a new 64MB FAT32 disk.img
#   ./tools/mkdisk.sh format        # Format existing disk.img as FAT32
#   ./tools/mkdisk.sh list          # List files on disk.img
#   ./tools/mkdisk.sh put <src> [dst]  # Copy host file to disk image
#   ./tools/mkdisk.sh get <src> [dst]  # Copy file from disk image to host
#   ./tools/mkdisk.sh rm <file>     # Delete file from disk image
#   ./tools/mkdisk.sh info          # Show disk image info

set -e

DISK="${DISK:-disk.img}"
SIZE="${SIZE:-64}"  # MB

cmd_init() {
    echo "Creating ${SIZE}MB disk image: $DISK"
    dd if=/dev/zero of="$DISK" bs=1M count="$SIZE" status=progress
    mkfs.vfat -F 32 "$DISK"
    echo "Done. FAT32 filesystem created."
}

cmd_format() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found. Run 'init' first."
        exit 1
    fi
    echo "Formatting $DISK as FAT32..."
    mkfs.vfat -F 32 "$DISK"
    echo "Done."
}

cmd_list() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found."
        exit 1
    fi
    echo "Files on $DISK:"
    mdir -i "$DISK" :: 2>/dev/null || echo "(empty or not formatted)"
}

cmd_put() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found. Run 'init' first."
        exit 1
    fi
    local src="$1"
    local dst="${2:-$(basename "$src")}"
    if [ ! -f "$src" ]; then
        echo "Error: source file '$src' not found."
        exit 1
    fi
    echo "Copying $src -> $DISK::$dst"
    mcopy -i "$DISK" "$src" "::$dst"
    echo "Done. File size: $(wc -c < "$src") bytes"
}

cmd_get() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found."
        exit 1
    fi
    local src="$1"
    local dst="${2:-$(basename "$src")}"
    echo "Copying $DISK::$src -> $dst"
    mcopy -i "$DISK" "::$src" "$dst"
    echo "Done."
}

cmd_rm() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found."
        exit 1
    fi
    local file="$1"
    echo "Deleting $file from $DISK"
    mdel -i "$DISK" "::$file"
    echo "Done."
}

cmd_info() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found."
        exit 1
    fi
    echo "Disk image: $DISK"
    echo "Size: $(du -h "$DISK" | cut -f1)"
    echo "Type: $(file "$DISK" | cut -d: -f2 | xargs)"
    echo ""
    echo "FAT32 info:"
    mdir -i "$DISK" :: 2>/dev/null || echo "(not formatted)"
}

case "${1:-help}" in
    init)   cmd_init ;;
    format) cmd_format ;;
    list|ls) cmd_list ;;
    put|cp)  shift; cmd_put "$@" ;;
    get)     shift; cmd_get "$@" ;;
    rm|del)  shift; cmd_rm "$@" ;;
    info)    cmd_info ;;
    help|*)
        echo "KarteOS Disk Image Manager"
        echo ""
        echo "Usage: $0 <command> [args...]"
        echo ""
        echo "Commands:"
        echo "  init                 Create new ${SIZE}MB FAT32 disk image"
        echo "  format               Format existing disk as FAT32"
        echo "  list                 List files on disk"
        echo "  put <src> [dst]      Copy host file to disk"
        echo "  get <src> [dst]      Copy file from disk to host"
        echo "  rm <file>            Delete file from disk"
        echo "  info                 Show disk image info"
        echo ""
        echo "Environment:"
        echo "  DISK=disk.img        Disk image path (default: disk.img)"
        echo "  SIZE=64              Size in MB for 'init' (default: 64)"
        ;;
esac
