#!/bin/bash
# tools/mkdisk.sh — Manage the KarteOS disk image (ext4 or FAT32)
#
# Usage:
#   ./tools/mkdisk.sh init          # Create a new 64MB ext4 disk.img
#   ./tools/mkdisk.sh init-fat32    # Create a new 64MB FAT32 disk.img
#   ./tools/mkdisk.sh deploy        # Create disk + deploy ALL user programs
#   ./tools/mkdisk.sh deploy-riscv  # Deploy RISC-V user programs to disk
#   ./tools/mkdisk.sh deploy-x86    # Deploy x86_64 user programs to disk
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
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJ_DIR="$(dirname "$SCRIPT_DIR")"

cleanup_mount() {
    if mountpoint -q "$MOUNT"; then
        sudo umount "$MOUNT" 2>/dev/null || true
    fi
    rmdir "$MOUNT" 2>/dev/null || true
}

trap cleanup_mount EXIT INT TERM

# ═══════════════════════════════════════════════════════════════════
#  Filesystem creation
# ═══════════════════════════════════════════════════════════════════

cmd_init() {
    echo "[disk] Creating ${SIZE}MB ext4 disk image: $DISK"
    dd if=/dev/zero of="$DISK" bs=1M count="$SIZE" status=progress 2>/dev/null
    mkfs.ext4 -O ^64bit -O ^has_journal -O ^metadata_csum -O ^flex_bg -O ^extra_isize \
        -b 4096 -L karteos "$DISK" >/dev/null 2>&1
    echo "[disk] Done."
}

cmd_format() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found."; exit 1; }
    echo "[disk] Formatting $DISK as ext4..."
    mkfs.ext4 -b 4096 -L karteos "$DISK" >/dev/null 2>&1
    echo "[disk] Done."
}

cmd_init_fat32() {
    echo "[disk] Creating ${SIZE}MB FAT32 disk image: $DISK"
    dd if=/dev/zero of="$DISK" bs=1M count="$SIZE" status=progress 2>/dev/null
    mkfs.vfat -F 32 "$DISK" >/dev/null 2>&1
    echo "[disk] Done."
}

cmd_format_fat32() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found."; exit 1; }
    echo "[disk] Formatting $DISK as FAT32..."
    mkfs.vfat -F 32 "$DISK" >/dev/null 2>&1
    echo "[disk] Done."
}

# ═══════════════════════════════════════════════════════════════════
#  Deploy — batch-install user programs into disk image
# ═══════════════════════════════════════════════════════════════════

# Deploy user programs for a specific architecture.
# Strips .elf extension so shell can find them as bare commands.
_deploy_arch() {
    local arch="$1"
    local user_dir="$PROJ_DIR/user"
    local count=0

    echo "[deploy] Building user programs for $arch..."
    cd "$user_dir" && make "ARCH=$arch" clean > /dev/null 2>&1 && make "ARCH=$arch" > /dev/null 2>&1
    cd "$PROJ_DIR"

    echo "[deploy] Installing $arch programs into $DISK..."

    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            _mount
            for elf in "$user_dir"/*.elf; do
                [ -f "$elf" ] || continue
                local base="$(basename "$elf" .elf)"
                # Skip RISC-V assembly programs on x86_64 (they're empty stubs)
                if [ "$arch" = "x86_64" ]; then
                    case "$base" in
                        hello|heap_test|file_test|spawn_test)
                            # Check if it's a real binary (not an empty stub)
                            local size
                            size=$(stat -c%s "$elf" 2>/dev/null || echo "0")
                            [ "$size" -lt 100 ] && continue
                            ;;
                    esac
                fi
                sudo cp "$elf" "$MOUNT/$base"
                count=$((count + 1))
            done
            sudo sync
            _unmount
            ;;
        FAT*)
            for elf in "$user_dir"/*.elf; do
                [ -f "$elf" ] || continue
                local base="$(basename "$elf" .elf)"
                if [ "$arch" = "x86_64" ]; then
                    case "$base" in
                        hello|heap_test|file_test|spawn_test)
                            local size
                            size=$(stat -c%s "$elf" 2>/dev/null || echo "0")
                            [ "$size" -lt 100 ] && continue
                            ;;
                    esac
                fi
                mcopy -i "$DISK" "$elf" "::$base" 2>/dev/null || true
                count=$((count + 1))
            done
            ;;
        *)
            echo "Error: Unknown filesystem. Run 'init' first."
            exit 1
            ;;
    esac
    echo "[deploy] Installed $count programs ($arch)."
}

cmd_deploy() {
    # Create disk if it doesn't exist
    [ -f "$DISK" ] || cmd_init
    # Deploy RISC-V by default (primary architecture)
    _deploy_arch riscv64
}

cmd_deploy_riscv() {
    [ -f "$DISK" ] || cmd_init
    _deploy_arch riscv64
}

cmd_deploy_x86() {
    [ -f "$DISK" ] || cmd_init
    _deploy_arch x86_64
}

# ═══════════════════════════════════════════════════════════════════
#  File operations (mount-based, works for both ext4 and FAT32)
# ═══════════════════════════════════════════════════════════════════

_mount() {
    mkdir -p "$MOUNT"
    if mountpoint -q "$MOUNT"; then
        echo "[disk] Cleaning stale mount at $MOUNT"
        sudo umount "$MOUNT"
    fi
    sudo mount -o loop "$DISK" "$MOUNT"
}

_unmount() {
    cleanup_mount
}

_detect_fs() {
    file "$DISK" | grep -o 'ext[234]\|FAT\|data' | head -1
}

cmd_list() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found."; exit 1; }
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
            mdir -i "$DISK" :: 2>/dev/null || echo "(empty)"
            ;;
        *)
            _mount 2>/dev/null && {
                ls -la "$MOUNT" 2>/dev/null || echo "(empty)"
                _unmount
            } || echo "(unable to read — format disk first)"
            ;;
    esac
}

cmd_put() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found. Run 'init' first."; exit 1; }
    local src="$1"
    local dst="${2:-$(basename "$src")}"
    [ -f "$src" ] || { echo "Error: source '$src' not found."; exit 1; }

    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "[disk] $src -> $DISK::$dst (ext4)"
            _mount
            sudo cp "$src" "$MOUNT/$dst"
            sudo sync
            _unmount
            ;;
        FAT*)
            echo "[disk] $src -> $DISK::$dst (FAT32)"
            mcopy -i "$DISK" "$src" "::$dst"
            ;;
        *)
            echo "Error: Unknown filesystem."; exit 1
            ;;
    esac
    echo "[disk] Done ($(wc -c < "$src") bytes)"
}

cmd_get() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found."; exit 1; }
    local src="$1"
    local dst="${2:-$(basename "$src")}"

    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "[disk] $DISK::$src -> $dst (ext4)"
            _mount
            cp "$MOUNT/$src" "$dst"
            _unmount
            ;;
        FAT*)
            echo "[disk] $DISK::$src -> $dst (FAT32)"
            mcopy -i "$DISK" "::$src" "$dst"
            ;;
        *)
            echo "Error: Unknown filesystem."; exit 1
            ;;
    esac
    echo "[disk] Done."
}

cmd_rm() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found."; exit 1; }
    local file="$1"
    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "[disk] Deleting $file from $DISK (ext4)"
            _mount
            sudo rm "$MOUNT/$file"
            sudo sync
            _unmount
            ;;
        FAT*)
            echo "[disk] Deleting $file from $DISK (FAT32)"
            mdel -i "$DISK" "::$file"
            ;;
        *)
            echo "Error: Unknown filesystem."; exit 1
            ;;
    esac
    echo "[disk] Done."
}

cmd_info() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found."; exit 1; }
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

# ═══════════════════════════════════════════════════════════════════
#  CLI dispatch
# ═══════════════════════════════════════════════════════════════════

case "${1:-help}" in
    init)         cmd_init ;;
    init-fat32)   cmd_init_fat32 ;;
    format)       cmd_format ;;
    format-fat32) cmd_format_fat32 ;;
    deploy)       cmd_deploy ;;
    deploy-riscv) cmd_deploy_riscv ;;
    deploy-x86)   cmd_deploy_x86 ;;
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
        echo "Create / Format:"
        echo "  init                 Create ${SIZE}MB ext4 disk (default)"
        echo "  init-fat32           Create ${SIZE}MB FAT32 disk"
        echo "  format               Re-format as ext4"
        echo "  format-fat32         Re-format as FAT32"
        echo ""
        echo "Deploy (one-command setup):"
        echo "  deploy               Create disk + install all RISC-V programs"
        echo "  deploy-riscv         Install RISC-V user programs into disk"
        echo "  deploy-x86           Install x86_64 user programs into disk"
        echo ""
        echo "File operations:"
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
