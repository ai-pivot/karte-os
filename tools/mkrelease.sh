#!/bin/bash
# tools/mkrelease.sh — Build distributable KarteOS image packages
#
# Produces self-contained tarballs that anyone can run with just QEMU:
#
#   karteos-riscv64-v0.2.0.tar.gz
#   ├── karte-os-kernel       (RISC-V kernel binary)
#   ├── disk.img              (ext4 disk with all user programs)
#   ├── run.sh                (one-command QEMU launcher)
#   └── README.txt            (quick start guide)
#
# Usage:
#   ./tools/mkrelease.sh              # Build RISC-V release (default)
#   ./tools/mkrelease.sh riscv64      # Build RISC-V release explicitly
#   ./tools/mkrelease.sh x86_64       # Build x86_64 release (ISO + disk)
#   ./tools/mkrelease.sh both         # Build both architectures
#   ./tools/mkrelease.sh riscv64 256  # Specify disk size in MB
#
# Requirements:
#   - Rust toolchain (stable for RISC-V, nightly for x86_64)
#   - QEMU (qemu-system-riscv64 and/or qemu-system-x86_64)
#   - e2fsprogs (mkfs.ext4, debugfs) — no sudo needed!
#   - For x86_64: grub-common + xorriso

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJ_DIR="$(dirname "$SCRIPT_DIR")"
ARCH="${1:-riscv64}"
DISK_SIZE="${2:-64}"
VERSION="${VERSION:-$(cd "$PROJ_DIR" && git describe --tags --always --dirty 2>/dev/null || echo 'dev')}"

cd "$PROJ_DIR"

echo "╔═══════════════════════════════════════════════════╗"
echo "║  KarteOS Release Builder                          ║"
echo "║  Version: $VERSION"
echo "║  Arch:    $ARCH"
echo "║  Disk:    ${DISK_SIZE}MB"
echo "╚═══════════════════════════════════════════════════╝"

# ─── RISC-V 64 release ───────────────────────────────────────────

build_riscv64() {
    local OUTDIR="target/release/karteos-riscv64-$VERSION"
    local TARBALL="target/release/karteos-riscv64-$VERSION.tar.gz"

    echo ""
    echo "[1/5] Building RISC-V user programs..."
    cd user && make ARCH=riscv64 clean > /dev/null 2>&1 && make ARCH=riscv64 > /dev/null 2>&1
    cd "$PROJ_DIR"

    echo "[2/5] Building RISC-V kernel..."
    cargo build --release -p karte-os-kernel --target riscv64gc-unknown-none-elf 2>&1 | tail -1

    echo "[3/5] Creating disk image with user programs..."
    mkdir -p "$OUTDIR"
    DISK="$OUTDIR/disk.img" SIZE="$DISK_SIZE" bash tools/mkdisk.sh init 2>&1 | tail -1
    DISK="$OUTDIR/disk.img" bash tools/mkdisk.sh deploy-riscv 2>&1 | tail -1

    echo "[4/5] Copying kernel and scripts..."
    cp target/riscv64gc-unknown-none-elf/release/karte-os-kernel "$OUTDIR/"

    # Generate the one-command run script
    cat > "$OUTDIR/run.sh" << 'RUNEOF'
#!/bin/bash
# KarteOS — RISC-V 64 QEMU Launcher
# Just run: ./run.sh
set -e

DIR="$(cd "$(dirname "$0")" && pwd)"

# Try qemu-system-riscv64, fall back to common locations
QEMU="${QEMU:-qemu-system-riscv64}"
if ! command -v "$QEMU" &>/dev/null; then
    echo "Error: $QEMU not found. Install with:"
    echo "  Ubuntu/Debian: sudo apt install qemu-system-misc"
    echo "  macOS:         brew install qemu"
    exit 1
fi

echo "Starting KarteOS (RISC-V 64)..."
echo "QEMU exit: Ctrl+A then X"
echo ""

exec "$QEMU" \
    -machine virt -cpu rv64 -bios default -nographic \
    -m 256M -smp 1 \
    -drive id=blk0,file="$DIR/disk.img",format=raw,if=none \
    -device virtio-blk-device,drive=blk0 \
    -netdev user,id=net0,hostfwd=tcp::2323-:23 \
    -device virtio-net-device,netdev=net0 \
    -kernel "$DIR/karte-os-kernel"
RUNEOF
    chmod +x "$OUTDIR/run.sh"

    # Generate README
    cat > "$OUTDIR/README.txt" << READMEEOF
╔═══════════════════════════════════════════════════╗
║  KarteOS $VERSION (RISC-V 64)
╚═══════════════════════════════════════════════════╝

QUICK START
-----------
  ./run.sh

WHAT'S INCLUDED
---------------
  karte-os-kernel   RISC-V 64 kernel (S-mode, OpenSBI)
  disk.img          ext4 disk with user programs:
                     shell, ls, cat, echo, grep, sed, wc,
                     head, tail, mkdir, rm, env, pwd, dmesg
  run.sh            One-command QEMU launcher

REQUIREMENTS
------------
  qemu-system-riscv64 (8.2+)

  Install on Ubuntu/Debian:  sudo apt install qemu-system-misc
  Install on macOS:          brew install qemu

USAGE
-----
  Start the system:   ./run.sh
  Exit QEMU:          Ctrl+A then X

  Inside the shell:
    \$ help            Show available commands
    \$ ls              List files on disk
    \$ echo hello      Print text
    \$ cat README.txt  Display this file

NETWORK (optional)
------------------
  The VM is on 10.0.2.15/24 (QEMU user-mode networking).
  Port 23 (telnet) is forwarded to host port 2323:
    telnet localhost 2323

CREDITS
-------
  KarteOS — A dual-architecture OS in Rust 2024 Edition
  https://github.com/ai-pivot/karte-os
READMEEOF

    echo "[5/5] Creating tarball..."
    cd target/release
    tar czf "karteos-riscv64-$VERSION.tar.gz" \
        "karteos-riscv64-$VERSION/karte-os-kernel" \
        "karteos-riscv64-$VERSION/disk.img" \
        "karteos-riscv64-$VERSION/run.sh" \
        "karteos-riscv64-$VERSION/README.txt"
    cd "$PROJ_DIR"

    # Generate checksum
    local basename
    basename="$(basename "$TARBALL")"
    (cd "$(dirname "$TARBALL")" && sha256sum "$basename" > "$basename.sha256")

    local size
    size=$(du -h "$TARBALL" | cut -f1)
    echo ""
    echo "✅ RISC-V release ready:"
    echo "   $TARBALL ($size)"
    echo "   $TARBALL.sha256"
}

# ─── x86_64 release ──────────────────────────────────────────────

build_x86_64() {
    local OUTDIR="target/release/karteos-x86_64-$VERSION"
    local TARBALL="target/release/karteos-x86_64-$VERSION.tar.gz"

    echo ""
    echo "[1/6] Building x86_64 user programs..."
    cd user && make ARCH=x86_64 clean > /dev/null 2>&1 && make ARCH=x86_64 > /dev/null 2>&1
    cd "$PROJ_DIR"

    echo "[2/6] Building x86_64 kernel..."
    rm -f target/x86_64-unknown-none/release/karte-os-kernel
    rm -rf target/x86_64-unknown-none/release/.fingerprint/karte-os-kernel-*
    cargo +nightly build --release --target x86_64-unknown-none -p karte-os-kernel \
        -Z build-std=core,alloc 2>&1 | tail -1

    echo "[3/6] Creating bootable ISO..."
    local ISO_DIR="target/x86_64-iso"
    local ISO_FILE="target/karte-os-x86_64.iso"
    mkdir -p "$ISO_DIR/boot/grub"
    cp target/x86_64-unknown-none/release/karte-os-kernel "$ISO_DIR/boot/karte-os-kernel"
    printf 'set timeout=0\nset default=0\nmenuentry "KarteOS" {\n    multiboot2 /boot/karte-os-kernel\n    boot\n}\n' \
        > "$ISO_DIR/boot/grub/grub.cfg"
    grub-mkrescue -o "$ISO_FILE" "$ISO_DIR" 2>/dev/null

    echo "[4/6] Creating disk image with user programs..."
    mkdir -p "$OUTDIR"
    DISK="$OUTDIR/disk.img" SIZE="$DISK_SIZE" bash tools/mkdisk.sh init 2>&1 | tail -1
    DISK="$OUTDIR/disk.img" bash tools/mkdisk.sh deploy-x86 2>&1 | tail -1

    echo "[5/6] Copying ISO and scripts..."
    cp "$ISO_FILE" "$OUTDIR/"

    cat > "$OUTDIR/run.sh" << 'RUNEOF'
#!/bin/bash
# KarteOS — x86_64 QEMU Launcher
# Just run: ./run.sh
set -e

DIR="$(cd "$(dirname "$0")" && pwd)"

QEMU="${QEMU:-qemu-system-x86_64}"
if ! command -v "$QEMU" &>/dev/null; then
    echo "Error: $QEMU not found. Install with:"
    echo "  Ubuntu/Debian: sudo apt install qemu-system-x86"
    echo "  macOS:         brew install qemu"
    exit 1
fi

echo "Starting KarteOS (x86_64)..."
echo "QEMU exit: Ctrl+A then X"
echo ""

exec "$QEMU" \
    -machine pc -cpu qemu64 -m 128M -smp 1 \
    -cdrom "$DIR/karte-os-x86_64.iso" -serial stdio -display none -no-reboot \
    -drive file="$DIR/disk.img",format=raw,if=none,id=hd0 \
    -device ich9-ahci,id=ahci \
    -device ide-hd,drive=hd0,bus=ahci.0 \
    -netdev user,id=net0 \
    -device virtio-net-pci,netdev=net0
RUNEOF
    chmod +x "$OUTDIR/run.sh"

    cat > "$OUTDIR/README.txt" << READMEEOF
╔═══════════════════════════════════════════════════╗
║  KarteOS $VERSION (x86_64)
╚═══════════════════════════════════════════════════╝

QUICK START
-----------
  ./run.sh

WHAT'S INCLUDED
---------------
  karte-os-x86_64.iso   Bootable GRUB ISO (kernel + bootloader)
  disk.img              ext4 disk with user programs
  run.sh                One-command QEMU launcher

REQUIREMENTS
------------
  qemu-system-x86_64 (8.2+)

  Install on Ubuntu/Debian:  sudo apt install qemu-system-x86
  Install on macOS:          brew install qemu

USAGE
-----
  Start:   ./run.sh
  Exit:    Ctrl+A then X
  Shell:   type 'help' for commands

CREDITS
-------
  KarteOS — A dual-architecture OS in Rust 2024 Edition
  https://github.com/ai-pivot/karte-os
READMEEOF

    echo "[6/6] Creating tarball..."
    cd target/release
    tar czf "karteos-x86_64-$VERSION.tar.gz" \
        "karteos-x86_64-$VERSION/karte-os-x86_64.iso" \
        "karteos-x86_64-$VERSION/disk.img" \
        "karteos-x86_64-$VERSION/run.sh" \
        "karteos-x86_64-$VERSION/README.txt"
    cd "$PROJ_DIR"

    sha256sum "$TARBALL" > "$TARBALL.sha256"

    local size
    size=$(du -h "$TARBALL" | cut -f1)
    echo ""
    echo "✅ x86_64 release ready:"
    echo "   $TARBALL ($size)"
    echo "   $TARBALL.sha256"
}

# ─── Dispatch ────────────────────────────────────────────────────

case "$ARCH" in
    riscv64|rv64|riscv)
        mkdir -p target/release
        build_riscv64
        ;;
    x86_64|x86|amd64)
        mkdir -p target/release
        build_x86_64
        ;;
    both|all)
        mkdir -p target/release
        build_riscv64
        build_x86_64
        ;;
    *)
        echo "Usage: $0 [riscv64|x86_64|both] [disk_size_mb]"
        echo ""
        echo "Examples:"
        echo "  $0                 # RISC-V release (default, 64MB disk)"
        echo "  $0 riscv64         # RISC-V release"
        echo "  $0 x86_64          # x86_64 release"
        echo "  $0 both            # Both architectures"
        echo "  $0 riscv64 128     # RISC-V with 128MB disk"
        exit 1
        ;;
esac

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Release build complete!"
echo "  Files in: target/release/"
echo "═══════════════════════════════════════════════════"
