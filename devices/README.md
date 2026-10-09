# KarteOS 多设备织物 —— 虚拟 IoT 设备矩阵

**验证的猜想**：同一份 KarteOS 可以跨 ISA/档位运行在多种 IoT 设备上，每台设备出厂即带
能力描述（CapDesc），「脑」（host/云端）通过**统一的 KRT1 调用平面**发现并调用它们——
无需任何设备侧 MCP 适配代码（「驱动即工具」）。

## 设备矩阵（全部真测通过）

| 设备 id | 档位 | QEMU 机器 | ISA | 角色 | 出厂能力（CapDesc 工具集） |
|---|---|---|---|---|---|
| `esp32-sensor` | S 档 MCU（32KB 固件） | `qemu-system-riscv32 -machine virt` | riscv32imc | 温湿度传感器 | `sensor.temp`, `sensor.humidity`, `mqtt.ping` |
| `esp32-relay` | S 档 MCU | `qemu-system-riscv32 -machine virt` | riscv32imc | 继电器执行器 | `actuator.on`, `actuator.off`, `actuator.status` |
| `karte-m-gw` | M 档主内核（1.67MB） | `qemu-system-riscv64 -machine virt` | riscv64 | 边缘网关 | `gpio_read`, `gpio_write`, `timer_sleep_ms`, `timer_sleep_until`, `vfs_ls`, `vfs_read`, `vfs_write` |
| `karte-a-node` | aarch64 档（76KB） | `qemu-system-aarch64 -machine virt -cpu cortex-a72` | aarch64 | 边缘 AI 节点 | `camera.snapshot`, `npu.infer`, `node.info` |

- **MCU 档**（`devices/mcu-rv32`）：同一份源码经编译期参数（`KARTE_DEVICE_ID`/`KARTE_ROLE`）
  构建出不同角色的设备固件——一台 MCU 一个 QEMU 实例。
- **M 档**（`kernel/`，`--features fabric_node`）：主内核内嵌的真实 CapDesc 注册表
  （`capability::tool_names()`，来自 vfs/timer/gpio 驱动描述符）直接作为网关能力表发布。
- **aarch64 档**（`devices/aarch64-node`）：EL1 裸入口 + PL011 双向串口。
- 传输：**UART（网关串行织物）**——QEMU riscv32 virt 的 virtio DMA 有平台级问题
  （驱动序列经 `-trace` 证实 100% 正确但设备不消费描述符，见根 AGENTS.md），
  TCP 真实性由 RV64 主内核证据链覆盖。

## 构造与真测

```bash
# 1) 构建全部设备固件 → devices/artifacts/
devices/build-all.sh

# 2) 一键真测：启动 4 台异构设备 + 脑端桥统一发现/调用
devices/run-fabric.sh 60
```

### 真测输出（关键行）

```
[brain] device registered: esp32-sensor tools=sensor.temp,sensor.humidity,mqtt.ping
[brain] device registered: esp32-relay tools=actuator.on,actuator.off,actuator.status
[brain] device registered: karte-a-node tools=camera.snapshot,npu.infer,node.info
[brain] device registered: karte-m-gw tools=gpio_read,gpio_write,timer_sleep_ms,timer_sleep_until,vfs_ls,vfs_read,vfs_write
[brain] registered 4/4 devices — unified invoke plane
[brain] result <- karte-m-gw tool=gpio_read result=0
[brain] result <- karte-a-node tool=camera.snapshot result=640x480:ok
[brain] result <- esp32-sensor tool=sensor.temp result=23.5
[brain] result <- esp32-relay tool=actuator.on result=on
[brain] all 4/4 tool calls answered
[brain] FABRIC OK: 4 devices registered, 4 unified invocations answered
```

## 织物协议（KRT1，与 `kernel/src/drt.rs` 同构）

```
KRT1|A|<id>|<seq>|<tool1,tool2,...>    announce（设备 → 脑，含能力表）
KRT1|I|<id>|<seq>|<tool>               invoke（脑 → 设备，统一调用平面）
KRT1|T|<id>|<seq>|<result>             tool result（设备 → 脑）
```

## 目录

```
devices/
  build-all.sh        构建 4 台设备固件
  run-fabric.sh       一键真测（QEMU × 4 + 脑端桥）
  bridge/
    fabric_brain.py   脑端桥：多 socket 汇聚 + 设备注册表 + 统一 invoke 路由
  mcu-rv32/           S 档 MCU 设备固件（编译期参数化：device id + role）
  aarch64-node/       aarch64 档设备固件（EL1 + PL011）
  artifacts/          构建产物（gitignore，build-all.sh 生成）
```

## 实现要点 / 踩过的坑

- **16550 RX FIFO 仅 16 字节且无流控**：桥必须分块限速发送（`send_flow_controlled`，
  8B/60ms），设备侧必须做**跨轮次帧聚合**（遇 `\n` 才成帧）——否则长帧被截断，
  invoke 永远解析不出工具名。
- **QEMU aarch64 virt 默认 CPU 是 cortex-a15（AArch32）**，无法执行 AArch64 ELF
  （`cpu_reset` 显示 `usr26`）。必须显式 `-cpu cortex-a72`；且 QEMU 6.2 的 aarch64
  不支持 `-bios none`（报 `Could not find ROM image 'none'`）。
- **M 档设备模式**（`fabric_node` feature）关 S 中断、串口轮询，内核作为专用固件
  运行；用独立 `CARGO_TARGET_DIR` 构建，默认构建与 182 测试完全不受影响。
