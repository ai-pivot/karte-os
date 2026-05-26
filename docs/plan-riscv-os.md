# 计划：KarteOS — 现代化 RISC-V 操作系统

> 生成时间：2026-05-26 13:12 CST
> 状态：执行中

## 背景与目标

构建一个优先支持 RISC-V 64-bit 的现代化个人操作系统，使用 Rust 实现。
- **目标平台**：RISC-V 64-bit (rv64gc)，QEMU `virt` 机器
- **核心特性**：S-mode 内核 + OpenSBI，Sv39 虚拟内存，抢占式调度，VirtIO 驱动
- **开发方式**：Multi-agent 协作

## 技术选型

| 组件 | 选择 | 原因 |
|------|------|------|
| Target | `riscv64gc-unknown-none-elf` | 完整 RV64GC 支持（含浮点） |
| SBI | OpenSBI (QEMU 默认) | 无需额外构建，直接使用 |
| 内存模型 | Sv39 三级页表 | RISC-V 标准，成熟稳定 |
| 调度器 | Round-Robin + Timer 抢占 | 简单可靠，易于理解 |
| 驱动模型 | MMIO + VirtIO | QEMU virt 标准设备接口 |
| Edition | Rust 2024 | 最现代的 Rust 特性 |

## 项目结构

```
karte-os/
├── Cargo.toml                    # Workspace root
├── .cargo/
│   └── config.toml               # Target, linker flags, runner
├── kernel/
│   ├── Cargo.toml
│   ├── memory.x                  # Linker script
│   ├── build.rs
│   └── src/
│       ├── main.rs               # #[entry] 入口
│       ├── lang_items.rs         # panic handler
│       ├── arch/
│       │   ├── mod.rs
│       │   └── trap.rs           # Trap 处理
│       ├── mm/
│       │   ├── mod.rs
│       │   ├── pmm.rs            # 物理内存管理
│       │   ├── vmm.rs            # 虚拟内存管理 (Sv39)
│       │   └── heap.rs           # 内核堆
│       ├── driver/
│       │   ├── mod.rs
│       │   └── uart.rs           # UART (ns16550a)
│       ├── sync/
│       │   ├── mod.rs
│       │   └── spinlock.rs
│       ├── sched/
│       │   ├── mod.rs
│       │   └── task.rs           # 任务管理
│       └── sbi.rs                # SBI 封装
├── Makefile
└── README.md
```

## 实施计划

### 阶段一：环境搭建（步骤 1-3）
- 安装 RISC-V target, QEMU, 交叉编译器
- 创建 Cargo workspace 和 kernel crate
- 配置 linker script, entry point, panic handler
- **验证**：`cargo build` 通过

### 阶段二：基础输出（步骤 4）
- 实现 UART 驱动（MMIO 0x10000000）
- SBI console 输出封装
- **验证**：QEMU 启动后看到 "Hello from KarteOS!" 

### 阶段三：Trap 框架（步骤 5）
- 实现 trap 上下文保存/恢复
- 异常分发器
- Timer 中断处理
- **验证**：Timer 中断正常触发

### 阶段四：内存管理（步骤 6-8）
- 物理页帧分配器（bitmap）
- Sv39 页表映射
- 内核堆分配器（buddy_system_allocator）
- **验证**：堆分配正常工作

### 阶段五：中断与调度（步骤 9-10）
- PLIC 驱动
- 任务控制块 + 上下文切换
- Round-Robin 调度 + Timer 抢占
- **验证**：多任务交替执行

### 阶段六：集成测试（步骤 11-12）
- Makefile 构建+运行脚本
- 端到端 QEMU 测试
- **验证**：`make run` 启动完整 OS

## 风险点

- **Edition 2024 `unsafe_op_in_unsafe_fn`**：所有 unsafe fn 中需要显式 `unsafe {}` 块
- **riscv-rt 兼容性**：确保使用的版本与 Rust 1.93.1 stable 兼容
- **QEMU 版本**：需要较新的 QEMU 版本支持 virt 机器

## 验证方案

1. `cargo build --release --target riscv64gc-unknown-none-elf` 编译通过
2. `make run` 在 QEMU 中看到启动信息
3. Timer 中断定期触发并打印信息
4. 堆分配测试通过
5. 多任务调度正常运行

✅ 自审通过
