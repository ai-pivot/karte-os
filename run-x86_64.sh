#!/bin/bash
# 启动 KarteOS x86_64 QEMU，直接进入 shell 交互模式
# 在 shell 里输入 xbot-cli-static 即可启动 xbot TUI

cd "$(dirname "$0")"

echo "启动 KarteOS x86_64..."
echo "进入 shell 后输入: xbot-cli-static"
echo "退出 QEMU: Ctrl+A 然后按 X"
echo "========================================"

qemu-system-x86_64 \
  -machine pc -cpu qemu64 -m 512M \
  -cdrom target/karte-os-x86_64.iso \
  -serial stdio \
  -display none -no-reboot \
  -drive file=disk.img,format=raw,if=none,id=hd0 \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=hd0,bus=ahci.0
