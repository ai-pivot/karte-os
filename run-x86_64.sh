#!/bin/bash
# 启动 KarteOS x86_64 QEMU，直接进入 shell 交互模式
# 在 shell 里输入: xbot-cli-static
#
# 退出 QEMU: Ctrl+A 然后按 X
#
# 所有输出实时写入日志文件

cd "$(dirname "$0")"

LOG=/tmp/karte-os-x86_64.log

# 清空旧日志
> "$LOG"

echo "QEMU 日志: $LOG (实时同步)"
echo "退出: Ctrl+A 然后按 X"
echo "---"

# script -c: 把整个 QEMU 命令作为单个参数，避免参数被 script 误解析
# -f: 每次 write 后立即 flush（实时同步到日志文件）
# -q: 安静模式（不打印 script 自身的开始/结束消息）
exec script -q -f -c \
  "qemu-system-x86_64 \
    -machine pc -cpu qemu64 -m 1024M \
    -cdrom target/karte-os-x86_64.iso \
    -serial stdio \
    -display none -no-reboot \
    -drive file=disk.img,format=raw,if=none,id=hd0 \
    -device ich9-ahci,id=ahci \
    -device ide-hd,drive=hd0,bus=ahci.0" \
  "$LOG"
