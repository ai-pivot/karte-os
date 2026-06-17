#!/bin/bash
# tools/mkdisk.sh — Manage the KarteOS disk image (ext4 or FAT32)
#
# No sudo required! Uses debugfs for ext4 and mtools for FAT32.
#
# Usage:
#   ./tools/mkdisk.sh init          # Create a new ext4 disk.img
#   ./tools/mkdisk.sh init-fat32    # Create a new FAT32 disk.img
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
#
# Environment:
#   DISK=disk.img    Path to disk image (default: disk.img)
#   SIZE=256         Size in MB for 'init' (default: 64)

set -e

DISK="${DISK:-disk.img}"
SIZE="${SIZE:-256}"  # MB
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJ_DIR="$(dirname "$SCRIPT_DIR")"

# ═══════════════════════════════════════════════════════════════════
#  Filesystem creation
# ═══════════════════════════════════════════════════════════════════

cmd_init() {
    echo "[disk] Creating ${SIZE}MB ext4 disk image: $DISK"
    dd if=/dev/zero of="$DISK" bs=1M count="$SIZE" status=progress 2>/dev/null
    _mkfs_ext4 "$DISK"
    echo "[disk] Done."
}

cmd_format() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found."; exit 1; }
    echo "[disk] Formatting $DISK as ext4..."
    _mkfs_ext4 "$DISK"
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

# Create ext4 filesystem with options compatible with the kernel's ext4_rs:
# - Disable 64bit, journal, metadata_csum, flex_bg, extra_isize for simplicity
_mkfs_ext4() {
    local img="$1"
    mkfs.ext4 -F -O ^64bit -O ^has_journal -O ^metadata_csum -O ^flex_bg -O ^extra_isize \
        -b 4096 -L karteos "$img" >/dev/null 2>&1
}

# ═══════════════════════════════════════════════════════════════════
#  ext4 operations — uses debugfs (no sudo/root required)
# ═══════════════════════════════════════════════════════════════════

# Check that debugfs is available
_require_debugfs() {
    if ! command -v debugfs &>/dev/null; then
        echo "Error: debugfs not found. Install e2fsprogs: sudo apt install e2fsprogs"
        exit 1
    fi
}

# Write a file into ext4 image via debugfs
_ext4_put() {
    local src="$1" dst="$2" img="$3"
    _require_debugfs
    # Remove existing file first (ignore error if not found)
    debugfs -w -R "rm $dst" "$img" 2>/dev/null || true
    debugfs -w -R "write $src $dst" "$img" 2>/dev/null
    # Set executable permissions for binaries
    debugfs -w -R "set_inode_field $dst mode 0100755" "$img" 2>/dev/null || true
}

# Read a file from ext4 image via debugfs
_ext4_get() {
    local src="$1" dst="$2" img="$3"
    _require_debugfs
    debugfs -R "dump $src $dst" "$img" 2>/dev/null
}

# Delete a file from ext4 image via debugfs
_ext4_rm() {
    local file="$1" img="$2"
    _require_debugfs
    debugfs -w -R "rm $file" "$img" 2>/dev/null
}

# List files in ext4 image via debugfs
_ext4_list() {
    local img="$1"
    _require_debugfs
    # debugfs "ls -l" format:
    #   inode  mode_octal  (type)  uid  gid  size  date  time  name
    # Example: "    12  100755 (1)      0      0   713760 16-Jun-2026 10:51 shell"
    debugfs -R "ls -l" "$img" 2>/dev/null | while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        [[ "$line" =~ ^debugfs: ]] && continue
        # Parse: skip inode, mode, (type), uid, gid → grab size → rest is date+name
        local inode mode mode_type uid gid size rest
        read -r inode mode mode_type uid gid size rest <<< "$line"
        [[ -z "$rest" ]] && continue
        # rest = "date time name", extract last word as name
        local name="${rest##* }"
        name="${name//\//}"
        [[ "$name" == "." || "$name" == ".." || -z "$name" ]] && continue
        # Format size
        if [ "${size:-0}" -gt 1048576 ] 2>/dev/null; then
            printf "%-32s %dMB\n" "$name" $((size / 1048576))
        elif [ "${size:-0}" -gt 1024 ] 2>/dev/null; then
            printf "%-32s %dKB\n" "$name" $((size / 1024))
        else
            printf "%-32s %dB\n" "$name" "${size:-0}"
        fi
    done
}

# Create directory in ext4 image
_ext4_mkdir() {
    local dir="$1" img="$2"
    _require_debugfs
    debugfs -w -R "mkdir $dir" "$img" 2>/dev/null
}

# ═══════════════════════════════════════════════════════════════════
#  Filesystem detection
# ═══════════════════════════════════════════════════════════════════

_detect_fs() {
    file "$DISK" | grep -o 'ext[234]\|FAT\|data' | head -1
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
            for elf in "$user_dir"/*.elf; do
                [ -f "$elf" ] || continue
                local base="$(basename "$elf" .elf)"
                # Skip RISC-V assembly stubs on x86_64
                if [ "$arch" = "x86_64" ]; then
                    case "$base" in
                        hello|heap_test|file_test|spawn_test)
                            local size
                            size=$(stat -c%s "$elf" 2>/dev/null || echo "0")
                            [ "$size" -lt 100 ] && continue
                            ;;
                    esac
                fi
                _ext4_put "$elf" "$base" "$DISK"
                count=$((count + 1))
            done
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
    [ -f "$DISK" ] || cmd_init
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
#  File operations (no sudo — debugfs for ext4, mtools for FAT32)
# ═══════════════════════════════════════════════════════════════════

cmd_list() {
    [ -f "$DISK" ] || { echo "Error: $DISK not found."; exit 1; }
    local fs_type=$(_detect_fs)
    case "$fs_type" in
        ext*)
            echo "Files on $DISK (ext4):"
            _ext4_list "$DISK" || echo "(empty)"
            ;;
        FAT*)
            echo "Files on $DISK (FAT32):"
            mdir -i "$DISK" :: 2>/dev/null || echo "(empty)"
            ;;
        *)
            echo "(unknown filesystem — run 'init' to format)"
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
            _ext4_put "$src" "$dst" "$DISK"
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
            _ext4_get "$src" "$dst" "$DISK"
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
            _ext4_rm "$file" "$DISK"
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
        echo "KarteOS Disk Image Manager (no-sudo)"
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
        echo "File operations (no sudo required!):"
        echo "  list                 List files on disk"
        echo "  put <src> [dst]      Copy host file to disk"
        echo "  get <src> [dst]      Copy file from disk to host"
        echo "  rm <file>            Delete file from disk"
        echo "  info                 Show disk image info"
        echo ""
        echo "Environment:"
        echo "  DISK=disk.img        Disk image path (default: disk.img)"
        echo "  SIZE=256             Size in MB for 'init' (default: 64)"
        ;;
esac
