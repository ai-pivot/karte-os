#!/bin/bash
# 启动 KarteOS x86_64 QEMU，直接进入 shell 交互模式
# 在 shell 里输入: xbot-cli-static
#
# 如果 xbot 报错退出，先用 tools/mkdisk.sh 创建配置文件：
#   echo '{}' > /tmp/config.json && tools/mkdisk.sh put /tmp/config.json .xbot/config.json
#
# 退出 QEMU: Ctrl+A 然后按 X

cd "$(dirname "$0")"

exec qemu-system-x86_64 \
  -machine pc -cpu qemu64 -m 512M \
  -cdrom target/karte-os-x86_64.iso \
  -serial stdio \
  -display none -no-reboot \
  -drive file=disk.img,format=raw,if=none,id=hd0 \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=hd0,bus=ahci.0
