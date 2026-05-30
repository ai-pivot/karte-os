# 计划：KarteOS x86_64 架构支持

> 生成时间：2025-07-28
> 状态：待确认

## 背景与目标

**核心目标**：为 KarteOS 添加 x86_64 架构支持，通过 feature flag 实现 RISC-V 64 和 x86_64 双架构共存。

**最终状态**：`make ARCH=x86_64 run` 能在 QEMU x86_64 上启动内核、加载 shell、执行用户程序。

**策略**：先重构引入 `arch/` 抽象层，将 RISC-V 代码移入 `arch/riscv64/`，再逐步实现 `arch/x86_64/`。

---

## 现状分析

### 代码库规模

| 类别 | 文件数 | 行数 | 架构耦合度 |
|------|--------|------|-----------|
| 架构相关（需重写） | 8 | ~1050 | 🔴 高 |
| 需局部修改 | 7 | ~600 | 🟡 中 |
| 可直接复用 | 15+ | ~7800 | 🟢 低 |

### 关键文件

| 文件 | 职责 | 修改类型 |
|------|------|----------|
| `kernel/src/entry.S` | RISC-V 汇编入口（29行） | 新增 x86_64 入口 |
| `kernel/src/arch/trap_entry.S` | RISC-V trap 汇编（257行） | 新增 x86_64 IDT/ISR |
| `kernel/src/arch/trap.rs` | Trap 处理 + TrapContext（299行） | 新增 x86_64 实现 |
| `kernel/src/sched/switch.S` | 上下文切换（46行） | 新增 x86_64 版本 |
| `kernel/src/arch/smp.rs` | SMP 多核（237行） | 新增 x86_64 AP 启动 |
| `kernel/src/arch/plic.rs` | 中断控制器（87行） | 新增 LAPIC/IOAPIC |
| `kernel/src/sbi.rs` | SBI 接口（73行） | 新增 x86_64 平台接口 |
| `kernel/src/mm/vmm.rs` | Sv39 页表（377行） | 需抽象为 trait |
| `kernel/src/mm/pmm.rs` | 物理内存（280行） | 改平台常量 |
| `kernel/src/mm/heap.rs` | 堆分配（89行） | 零修改 |
| `kernel/src/main.rs` | kmain 入口（177行） | 条件编译 |
| `kernel/src/driver/uart.rs` | UART 驱动（98行） | 新增 COM1 驱动 |
| `kernel/src/driver/virtio.rs` | VirtIO MMIO（~200行） | 新增 PCI 版本 |
| `kernel/src/driver/tty.rs` | TTY 子系统（~270行） | 参数化 UART 地址 |
| `kernel/src/sync/int_spinlock.rs` | 中断安全锁（124行） | 改为 x86 pushf/cli/sti |
| `kernel/src/process/elf.rs` | ELF 加载器（~170行） | 改 e_machine 校验 |
| `kernel/src/process/mod.rs` | 进程管理（~200行） | 参数化 MMIO 地址 |
| `kernel/src/syscall/mod.rs` | 系统调用（1522行） | 替换 sfence/satp |
| `kernel/src/lang_items.rs` | panic handler（7行） | 条件编译 shutdown |
| `kernel/Cargo.toml` | 依赖声明 | 条件依赖 |
| `Makefile` | 构建脚本 | 参数化 ARCH |
| `kernel/memory.x` | 链接脚本 | 新增 x86_64 版本 |

### 依赖关系

```
                    ┌─────────────────────────────────────────────┐
                    │              kmain (main.rs)                 │
                    └──┬──────┬──────┬──────┬──────┬──────┬───────┘
                       │      │      │      │      │      │
            ┌──────────┘      │      │      │      │      └──────────┐
            ▼                 ▼      ▼      ▼      ▼                 ▼
     arch::trap          mm::vmm  sched  driver  env     syscall
     arch::smp           mm::pmm    │      │                    │
     arch::plic          mm::heap   │      │                    │
            │                 │     │      │                    │
            ▼                 ▼     ▼      ▼                    ▼
     ┌──────────────────────────────────────────────────────┐
     │           硬件抽象层 (arch trait 接口)                 │
     │  TrapContext | ContextSwitch | PageTable | Interrupt  │
     │  Console | Timer | Shutdown | SMP                     │
     └──────────────────────────────────────────────────────┘
            │                                    │
     ┌──────┴──────┐                    ┌───────┴──────┐
     │  riscv64/   │                    │   x86_64/    │
     │  trap.S     │                    │   boot.S     │
     │  trap.rs    │                    │   gdt.rs     │
     │  smp.rs     │                    │   idt.rs     │
     │  plic.rs    │                    │   trap.rs    │
     │  switch.S   │                    │   paging.rs  │
     │  sbi.rs     │                    │   lapic.rs   │
     │  vmm_impl   │                    │   switch.S   │
     └─────────────┘                    │   uart.rs    │
                                        └──────────────┘
```

### 风险点

1. **TrapContext 三文件耦合**：trap_entry.S / trap.rs / sched/mod.rs 的偏移量必须严格一致，x86_64 版本需要同等严格的同步
2. **VMM 重写量大**：Sv39 → x86_64 4级页表，PTE 格式完全不同，~400 行核心代码
3. **ext4/fat32 直接调用 virtio::read_block**：绕过了 BlockDevice trait，需要先解耦
4. **satp 格式硬编码**：`(8 << 60) | ppn` 散布在 5+ 处，需要统一抽象
5. **first_enter_user 原子性**：RISC-V 用单个 asm! 块写 6 个 CSR，x86_64 的等价操作需要确保原子性
6. **构建系统深度耦合**：Makefile、CI、linker script 全是 RISC-V 特定的

---

## 详细计划

### 阶段零：架构抽象层重构（预估 ~800 行新增/修改）

**目标**：将 RISC-V 特定代码从通用代码中分离，建立 arch trait 接口。

#### 步骤 0.1：创建 arch 子目录结构

- [ ] 创建 `kernel/src/arch/riscv64/` 目录
- [ ] 移动 `arch/trap.rs` → `arch/riscv64/trap.rs`
- [ ] 移动 `arch/trap_entry.S` → `arch/riscv64/trap_entry.S`
- [ ] 移动 `arch/smp.rs` → `arch/riscv64/smp.rs`
- [ ] 移动 `arch/plic.rs` → `arch/riscv64/plic.rs`
- [ ] 移动 `arch/emergency_stack.rs` → `arch/riscv64/emergency_stack.rs`
- [ ] 移动 `entry.S` → `arch/riscv64/entry.S`
- [ ] 移动 `sched/switch.S` → `arch/riscv64/switch.S`
- [ ] 创建 `arch/riscv64/mod.rs` — 导出所有模块
- [ ] 更新 `arch/mod.rs` 为 `#[cfg(target_arch = "riscv64")] mod riscv64;`
- [ ] 更新所有 `use crate::arch::trap` 等引用路径
- [ ] **处理散布的 RISC-V 内联汇编**：
  - `sched/mod.rs` 的 `global_asm!(first_task_shim)` → 移入 `arch/riscv64/`
  - `sync/int_spinlock.rs` 的 `csrr/csrci/csrsi sstatus` → 提取为 `arch::irq_save()`/`arch::irq_restore()` 接口
  - `syscall/mod.rs` 的 5+ 处 `sfence.vma` → 提取为 `arch::flush_tlb()`/`arch::flush_tlb_addr()`
  - `syscall/mod.rs` 的 `satp` 计算 `(8 << 60) | ppn` → 统一为 `arch::make_satp(ppn)` 或使用 VMM trait
  - `driver/tty.rs` 的 `sie::set_sext()` → 提取为 `arch::enable_uart_irq()`
  - `main.rs` 的 `sstatus::clear_sie/set_sie` → 提取为 `arch::irq_disable()`/`arch::irq_enable()`
  - `main.rs` 的 `csrr satp` → 提取为 `arch::read_page_table()`
- 涉及文件：`kernel/src/arch/mod.rs`（重写）, `kernel/src/main.rs`（路径更新）, 所有引用 arch 的文件（sched/mod.rs, sync/int_spinlock.rs, syscall/mod.rs, driver/tty.rs）

#### 步骤 0.2：定义 arch 公共接口 trait

- [ ] 创建 `kernel/src/arch/mod.rs` 定义通用接口：

```rust
#[cfg(target_arch = "riscv64")]
mod riscv64;
#[cfg(target_arch = "x86_64")]  
mod x86_64;

// 统一导出
#[cfg(target_arch = "riscv64")]
pub use riscv64::*;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;
```

- [ ] 在 `arch/riscv64/mod.rs` 中导出所有现有符号（保持向后兼容）
- 涉及文件：`kernel/src/arch/mod.rs`（重写）

#### 步骤 0.3：提取 VMM 架构抽象

- [ ] 创建 `kernel/src/mm/vmm_arch.rs` 定义 MMU trait：

```rust
pub trait ArchMmu {
    const PAGE_TABLE_LEVELS: usize;
    const ENTRIES_PER_TABLE: usize;
    const PAGE_SIZE: usize;
    
    type PteFlags: Copy + Clone + core::fmt::Debug;
    type Pte: Copy + Clone;
    
    fn pte_new(ppn: usize, flags: Self::PteFlags) -> Self::Pte;
    fn pte_ppn(pte: Self::Pte) -> usize;
    fn pte_is_valid(pte: Self::Pte) -> bool;
    fn pte_is_leaf(pte: Self::Pte) -> bool;
    
    fn vpn_for_level(vaddr: usize, level: usize) -> usize;
    fn activate_page_table(root_ppn: usize);
    fn flush_tlb();
    fn flush_tlb_addr(addr: usize);
    
    fn flags_kernel_rw() -> Self::PteFlags;
    fn flags_kernel_rwx() -> Self::PteFlags;
    fn flags_user_rw() -> Self::PteFlags;
    fn flags_user_rwx() -> Self::PteFlags;
}
```

- [ ] 在 `arch/riscv64/` 中实现 `ArchMmu` for RISC-V Sv39
- [ ] 重构 `vmm.rs` 的 `map()`/`translate()`/`free_page_table()` 使用 trait 泛型
- 涉及文件：新增 `mm/vmm_arch.rs`，修改 `mm/vmm.rs`，新增 `arch/riscv64/mmu.rs`

#### 步骤 0.4：抽象平台常量

- [ ] 创建 `kernel/src/platform.rs` 统一平台常量：

```rust
#[cfg(target_arch = "riscv64")]
pub mod riscv64 {
    pub const UART_BASE: usize = 0x1000_0000;
    pub const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
    pub const PLIC_BASE: usize = 0x0C00_0000;
    pub const KERNEL_LOAD_ADDR: usize = 0x8020_0000;
    pub const MEMORY_SIZE: usize = 128 * 1024 * 1024;
    pub const USER_STACK_TOP: usize = 0x8000_0000;
    pub const USER_CODE_BASE: usize = 0x1000;
}

#[cfg(target_arch = "x86_64")]
pub mod x86_64 {
    pub const COM1_BASE: u16 = 0x3F8;
    pub const MEMORY_SIZE: usize = 128 * 1024 * 1024;
    pub const USER_STACK_TOP: usize = 0x8000_0000;
    pub const USER_CODE_BASE: usize = 0x1000;
    // PCI/APIC 地址由探测确定
}
```

- [ ] 替换代码中所有硬编码的 `0x1000_0000`、`0x8020_0000` 等为 `platform::` 常量
- 涉及文件：新增 `platform.rs`，修改 `sbi.rs`、`driver/uart.rs`、`driver/tty.rs`、`process/mod.rs`、`mm/pmm.rs`、`mm/vmm.rs`

#### 步骤 0.5：条件编译 RISC-V 依赖

- [ ] 修改 `kernel/Cargo.toml`：

```toml
[target.'cfg(target_arch = "riscv64")'.dependencies]
riscv = "0.16"
riscv-rt = "0.17"
sbi = "0.3"

[target.'cfg(target_arch = "x86_64")'.dependencies]
x86_64 = "0.15"
uart_16550 = "0.3"
```

- [ ] 修改 `kernel/src/main.rs` 中的 `global_asm!(entry.S)` 为条件编译
- [ ] 修改 `kernel/src/lang_items.rs` 中 shutdown 为条件编译
- [ ] 修改 `kernel/build.rs` 根据 target_arch 选择不同 linker script
- 涉及文件：`kernel/Cargo.toml`、`kernel/build.rs`、`main.rs`、`lang_items.rs`

#### 步骤 0.6：解耦 ext4/fat32 与 virtio 直接依赖

- [ ] 修改 `driver/ext4.rs` 的 `KarteBlockDevice` 改为通过 `block::get_block_device()` 获取
- [ ] 修改 `driver/fat32.rs` 的 `Fat32Storage` 同上
- [ ] 这一步确保文件系统层与底层块设备驱动解耦
- 涉及文件：`driver/ext4.rs`、`driver/fat32.rs`

#### 验证 0.X

- [ ] `cd user && make clean && make`
- [ ] `cargo fmt`
- [ ] `cargo build --release -p karte-os-kernel`（RISC-V 构建）
- [ ] `make test`（所有 70 测试通过）
- [ ] `make run`（shell 正常启动）

---

### 阶段一：x86_64 最小可启动（预估 ~600 行新增）

**目标**：x86_64 QEMU 上启动内核，通过 COM1 串口输出 "Hello from KarteOS (x86_64)!"。

#### 步骤 1.1：x86_64 构建基础设施

- [ ] 创建自定义 target JSON：`kernel/x86_64-karte-os.json`

```json
{
    "llvm-target": "x86_64-unknown-none",
    "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128",
    "arch": "x86_64",
    "target-endian": "little",
    "target-pointer-width": "64",
    "target-c-int-width": "32",
    "os": "none",
    "executables": true,
    "linker-flavor": "ld.lld",
    "linker": "rust-lld",
    "panic-strategy": "abort",
    "disable-redzone": true,
    "features": "-mmx,-sse,+soft-float",
    "pre-link-entries": {
        "gc-sections": false
    }
}
```

- [ ] 创建 `kernel/src/arch/x86_64/mod.rs` — 最小导出（stubs）
- [ ] 更新 `rust-toolchain.toml` 添加 x86_64 target
- [ ] 更新 `Makefile` 添加 `ARCH` 变量和 x86_64 构建规则
- 涉及文件：新增 `kernel/x86_64-karte-os.json`，修改 `rust-toolchain.toml`、`Makefile`

#### 步骤 1.2：Multiboot2 启动入口

- [ ] 创建 `kernel/src/arch/x86_64/boot.S`（~150 行）：

```
_multiboot2_header:
  .long 0xE85250D6          # magic
  .long 0                    # arch: i386 (protected mode)
  .long _multiboot2_header_end - _multiboot2_header  # length
  .long -(0xE85250D6 + 0 + length)  # checksum
  # frame buffer tag (optional)
  # end tag
_multiboot2_header_end:

_start:
  # 1. Multiboot2 将我们放在 32-bit protected mode
  # 2. 设置临时页表 (PML4 → PDPT → PD, 2MB 大页 identity map)
  # 3. 启用 PAE (CR4.PAE=1)
  # 4. 加载 PML4 到 CR3
  # 5. 启用长模式 (IA32_EFER.LME=1)
  # 6. 启用分页 (CR0.PG=1)
  # 7. 加载 64-bit GDT
  # 8. 远跳转到 64-bit 代码
  # 9. 清 BSS
  # 10. call kmain(ebx=multiboot2_info, 0)
  # 11. hlt
```

- [ ] 创建 `kernel/src/arch/x86_64/gdt.rs` — 全局描述符表
- [ ] 创建 x86_64 链接脚本 `kernel/memory-x86_64.ld`
- 涉及文件：新增 `arch/x86_64/boot.S`、`arch/x86_64/gdt.rs`、`kernel/memory-x86_64.ld`

#### 步骤 1.3：COM1 串口驱动

- [ ] 创建 `kernel/src/arch/x86_64/uart.rs`（~80 行）：

```rust
pub struct ComPort { port: u16 }

impl ComPort {
    pub const fn new(port: u16) -> Self { Self { port } }
    
    pub fn init(&self) {
        // 1. 禁用中断 (IER = 0x00)
        // 2. 设置波特率 115200 (DLAB + divisor)
        // 3. 8N1 (LCR = 0x03)
        // 4. 启用 FIFO (FCR = 0xC7)
        // 5. 启用接收中断 (IER = 0x01)
    }
    
    pub fn put_char(&self, c: u8) {
        while self.read_lsr() & 0x20 == 0 {} // 等待 TX 空
        self.write_thr(c);
    }
}

// I/O 端口操作: inb(port) / outb(port, val)
```

- [ ] 实现 `console_putchar`、`shutdown`（isa-debug-exit device @ port 0x501）
- [ ] 实现 `console_print!` / `console_println!` 宏的 x86_64 版本
- 涉及文件：新增 `arch/x86_64/uart.rs`，修改 `sbi.rs` 或创建 `arch/x86_64/platform.rs`

#### 步骤 1.4：最小 arch 接口实现

- [ ] 创建 `kernel/src/arch/x86_64/trap.rs` — stub 实现（仅 panic/loop）
- [ ] 创建 `kernel/src/arch/x86_64/smp.rs` — stub（单核）
- [ ] 修改 `kmain` 支持条件编译（跳过 RISC-V 特有的初始化阶段）
- 涉及文件：新增 `arch/x86_64/trap.rs`、`arch/x86_64/smp.rs`，修改 `main.rs`

#### 验证 1.X

- [ ] `make ARCH=x86_64 build` — 编译成功
- [ ] `make ARCH=x86_64 run` — QEMU 启动，串口输出 "Hello from KarteOS (x86_64)!"
- [ ] `make ARCH=riscv64 build` — RISC-V 构建不受影响
- [ ] `make test` — RISC-V 测试仍然全部通过

---

### 阶段二：x86_64 内存管理（预估 ~500 行新增）

**目标**：x86_64 4级页表工作，内核 identity map + 用户地址空间隔离。

#### 步骤 2.1：x86_64 4级页表实现

- [ ] 创建 `kernel/src/arch/x86_64/paging.rs`（~300 行）：

```rust
bitflags! {
    pub struct PteFlags: u64 {
        const PRESENT = 1 << 0;
        const WRITABLE = 1 << 1;
        const USER = 1 << 2;
        const WRITE_THROUGH = 1 << 3;
        const CACHE_DISABLE = 1 << 4;
        const ACCESSED = 1 << 5;
        const DIRTY = 1 << 6;
        const HUGE = 1 << 7;      // 2MB/1GB page
        const GLOBAL = 1 << 8;
        const NO_EXECUTE = 1 << 63;
    }
}
```

- [ ] 实现 `ArchMmu` trait for x86_64：
  - 4 级页表（PML4 → PDPT → PD → PT）
  - 每级 512 entries
  - PTE 格式：`PPN[51:12] | flags[63:0]`
  - `activate_page_table` → `mov cr3, rax`
  - `flush_tlb` → `invlpg` / CR3 reload
- [ ] 修改 PMM 的 `MEMORY_START` 为平台常量（x86_64 由 Multiboot2 memory map 确定）
- 涉及文件：新增 `arch/x86_64/paging.rs`，修改 `mm/vmm.rs`（使用 trait）、`mm/pmm.rs`（常量）

#### 步骤 2.2：内核页表初始化

- [ ] 实现 `vmm::init()` 的 x86_64 路径：
  - 从 Multiboot2 memory map 获取可用内存范围
  - 创建 PML4 identity map（使用 2MB 大页覆盖 128MB）
  - 映射 MMIO 设备（VGA、APIC 等）
  - 加载 CR3
- 涉及文件：`arch/x86_64/paging.rs`、`mm/vmm.rs`

#### 验证 2.X

- [ ] 内核 identity map 工作正常
- [ ] PMM 能分配/释放物理页
- [ ] Heap allocator 初始化成功
- [ ] 串口输出 + 动态内存分配（`console_println!` + `Vec::new()`）

---

### 阶段三：x86_64 中断与 Trap（预估 ~700 行新增）

**目标**：IDT 配置完成，能处理异常和硬件中断，syscall 可用。

#### 步骤 3.1：IDT（中断描述符表）

- [ ] 创建 `kernel/src/arch/x86_64/idt.rs`（~200 行）：
  - IDT 结构（256 entries）
  - InterruptStackFrame 结构（RIP, CS, RFLAGS, RSP, SS — CPU 自动压栈）
  - ISR stub 生成宏（为每个中断向量生成汇编入口）
  - 加载 IDT（`lidt` 指令）
- 涉及文件：新增 `arch/x86_64/idt.rs`

#### 步骤 3.2：TrapContext 和 Trap 处理

- [ ] 定义 x86_64 TrapContext：

```rust
#[repr(C)]
pub struct TrapContext {
    // CPU 自动压栈的部分（由 interrupt/exception 压入）
    // 已包含在 InterruptStackFrame 中
    // 我们需要在 ISR handler 中额外保存的：
    pub rax: usize, pub rbx: usize, pub rcx: usize, pub rdx: usize,
    pub rsi: usize, pub rdi: usize, pub r8: usize, pub r9: usize,
    pub r10: usize, pub r11: usize, pub r12: usize, pub r13: usize,
    pub r14: usize, pub r15: usize, pub rbp: usize,
    // InterruptStackFrame 已包含: rip, cs, rflags, rsp, ss (由 CPU 压栈)
}
```

- [ ] 创建 `kernel/src/arch/x86_64/isr.S` — ISR stub 汇编：
  - 保存所有通用寄存器到 TrapContext
  - 传递 InterruptStackFrame + TrapContext 给 Rust handler
  - 从 Rust handler 返回后恢复寄存器
  - `iretq` 返回
- [ ] 实现 `trap_handler` — 异常分发：
  - Page fault (#PF, vector 14) → 惰性页面分配
  - General protection (#GP, vector 13) → 进程杀死
  - Double fault (#DF, vector 8) → 紧急栈 + panic
- 涉及文件：新增 `arch/x86_64/trap.rs`（重写）、`arch/x86_64/isr.S`

#### 步骤 3.3：Syscall 接口

- [ ] 使用 `int 0x80` 软件中断（简单方案）或 `syscall/sysret`（高性能方案）
- [ ] x86_64 的 syscall 调用约定：
  - `rax` = syscall number（对应 RISC-V 的 `a7`）
  - `rdi, rsi, rdx, r10, r8, r9` = 参数（对应 `a0-a5`）
  - `rax` = 返回值
- [ ] `int 0x80` ISR → `trap_handler` → `syscall::dispatch()`
- 涉及文件：`arch/x86_64/trap.rs`、`arch/x86_64/isr.S`

#### 步骤 3.4：LAPIC 定时器

- [ ] 创建 `kernel/src/arch/x86_64/lapic.rs`（~150 行）：
  - LAPIC MMIO 基地址读取（从 MSR `IA32_APIC_BASE`）
  - 映射 LAPIC MMIO 到内核页表
  - 定时器初始化（divide=16, periodic mode）
  - `set_next_timer()` — 设置下次定时器中断
  - EOI（End of Interrupt）写入
- [ ] 创建 `kernel/src/arch/x86_64/ioapic.rs`（~100 行）：
  - IOAPIC 初始化
  - UART IRQ4 路由到 LAPIC
- 涉及文件：新增 `arch/x86_64/lapic.rs`、`arch/x86_64/ioapic.rs`

#### 步骤 3.5：上下文切换

- [ ] 创建 `kernel/src/arch/x86_64/switch.S`（~40 行）：

```asm
__switch:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    
    mov [rdi], rsp          ; *current_sp = rsp
    mov rsp, [rsi]          ; rsp = *next_sp
    
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
```

- [ ] 更新 `sched/task.rs` 的 TaskContext 定义为条件编译
- 涉及文件：新增 `arch/x86_64/switch.S`，修改 `sched/task.rs`

#### 验证 3.X

- [ ] 异常处理正常（page fault 能被捕获）
- [ ] 定时器中断触发调度
- [ ] Syscall 从用户态调用并返回
- [ ] 上下文切换在两个内核线程间工作

---

### 阶段四：x86_64 用户态（预估 ~400 行新增/修改）

**目标**：能加载 ELF 进入用户态执行，shell 运行正常。

#### 步骤 4.1：用户态进入/退出

- [ ] 实现 x86_64 的 `first_enter_user()`：
  - 设置 TSS.RSP0 = kernel_stack_top
  - 切换 CR3 到用户页表
  - 通过 `iretq` 进入 Ring 3（设置 CS=用户代码段, SS=用户数据段, RFLAGES.IF=1）
- [ ] TSS（Task State Segment）配置：
  - TSS 用于 Ring 3 → Ring 0 时自动切换栈（RSP0）
  - 等价于 RISC-V 的 sscratch 机制
- 涉及文件：`arch/x86_64/trap.rs`、`arch/x86_64/gdt.rs`

#### 步骤 4.2：ELF 加载器适配

- [ ] 修改 `process/elf.rs` 的 `EM_RISCV` 为条件编译
- [ ] x86_64 的用户地址空间布局：
  - 代码段：0x1000 起（与 RISC-V 相同）
  - 用户栈：0x8000_0000 向下增长
  - 用户堆：通过 brk 扩展
- [ ] ELF PT_LOAD 段权限映射：
  - RISC-V `PTEFlags::URWX` → x86_64 `PRESENT|USER|WRITABLE|!NO_EXECUTE`
- 涉及文件：`process/elf.rs`、`process/mod.rs`

#### 步骤 4.3：调度器适配

- [ ] 更新 `sched/mod.rs` 中的 `add_user_process` 为条件编译：
  - TrapContext 布局不同（x86_64 使用 InterruptStackFrame + 额外寄存器）
  - TaskContext 帧大小不同（x86_64 为 48 字节 vs RISC-V 104 字节）
  - 首次进入路径：x86_64 走 `iretq` 而非 `sret`
- 涉及文件：`sched/mod.rs`、`sched/task.rs`

#### 步骤 4.4：构建 x86_64 用户程序

- [ ] 创建 `user/user-x86_64.ld` 链接脚本
- [ ] 修改 `user/Makefile` 支持 `ARCH=x86_64` 编译
- [ ] 用户 C/Rust 程序的 syscall 调用约定适配（`int 0x80` + 寄存器映射）
- [ ] 修改 `user/syscall.rs` 为条件编译
- 涉及文件：新增 `user/user-x86_64.ld`，修改 `user/Makefile`、`user/syscall.rs`

#### 验证 4.X

- [ ] `make ARCH=x86_64 run` — shell 启动并响应命令
- [ ] `ls` 命令能列出文件
- [ ] 管道 `|` 和重定向 `>` 正常工作

---

### 阶段五：x86_64 块设备驱动（预估 ~500 行新增）

**目标**：x86_64 上能从磁盘读取 ext4/FAT32 文件。

#### 步骤 5.1：PCI 总线枚举

- [ ] 创建 `kernel/src/driver/pci.rs`（~200 行）：
  - PCI 配置空间访问（I/O 端口 0xCF8/0xCFC 或 ECAM MMIO）
  - 设备枚举（bus/device/function 遍历）
  - BAR（Base Address Register）解析
  - 识别 VirtIO 设备（vendor ID = 0x1AF4）
- 涉及文件：新增 `driver/pci.rs`

#### 步骤 5.2：VirtIO PCI 块设备

- [ ] 创建 `kernel/src/driver/virtio_pci.rs`（~200 行）：
  - 使用 `virtio-drivers` crate 的 PCI transport
  - 从 PCI BAR 获取 VirtIO 配置空间
  - 初始化 VirtQueue
  - 实现 `BlockDevice` trait
- 涉及文件：新增 `driver/virtio_pci.rs`

#### 步骤 5.3：x86_64 QEMU 配置

- [ ] 更新 Makefile x86_64 运行参数：

```makefile
qemu-system-x86_64 \
    -machine q35 \
    -cpu qemu64 \
    -m 128M \
    -kernel $(KERNEL_ELF) \
    -drive file=disk.img,format=raw,if=virtio \
    -serial stdio \
    -device isa-debug-exit,iobase=0x501,osize=2
```

- 涉及文件：`Makefile`

#### 验证 5.X

- [ ] ext4/FAT32 文件系统能正常挂载
- [ ] shell 中 `cat file` 能读取磁盘上的文件
- [ ] `mkdir`、`rm` 等文件操作正常

---

### 阶段六：x86_64 SMP 多核（预估 ~300 行新增）

**目标**：x86_64 上支持多核 CPU。

#### 步骤 6.1：ACPI 基础支持

- [ ] 创建 `kernel/src/arch/x86_64/acpi.rs`（~150 行）：
  - RSDP 定位（BIOS 区域扫描或 EFI 系统表）
  - RSDT/XSDT 解析
  - MADT（APIC 表）解析获取 CPU 核心信息
- 涉及文件：新增 `arch/x86_64/acpi.rs`

#### 步骤 6.2：AP（Application Processor）启动

- [ ] 创建 `kernel/src/arch/x86_64/smp.rs`（~200 行）：
  - BSP 通过 LAPIC ICR 发送 INIT IPI
  - 等待 10ms
  - 发送 SIPI（Startup IPI），目标地址 = AP trampoline 代码
  - AP 从 16-bit real mode 启动 → 转换到 64-bit long mode
  - AP 跳转到 `secondary_hart_entry()`（复用调度循环）
- 涉及文件：新增 `arch/x86_64/smp.rs`、`arch/x86_64/ap_trampoline.S`

#### 验证 6.X

- [ ] `make ARCH=x86_64 run` with `-smp 4` — 4 核全部启动
- [ ] 多进程在不同核心上运行

---

### 阶段七：CI 和文档（预估 ~200 行修改）

**目标**：x86_64 构建和测试集成到 CI。

#### 步骤 7.1：CI 配置

- [ ] 修改 `.github/workflows/ci.yml` 添加 x86_64 矩阵：
  - `build-x86_64` job
  - `test-x86_64` job（当测试框架适配后）
  - `boot-test-x86_64` job
- 涉及文件：`.github/workflows/ci.yml`

#### 步骤 7.2：文档更新

- [ ] 更新 `AGENTS.md` 反映双架构支持
- [ ] 更新 `docs/agent/` 下的知识文件
- [ ] 更新 `README.md`
- 涉及文件：`AGENTS.md`、`README.md`、`docs/agent/*.md`

---

## 验证方案

### 每阶段验证

| 阶段 | 验证手段 | 预期结果 |
|------|----------|----------|
| 阶段 0 | `make build && make test && make run` | RISC-V 完全不受影响，70 测试全过 |
| 阶段 1 | `make ARCH=x86_64 run` | QEMU 输出 "Hello from KarteOS (x86_64)!" |
| 阶段 2 | 串口输出 `Vec::new()` 测试 | 内存分配成功，无 page fault |
| 阶段 3 | 定时器中断 + 异常捕获 | 周期性打印 tick，page fault 被正确处理 |
| 阶段 4 | shell 交互 | shell 响应命令输入 |
| 阶段 5 | 文件操作 | ext4 文件读取成功 |
| 阶段 6 | SMP 启动 | 多核打印 hart ID |
| 阶段 7 | CI 全绿 | build + lint + test + boot-test 通过 |

### 回归验证

每个阶段完成后，必须确认 RISC-V 构建不受影响：
```bash
cd user && make clean && make
cargo fmt
cargo build --release -p karte-os-kernel
make test
make run
```

## 回滚策略

- 每个阶段作为独立 git commit / PR
- 如果某阶段出现问题，revert 到上一个阶段的 commit
- 阶段 0（抽象层重构）是最高风险阶段，因为移动了大量文件
  - 建议：先在 feature branch 上完成阶段 0，确认 RISC-V 完全正常后再合并

## 注意事项

1. **TrapContext 布局同步**：x86_64 的 TrapContext 布局在 `isr.S`、`trap.rs`、`sched/mod.rs` 三处必须一致
2. **CR3 切换时机**：x86_64 的 CR3 切换比 RISC-V satp 更昂贵（自动刷新全部 TLB），需要考虑使用 PCID 优化
3. **SMEP/SMAP**：x86_64 有 SMEP（Supervisor Mode Execution Prevention）和 SMAP（Supervisor Mode Access Prevention），类似 RISC-V 的 SUM 但更严格
4. **FPU/SSE 状态**：x86_64 需要在上下文切换时保存/恢复 FPU/SSE 状态（或用 `CR0.TS` 延迟保存）
5. **指令长度可变**：x86_64 指令 1-15 字节，`skip_trap_instruction` 不能简单加 4
6. **内核栈隔离**：x86_64 通过 TSS.RSP0 提供 per-CPU 内核栈，比 RISC-V 的 sscratch 方案更安全
7. **QEMU isa-debug-exit**：使用 port 0x501 实现关机，退出码 = `(value << 1) | 1`
8. **Multiboot2 信息**：bootloader 通过 EBX 寄存器传递 multiboot2 info 结构地址，包含内存 map、命令行等
9. **不要修改 `ext4_rs` vendored crate**：它是架构无关的，只需确保块设备接口正确
10. **Rust 2024 Edition**：保持 `#[unsafe(no_mangle)]`、`unsafe extern "C"` 等新模式

---

## 自审记录

### 审查清单

- [x] **目标一致性**：每一步都服务于"双架构共存，feature flag 切换"的目标
- [x] **步骤可执行性**：每步明确了涉及文件、具体操作和代码示例
- [x] **遗漏检查**：已补充散布在非 arch 文件中的 RISC-V 内联汇编处理（sfence.vma、sstatus、satp 等）
- [x] **依赖检查**：阶段 0→1→2→3→4→5→6→7 严格顺序，每步依赖前一步的输出
- [x] **文件准确性**：文件路径与探索结果一致
- [x] **风险评估**：识别了 6 大风险点 + 10 条注意事项
- [x] **计划自洽性**：阶段 0（重构）确保 RISC-V 不受影响，后续阶段逐步添加 x86_64

### 自审修正

1. 补充了步骤 0.1 中散布在 sched/mod.rs、sync/int_spinlock.rs、syscall/mod.rs、driver/tty.rs、main.rs 中的 RISC-V 内联汇编提取任务
2. 确认 x86_64 callee-saved 寄存器（rbx, rbp, r12-r15）= 6 个，switch 帧大小 48 字节

✅ 自审通过
