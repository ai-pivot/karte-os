# 计划：x86_64 真机支持 — VGA + PS/2 键盘 + AHCI/SATA 硬盘

> 生成时间：2026-05-30
> 状态：执行中

## 背景与目标

让 KarteOS x86_64 内核能在真实物理机上启动、显示画面、接受键盘输入、读写 SATA 硬盘。
三个核心驱动：VGA 文本模式、PS/2 键盘、AHCI/SATA。

## 现状分析

### 关键文件
| 文件 | 职责 | 修改类型 |
|------|------|----------|
| `kernel/src/driver/vga.rs` | VGA 文本模式驱动 | 新增 |
| `kernel/src/driver/keyboard.rs` | PS/2 键盘驱动 | 新增 |
| `kernel/src/driver/ahci.rs` | AHCI/SATA 驱动 | 新增 |
| `kernel/src/driver/mod.rs` | 驱动模块注册 | 修改 |
| `kernel/src/arch/x86_64/platform.rs` | console_putchar | 修改（加 VGA 输出）|
| `kernel/src/arch/x86_64/idt.rs` | 键盘中断 handler | 修改 |
| `kernel/src/arch/x86_64/pci.rs` | PCI 枚举 | 修改（加 AHCI 发现）|
| `kernel/src/driver/block.rs` | BlockDevice trait | 修改（加 AHCI 实现）|
| `kernel/src/main.rs` | 初始化流程 | 修改 |

### 依赖关系
```
VGA (独立) → platform.rs console_putchar → 所有输出都经过
Keyboard (独立) → idt.rs handler → tty::on_char → sys_read
AHCI → pci.rs 发现 → block.rs trait → ext4/fat32 挂载
```

## 详细计划

### 阶段一：VGA 文本模式驱动
- [x] 创建 `driver/vga.rs`：80×25 文本缓冲区管理、光标控制、滚屏
- [x] 修改 `platform.rs`：console_putchar 同时输出到 COM1 和 VGA

### 阶段二：PS/2 键盘驱动
- [x] 创建 `driver/keyboard.rs`：PS/2 Set 1 扫描码解析、Shift/Ctrl 状态机
- [x] 修改 `idt.rs`：keyboard_handler 解析扫描码并调用 tty::feed_byte

### 阶段三：AHCI/SATA 驱动
- [x] 创建 `driver/ahci.rs`：AHCI 控制器驱动、端口初始化、DMA 命令
- [x] 修改 `pci.rs`：添加 AHCI 设备发现（class 0x01, subclass 0x06）
- [x] 修改 `block.rs`：AHCI 实现 BlockDevice trait
- [x] 修改 `main.rs`：初始化 AHCI 并注册为块设备

### 阶段四：集成测试
- [ ] 构建 x86_64 ISO，在 QEMU 中验证 VGA+键盘+AHCI

### 阶段五：文档更新
- [ ] 更新 AGENTS.md

## 验证方案
- `make build`：编译无错误
- QEMU 启动：VGA 显示 shell 提示符、键盘可输入、硬盘可读写
- `make test`：RISC-V 测试不受影响

## 回滚策略
所有新增代码通过 `#[cfg(target_arch = "x86_64")]` 门控，RISC-V 构建不受影响。
