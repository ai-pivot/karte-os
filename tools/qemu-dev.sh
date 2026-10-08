#!/usr/bin/env bash
# karte-sdk — QEMU 一键开发环境：构建用户程序 + 部署 + 启动 QEMU
# 用法：tools/qemu-dev.sh [--arch x86_64] [--no-deploy]
set -euo pipefail
cd "$(dirname "$0")/.."

ARCH=riscv64
[[ "${1:-}" == "--arch" ]] && ARCH="$2" && shift 2 || true
[[ "${1:-}" == "--no-deploy" ]] && NO_DEPLOY=1 && shift || true

echo "== karte-dev: building user programs (${ARCH}) =="
(cd user && make ARCH="$ARCH")

echo "== karte-dev: building kernel =="
if [[ "$ARCH" == "riscv64" ]]; then
    cargo build --release -p karte-os-kernel --target riscv64gc-unknown-none-elf
    KERNEL=target/riscv64gc-unknown-none-elf/release/karte-os-kernel
elif [[ "$ARCH" == "x86_64" ]]; then
    cargo +nightly build --release -p karte-os-kernel --target x86_64-unknown-none -Z build-std=core,alloc
    KERNEL=target/x86_64-unknown-none/release/karte-os-kernel
else
    echo "unsupported arch: $ARCH" >&2; exit 1
fi

if [[ "${NO_DEPLOY:-0}" != 1 ]]; then
    echo "== karte-dev: deploying to disk =="
    tools/mkdisk.sh deploy >/dev/null 2>&1 || tools/mkdisk.sh init
fi

echo "== karte-dev: launching QEMU (${ARCH}) — Ctrl+A X to exit =="
if [[ "$ARCH" == "riscv64" ]]; then
    exec qemu-system-riscv64 -machine virt -nographic -bios default \
        -kernel "$KERNEL" \
        -drive file=disk.img,format=raw,if=none,id=hd0 \
        -device virtio-blk-device,drive=hd0
else
    make shell-x86
fi
