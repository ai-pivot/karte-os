# MCU 档（S 档）预研 — riscv32 裁剪可行性报告（P3.5 定稿）

> ROADMAP Phase 3 · P3.5 验收物。骨架已真启动：
> `scratch32/`（riscv32imc, QEMU riscv32 virt M-mode, 5.5KB ELF）→
> `[karte32] hello from KarteOS S-tier!`（2026-10-08 实测）。

## 1. 结论

**可行，路径明确。** ESP32-C3 = RV32IMC（无 A 扩展）+ QEMU 6.2 自带
`qemu-system-riscv32 -M virt`（与 RV64 virt 同外设布局：16550 UART
0x10000000、CLINT 0x02000000），同构 ISA 使 S 档与 M 档共享 90% 代码。

## 2. 内核体积拆解（当前 riscv64 release ≈ 2.3MB）

| 子系统 | 大小估计 | S 档策略 |
|--------|---------|---------|
| AI 栈（M1 推理/分词/采样） | ~700KB | **feature `ai` 整体剔除** |
| MCP 织物（capability/drt/mcp_cb/capauth/brain） | ~120KB | **保留精简版（`fabric`）：CapDesc+DRT 核** |
| smoltcp + VirtIO | ~200KB | 保留 TCP/UDP（`net`），去 ICMP/DNS 调试 |
| ext4 + FAT32 + VFS | ~260KB | **RamFS only（`fs=ramfs`）** |
| Linux 兼容层 | ~30KB | 剔除 |
| trap/调度/PMM/页表 | ~150KB | Sv32 单进程 + 8 任务静态槽 |
| 双架构样板 + 复制页表 | ~200KB | riscv32 单架构 |

**S 档净内核目标 ≈ 700KB-1MB（RV32 代码密度更高，实测后修正）**；
超 256KB 的部分是 ext4/smoltcp——**ESP32-C3 256KB RAM 约束下 RamFS+精简
net 是现实路径**；256KB flash 目标需 LTO+opt-level=s+`--gc-sections`
（scratch32 实测 5.5KB hello 的裁剪密度可信）。

## 3. 关键技术差异（RV64 → RV32IMC）

1. **无 A 扩展**：`core::sync::atomic` 的 LR/SC 原语不可用 →
   单 hart 用中断屏蔽锁（csrci/csrcsi，IntSpinLock 形态）；ESP32-C3
   双核（LP+HP）下 HP 核单 hart 运行 OS。
2. **Sv32**：页表 2 级、PTE 格式不同（PNN1[20:10]）、satp MODE=1。
3. **无 OpenSBI**：`-bios none` M-mode 直起（MCU 档的正确形态），
   自管 mtime/mtimecmp、PLIC 或 CLINT 直连。
4. **compressed 指令**：C 扩展 trap skip 已有 16/32 位判断逻辑可复用。
5. **ESP32-C3 外设映射**（真机/Renode）：UART0 0x60000000、GPIO
   0x60004000、WDT 0x60008000——与 QEMU virt 的 16550 布局不同，
   SoC 层用 `soc = "esp32c3" | "qemu-virt"` cfg 区分（HAL薄层）。

## 4. Feature 矩阵（S 档）

```toml
[features]
s-tier = []           # 顶层裁剪开关
s-tier = ["fabric", "net-min", "fs-ramfs"]   # ~700KB-1MB
s-tier-min = []       # fabric 也去：纯 RTOS 形态 ~200KB
```

每个子系统在 `docs/agent/` 声明档位归属（ROADMAP §2.3 约定）。

## 5. 虚拟 ESP32 真测路径（Phase 最终锚点）

1. **已完成**：RV32IMC 工具链 + M-mode 启动 + UART + wfi（scratch32）
2. **下一步**：Sv32 单任务 + ticker + GPIO 影子设备 → CapDesc 织物精简
   版入核（`s-tier` feature）
3. **真机形态**：Renode（esp32c3 机器，含真外设模型）或 QEMU 8+ 的
   esp32c3 分支；QEMU 6.2 无 esp32c3 机器，用 virt 机 ISA 等价验证 +
   外设映射文档（已如实记录差异）
4. **验收**：虚拟设备上 MQTT 心跳（P3.3 产物）+ CapDesc 工具注册 +
   脑端（host 侧）调用 —— 三件套即"ESP32 真测"

## 6. 风险

- smoltcp 在 no-A 原子上的编译（`AtomicU32` 编译为 libcall，性能可接受）
- ext4 完全剔除后 OTA（P3.3）需要自定义分区表格式（A/B 两个 RamFS 镜像）
