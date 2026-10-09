#!/usr/bin/env bash
# KarteOS 多设备织物一键真测
#
# 启动 N 台异构 IoT 设备（每台跑 KarteOS 的某个档位构建，各自 QEMU 实例），
# 全部通过 unix socket 暴露串口 → 脑端桥统一发现（CapDesc announce）+
# 统一调用（invoke → tool result）→ 输出「脑-肢体」猜想验证证据。
#
# 用法: run-fabric.sh [deadline_s]
#   FABRIC_SOCKDIR  套接字目录（默认 /tmp/karte-fabric）
#   FABRIC_LOGDIR   设备串口日志目录（默认 /tmp/karte-fabric-logs）
#   QEMU32/QEMU64/QEMU_A64  QEMU 可执行（默认系统路径）
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ART="$ROOT/devices/artifacts"
SOCKDIR="${FABRIC_SOCKDIR:-/tmp/karte-fabric}"
LOGDIR="${FABRIC_LOGDIR:-/tmp/karte-fabric-logs}"
DEADLINE="${1:-30}"
QEMU32="${QEMU32:-qemu-system-riscv32}"
QEMU64="${QEMU64:-qemu-system-riscv64}"
QEMU_A64="${QEMU_A64:-qemu-system-aarch64}"

# 设备表: "id|artifact|machine"  machine ∈ {rv32, rv64, a64}
DEVICES=(
  "esp32-sensor|$ART/esp32-sensor|rv32"
  "esp32-relay|$ART/esp32-relay|rv32"
)
[ -f "$ART/karte-m-gw" ] && DEVICES+=("karte-m-gw|$ART/karte-m-gw|rv64")
[ -f "$ART/karte-a-node" ] && DEVICES+=("karte-a-node|$ART/karte-a-node|a64")

mkdir -p "$SOCKDIR" "$LOGDIR"
# 清残留套接字（QEMU server socket 需新创建）
find "$SOCKDIR" -maxdepth 1 -name "*.sock" -delete 2>/dev/null

PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do
    [ -n "$p" ] && kill "$p" 2>/dev/null
  done
  wait 2>/dev/null
}
trap cleanup EXIT

launch() { # id artifact machine
  local id="$1" art="$2" mach="$3"
  local sock="$SOCKDIR/$id.sock"
  local log="$LOGDIR/$id.log"
  case "$mach" in
    rv32)
      "$QEMU32" -machine virt -nographic -bios none -kernel "$art" \
        -netdev user,id=n0 -device virtio-net-device,netdev=n0 \
        -chardev "socket,id=s0,path=$sock,server=on,wait=off" -serial chardev:s0 \
        > "$log" 2>&1 &
      ;;
    rv64)
      "$QEMU64" -machine virt -nographic -bios default -kernel "$art" \
        -drive file="$ROOT/disk.img",format=raw,if=none,id=hd0 \
        -device virtio-blk-device,drive=hd0 \
        -netdev user,id=n0 -device virtio-net-device,netdev=n0 \
        -chardev "socket,id=s0,path=$sock,server=on,wait=off" -serial chardev:s0 \
        > "$log" 2>&1 &
      ;;
    a64)
      # QEMU aarch64 virt 默认 CPU 是 cortex-a15（AArch32）——无法执行
      # AArch64 ELF，必须显式指定 64 位 CPU；且 6.2 不支持 -bios none
      "$QEMU_A64" -machine virt -cpu cortex-a72 -nographic -kernel "$art" \
        -chardev "socket,id=s0,path=$sock,server=on,wait=off" -serial chardev:s0 \
        > "$log" 2>&1 &
      ;;
  esac
  PIDS+=("$!")
  echo "[fabric] launched $id ($mach) sock=$sock log=$log"
}

echo "[fabric] === KarteOS multi-device fabric real-test ==="
for d in "${DEVICES[@]}"; do
  IFS='|' read -r id art mach <<< "$d"
  if [ ! -f "$art" ]; then
    echo "[fabric] SKIP $id — artifact missing: $art"
    continue
  fi
  launch "$id" "$art" "$mach"
done

# 脑端桥：统一发现 + 统一调用
python3 "$ROOT/devices/bridge/fabric_brain.py" "$SOCKDIR" "${#DEVICES[@]}" "" "$DEADLINE"
RC=$?
echo "[fabric] bridge rc=$RC"
exit $RC
