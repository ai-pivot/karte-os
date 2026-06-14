#!/bin/bash
# 启动 KarteOS RISC-V 64 QEMU，直接进入 shell 交互模式
# 在 shell 里输入: ls, xbot-cli-static 等
#
# 退出 QEMU: Ctrl+A 然后按 X
#
# 日志: 运行后再看 /tmp/karte-os-riscv64.log（tee 管道会干扰交互输入）

cd "$(dirname "$0")"

echo "退出: Ctrl+A 然后按 X"
echo "---"

# 直接运行 QEMU，无管道包装，确保 stdin/stdout 直通
exec qemu-system-riscv64 \
    -machine virt -cpu rv64 -bios default \
    -m 1024M -smp 1 \
    -serial stdio \
    -display none -no-reboot \
    -kernel target/riscv64gc-unknown-none-elf/release/karte-os-kernel \
    -drive id=blk0,file=disk.img,format=raw,if=none \
    -device virtio-blk-device,drive=blk0 \
    -netdev user,id=net0 \
    -device virtio-net-device,netdev=net0
