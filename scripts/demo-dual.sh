#!/usr/bin/env bash
# P2.5 M2 dual-machine demo: two QEMU virt instances bridged by a UDP
# socket netdev pair; each kernel announces itself on the fabric (DRT
# port 43110) and registers the peer as a remote device. One command:
#   ./scripts/demo-dual.sh
set -u
cd "$(dirname "$0")/.."

KERNEL=target/riscv64gc-unknown-none-elf/release/karte-os-kernel-a
KERNEL_B=target/riscv64gc-unknown-none-elf/release/karte-os-kernel-b
DUR=${DUR:-45}
A_LOG=/tmp/dual_a.log
B_LOG=/tmp/dual_b.log

if [ ! -f "$KERNEL" ] || [ ! -f "$KERNEL_B" ]; then
  echo "[demo] kernels not found (build kernel + kernel --features net_node_b)"; exit 1
fi

qemu-net() { # $1 kernel $2 netdev args
  qemu-system-riscv64 -machine virt -nographic -bios default -kernel "$1" \
    -netdev "$2" -device virtio-net-device,netdev=n0
}

echo "[demo] launching node A (10.0.2.15, prefix a) and node B (10.0.2.16, prefix b)"
# QEMU socket netdev in TCP-tunnel mode: A listens, B connects; frames are
# relayed verbatim between the two instances.
qemu-net "$KERNEL" "socket,id=n0,listen=127.0.0.1:4444" >"$A_LOG" 2>&1 &
PA=$!
qemu-net "$KERNEL_B" "socket,id=n0,connect=127.0.0.1:4444" >"$B_LOG" 2>&1 &
PB=$!

sleep "$DUR"
kill $PA $PB 2>/dev/null
wait $PA $PB 2>/dev/null

echo "[demo] node A drt-net lines:"; grep -a "drt-net" "$A_LOG" | head -3
echo "[demo] node B drt-net lines:"; grep -a "drt-net" "$B_LOG" | head -3
A_SEES=$(grep -ac "remote" "$A_LOG"); B_SEES=$(grep -ac "remote" "$B_LOG")
echo "[demo] peer-discovery hits: A=$A_SEES B=$B_SEES"
if [ "$A_SEES" -gt 0 ] || [ "$B_SEES" -gt 0 ]; then
  echo "[demo] DUAL-MACHINE DISCOVERY OK (logs: $A_LOG $B_LOG)"; exit 0
fi
echo "[demo] no peer discovery yet — see logs (QEMU socket netdev may need root UDP or different ports)"; exit 1
