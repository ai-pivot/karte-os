#!/bin/bash
# 启动 KarteOS x86_64 QEMU
# 预装: shell + ls/cat/echo/grep/sed/wc/head/tail/mkdir/rm/env/pwd + xbot-cli-static
#
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

# -m 512M: xbot-cli-static 需要 ~256MB 堆空间
# AHCI: 磁盘通过 AHCI/SATA 控制器连接（内核优先使用 AHCI）
# virtio-net-pci: 网络（QEMU user-mode, 10.0.2.15/24）
exec script -q -f -c \
  "qemu-system-x86_64 \
    -machine pc -cpu qemu64 -m 512M -smp 1 \
    -cdrom target/karte-os-x86_64.iso \
    -serial stdio \
    -display none -no-reboot \
    -drive file=disk.img,format=raw,if=none,id=hd0 \
    -device ich9-ahci,id=ahci \
    -device ide-hd,drive=hd0,bus=ahci.0 \
    -netdev user,id=net0 \
    -device virtio-net-pci,netdev=net0" \
  "$LOG"
