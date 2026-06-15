#!/bin/bash
# 启动 KarteOS RISC-V 64 QEMU，直接进入 shell 交互模式
# 在 shell 里输入: ls, xbot-cli-static 等
#
# 退出 QEMU: Ctrl+A 然后按 X
#
# 所有输出实时写入日志文件

cd "$(dirname "$0")"

LOG=/tmp/karte-os-riscv64.log

# 清空旧日志
> "$LOG"

echo "QEMU 日志: $LOG (实时同步)"
echo "退出: Ctrl+A 然后按 X"
echo "---"

# script -c: 把整个 QEMU 命令作为单个参数，避免参数被 script 误解析
# -f: 每次 write 后立即 flush（实时同步到日志文件）
# -q: 安静模式（不打印 script 自身的开始/结束消息）
exec script -q -f -c \
  "qemu-system-riscv64 \
    -machine virt -cpu rv64 -bios default \
    -m 2048M -smp 1 \
    -serial stdio \
    -display none -no-reboot \
    -kernel target/riscv64gc-unknown-none-elf/release/karte-os-kernel \
    -drive id=blk0,file=disk.img,format=raw,if=none \
    -device virtio-blk-device,drive=blk0 \
    -netdev user,id=net0 \
    -device virtio-net-device,netdev=net0" \
  "$LOG"
