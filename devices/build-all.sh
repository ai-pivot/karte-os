#!/usr/bin/env bash
# 构建全部设备固件 → devices/artifacts/
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ART="$ROOT/devices/artifacts"
mkdir -p "$ART"

echo "[build] S 档 MCU 设备 (riscv32imc)"
cd "$ROOT/devices/mcu-rv32"
KARTE_DEVICE_ID=esp32-sensor KARTE_ROLE=sensor cargo build --release
cp target/riscv32imc-unknown-none-elf/release/karte-mcu "$ART/esp32-sensor"
KARTE_DEVICE_ID=esp32-relay KARTE_ROLE=relay cargo build --release
cp target/riscv32imc-unknown-none-elf/release/karte-mcu "$ART/esp32-relay"

echo "[build] aarch64 档设备 (EL1/PL011)"
cd "$ROOT/devices/aarch64-node"
cargo build --release
cp target/aarch64-unknown-none-softfloat/release/karte-a-node "$ART/karte-a-node"

echo "[build] M 档主内核设备 (rv64, --features fabric_node)"
cd "$ROOT"
CARGO_TARGET_DIR=/tmp/karte-fabric-target \
  cargo build --release -p karte-os-kernel --features fabric_node \
  --target riscv64gc-unknown-none-elf
cp /tmp/karte-fabric-target/riscv64gc-unknown-none-elf/release/karte-os-kernel "$ART/karte-m-gw"

echo "[build] artifacts:"
ls -la "$ART" | awk '{print "  " $5, $9}'
