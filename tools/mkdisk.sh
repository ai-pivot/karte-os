#!/bin/bash
# tools/mkdisk.sh — Manage the KarteOS disk image (ext4 or FAT32)
#
# Usage:
#   ./tools/mkdisk.sh init          # Create a new 64MB ext4 disk.img
#   ./tools/mkdisk.sh init-fat32    # Create a new 64MB FAT32 disk.img
#   ./tools/mkdisk.sh format        # Format existing disk.img as ext4
#   ./tools/mkdisk.sh format-fat32  # Format existing disk.img as FAT32
#   ./tools/mkdisk.sh list          # List files on disk.img
#   ./tools/mkdisk.sh put <src> [dst]  # Copy host file to disk image
#   ./tools/mkdisk.sh get <src> [dst]  # Copy file from disk image to host
#   ./tools/mkdisk.sh rm <file>     # Delete file from disk image
#   ./tools/mkdisk.sh info          # Show disk image info

set -e

DISK="${DISK:-disk.img}"
SIZE="${SIZE:-64}"  # MB
MOUNT="/tmp/karteos-mnt"

# ── ext4 commands ─────────────────────────────────────────────────────

cmd_init() {
    echo "Creating ${SIZE}MB ext4 disk image: $DISK"
    dd if=/dev/zero of="$DISK" bs=1M count="$SIZE" status=progress
    mkfs.ext4 -b 4096 -L karteos "$DISK"
    echo "Done. ext4 filesystem created (block size: 4096)."
}

cmd_format() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found. Run 'init' first."
        exit 1
    fi
    echo "Formatting $DISK as ext4 (4096-byte blocks)..."
    mkfs.ext4 -b 4096 -L karteos "$DISK"
    echo "Done."
}

# ── FAT32 commands ────────────────────────────────────────────────────

cmd_init_fat32() {
    echo "Creating ${SIZE}MB FAT32 disk image: $DISK"
    dd if=/dev/zero of="$DISK" bs=1M count="$SIZE" status=progress
    mkfs.vfat -F 32 "$DISK"
    echo "Done. FAT32 filesystem created."
}

cmd_format_fat32() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found. Run 'init' first."
        exit 1
    fi
    echo "Formatting $DISK as FAT32..."
    mkfs.vfat -F 32 "$DISK"
    echo "Done."
}

# ── Shared commands (mount-based, works for both ext4 and FAT32) ──────

_mount() {
    mkdir -p "$MOUNT"
    sudo mount -o loop "$DISK" "$MOUNT"
}

_unmount() {
    sudo umount "$MOUNT" 2>/dev/null || true
    rmdir "$MOUNT" 2>/dev/null || true
}

_detect_fs() {
    local fs_type
    fs_type=$(file "$DISK" | grep -o 'ext[234]\|FAT\|data' | head -1)
    echo "$fs_type"
}

cmd_list() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found."
        exit 1
    fi
    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "Files on $DISK (ext4):"
            _mount
            find "$MOUNT" -maxdepth 1 -type f -printf "%f %s\n" 2>/dev/null || echo "(empty)"
            find "$MOUNT" -maxdepth 1 -type d -not -path "$MOUNT" -printf "%f/ -\n" 2>/dev/null || true
            _unmount
            ;;
        FAT*)
            echo "Files on $DISK (FAT32):"
            mdir -i "$DISK" :: 2>/dev/null || echo "(empty or not formatted)"
            ;;
        *)
            echo "Files on $DISK:"
            # Try mount-based approach
            _mount 2>/dev/null && {
                ls -la "$MOUNT" 2>/dev/null || echo "(empty)"
                _unmount
            } || echo "(unable to read — format disk first)"
            ;;
    esac
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

    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "Copying $src -> $DISK::$dst (ext4)"
            _mount
            sudo cp "$src" "$MOUNT/$dst"
            sudo sync
            _unmount
            echo "Done. File size: $(wc -c < "$src") bytes"
            ;;
        FAT*)
            echo "Copying $src -> $DISK::$dst (FAT32)"
            mcopy -i "$DISK" "$src" "::$dst"
            echo "Done. File size: $(wc -c < "$src") bytes"
            ;;
        *)
            echo "Error: Unknown filesystem. Format disk first."
            exit 1
            ;;
    esac
}

cmd_get() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found."
        exit 1
    fi
    local src="$1"
    local dst="${2:-$(basename "$src")}"

    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "Copying $DISK::$src -> $dst (ext4)"
            _mount
            cp "$MOUNT/$src" "$dst"
            _unmount
            echo "Done."
            ;;
        FAT*)
            echo "Copying $DISK::$src -> $dst (FAT32)"
            mcopy -i "$DISK" "::$src" "$dst"
            echo "Done."
            ;;
        *)
            echo "Error: Unknown filesystem."
            exit 1
            ;;
    esac
}

cmd_rm() {
    if [ ! -f "$DISK" ]; then
        echo "Error: $DISK not found."
        exit 1
    fi
    local file="$1"
    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "Deleting $file from $DISK (ext4)"
            _mount
            sudo rm "$MOUNT/$file"
            sudo sync
            _unmount
            echo "Done."
            ;;
        FAT*)
            echo "Deleting $file from $DISK (FAT32)"
            mdel -i "$DISK" "::$file"
            echo "Done."
            ;;
        *)
            echo "Error: Unknown filesystem."
            exit 1
            ;;
    esac
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
    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "ext4 info:"
            dumpe2fs "$DISK" 2>/dev/null | head -20 || echo "(unable to read)"
            ;;
        FAT*)
            echo "FAT32 info:"
            mdir -i "$DISK" :: 2>/dev/null || echo "(not formatted)"
            ;;
        *)
            echo "(unknown filesystem — run 'init' to format)"
            ;;
    esac
}

case "${1:-help}" in
    init)         cmd_init ;;
    init-fat32)   cmd_init_fat32 ;;
    format)       cmd_format ;;
    format-fat32) cmd_format_fat32 ;;
    list|ls)      cmd_list ;;
    put|cp)       shift; cmd_put "$@" ;;
    get)          shift; cmd_get "$@" ;;
    rm|del)       shift; cmd_rm "$@" ;;
    info)         cmd_info ;;
    help|*)
        echo "KarteOS Disk Image Manager"
        echo ""
        echo "Usage: $0 <command> [args...]"
        echo ""
        echo "Commands:"
        echo "  init                 Create new ${SIZE}MB ext4 disk image (default)"
        echo "  init-fat32           Create new ${SIZE}MB FAT32 disk image"
        echo "  format               Format existing disk as ext4"
        echo "  format-fat32         Format existing disk as FAT32"
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
