# KarteOS 安装指南

> 从零开始在真实 x86_64 硬件上安装 KarteOS

## 目录

1. [构建环境准备](#1-构建环境准备)
2. [编译 KarteOS](#2-编译-karteos)
3. [方法一：制作 USB 启动盘（推荐）](#3-方法一制作-usb-启动盘推荐)
4. [方法二：直接使用 ISO 镜像](#4-方法二直接使用-iso-镜像)
5. [从 USB 启动](#5-从-usb-启动)
6. [使用 KarteOS](#6-使用-karteos)
7. [在 QEMU 中挂载宿主机目录](#7-在-qemu-中挂载宿主机目录)
8. [硬件支持](#8-硬件支持)
9. [故障排除](#9-故障排除)

---

## 1. 构建环境准备

### 要求

- **操作系统**: Ubuntu 22.04+ 或其他 Linux 发行版
- **Rust**: stable (1.93+) + nightly 工具链
- **磁盘空间**: ~2GB（源码 + 编译产物）
- **USB 驱动器**: ≥128MB（用于启动盘）

### 安装依赖

```bash
# Rust 工具链
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
rustup target add riscv64gc-unknown-none-elf  # RISC-V（如果需要）

# 系统工具
sudo apt-get update
sudo apt-get install -y \
    qemu-system-x86_64 \
    grub-common \
    grub-pc-bin \
    xorriso \
    mtools \
    dosfstools \
    e2fsprogs \
    sfdisk \
    gcc-riscv64-linux-gnu  # RISC-V 用户程序编译

# 克隆代码
git clone <your-repo-url> karte-os
cd karte-os
```

---

## 2. 编译 KarteOS

### 编译 x86_64 内核（用于实机）

```bash
# 编译用户程序（shell, ls, cat 等）
cd user && make ARCH=x86_64 clean && make ARCH=x86_64
cd ..

# 编译内核 + 创建 GRUB ISO
make build-x86
```

成功后生成:
- **内核**: `target/x86_64-unknown-none/release/karte-os-kernel`
- **ISO**: `target/karte-os-x86_64.iso`

### 编译 RISC-V 内核（用于 QEMU 测试）

```bash
cd user && make clean && make
cd ..
make build
```

### 快速测试（QEMU）

```bash
# 在 QEMU 中运行 x86_64 版本
make run-x86

# 退出 QEMU: Ctrl+A 然后 X
```

---

## 3. 方法一：制作 USB 启动盘（推荐）

### 自动方式

```bash
# 创建 USB 镜像文件（默认 512MB）
make usb-image

# 查看生成的镜像
ls -lh target/karte-os-usb.img
```

然后写入 U 盘：

```bash
# 1. 插入 U 盘，查看设备名
lsblk
# 假设 U 盘是 /dev/sdb（注意不要选错！）

# 2. 写入镜像（会擦除 U 盘所有数据）
sudo dd if=target/karte-os-usb.img of=/dev/sdb bs=4M status=progress
sync
```

### 手动方式

如果自动脚本有问题，可以手动创建：

```bash
# 1. 创建 512MB 镜像
dd if=/dev/zero of=usb.img bs=1M count=512

# 2. 分区（GPT 或 MBR）
# 使用 fdisk:
#   n → 新建分区1 (64MB, FAT32, bootable)
#   n → 新建分区2 (剩余, ext4)
#   a → 设置分区1 为 bootable
#   w → 写入

# 3. 格式化
sudo mkfs.fat -F 32 /dev/sdX1
sudo mkfs.ext4 /dev/sdX2

# 4. 安装 GRUB
sudo mount /dev/sdX1 /mnt
sudo mkdir -p /mnt/boot/grub
sudo cp target/x86_64-unknown-none/release/karte-os-kernel /mnt/boot/karte-os-kernel
cat > /tmp/grub.cfg << 'EOF'
set timeout=3
set default=0
menuentry "KarteOS" {
    multiboot2 /boot/karte-os-kernel
    boot
}
EOF
sudo cp /tmp/grub.cfg /mnt/boot/grub/grub.cfg
sudo grub-install --target=i386-pc --boot-directory=/mnt/boot /dev/sdX
sudo umount /mnt

# 5. 复制用户程序到 ext4 分区
sudo mount /dev/sdX2 /mnt
sudo cp user/shell.elf /mnt/shell
sudo cp user/ls.elf /mnt/ls
sudo cp user/cat.elf /mnt/cat
# ... 其他程序
sudo mkdir -p /mnt/bin /mnt/etc /mnt/dev /mnt/tmp /mnt/home
sudo umount /mnt
```

---

## 4. 方法二：直接使用 ISO 镜像

如果你有光驱或者主板支持从 ISO 启动：

```bash
# 1. 编译
make build-x86

# 2. 写入 U 盘（ISO 模式，简单但只读）
sudo dd if=target/karte-os-x86_64.iso of=/dev/sdX bs=4M status=progress
sync
```

> **注意**: ISO 模式下，KarteOS 的文件系统是嵌入内核的 RamFS（只读）。
> 如需可写的 ext4 文件系统，请使用方法一。

---

## 5. 从 USB 启动

1. **插入 USB 启动盘**到目标电脑

2. **进入 BIOS/UEFI 设置**（通常开机时按 F2、F12、Del 或 Esc）

3. **调整启动顺序**:
   - 将 USB 设备设为第一启动项
   - 如果使用 UEFI 模式，确保启用 "Legacy Boot" 或 "CSM"
     （KarteOS 使用 GRUB 的 BIOS/MBR 模式）
   - 保存并退出

4. **启动 KarteOS**:
   - 看到 GRUB 菜单后选择 "KarteOS"
   - 内核会自动加载并启动 shell

5. **成功标志**: 屏幕上出现 `KarteOS Shell` 提示符

---

## 6. 使用 KarteOS

### 内建 Shell 命令

KarteOS 启动后进入交互式 shell，支持以下命令：

| 命令 | 说明 |
|------|------|
| `help` | 显示帮助信息 |
| `ls` | 列出当前目录文件 |
| `cat <file>` | 显示文件内容 |
| `echo <text>` | 输出文本 |
| `cd <dir>` | 切换目录 |
| `pwd` | 显示当前目录 |
| `mkdir <dir>` | 创建目录 |
| `rm <file>` | 删除文件 |
| `env` | 显示环境变量 |
| `export KEY=VAL` | 设置环境变量 |
| `kill <pid>` | 终止进程 |
| `exit` | 关机 |

### Shell 特性

- **管道**: `ls | cat`
- **输出重定向**: `echo hello > file.txt`
- **追加**: `echo world >> file.txt`
- **输入重定向**: `cat < file.txt`
- **命令历史**: ↑/↓ 方向键
- **Tab 补全**: 按 Tab 键自动补全

### 文件系统

KarteOS 支持 ext4、FAT32 和 RamFS：

- **ext4**: 首选文件系统，用于 SATA/NVMe 硬盘上的持久存储
- **FAT32**: 备选文件系统，兼容性好
- **RamFS**: 内存文件系统，内核内嵌的 ELF 程序

---

## 7. 在 QEMU 中挂载宿主机目录

使用 virtio-9p 协议可以在 QEMU 中将宿主机目录共享给 KarteOS：

```bash
# 方法 1: 使用 Makefile target
make run-9p HOST_DIR=/path/to/share

# 方法 2: 手动指定 QEMU 参数
make build-x86
qemu-system-x86_64 \
    -machine pc -cpu qemu64 -nographic -m 128M -smp 1 \
    -cdrom target/karte-os-x86_64.iso -no-reboot \
    -drive file=disk.img,format=raw,if=none,id=hd0 \
    -device virtio-blk-pci,drive=hd0 \
    -fsdev local,id=share1,path=/path/to/share,security_model=none \
    -device virtio-9p-pci,fsdev=share1,mount_tag=hostshare
```

> **注意**: virtio-9p 驱动框架已实现，完整的 VirtIO 传输层需要
> virtqueue 支持才能实际通信。当前 shell 的文件操作通过 ext4/FAT32
> 文件系统访问 QEMU 的 disk.img。

---

## 8. 硬件支持

### 块设备（存储）

| 设备类型 | 驱动 | 状态 | 说明 |
|---------|------|------|------|
| **NVMe SSD** | `nvme.rs` | ✅ 已实现 | PCIe NVMe 控制器，优先检测 |
| **SATA 硬盘/SSD** | `ahci.rs` | ✅ 已实现 | AHCI/SATA 控制器（PCI class 01/06/01） |
| **VirtIO Block** | `virtio.rs` | ✅ 已实现 | QEMU 虚拟块设备 |
| **USB 大容量存储** | — | ❌ 未实现 | 需要 xHCI 驱动 |

### 存储设备优先级

内核按以下顺序检测块设备：
1. **NVMe** — 如果检测到 NVMe 控制器（class 01/08/02），优先使用
2. **AHCI** — 如果没有 NVMe，检测 AHCI SATA 控制器
3. **VirtIO** — QEMU 环境中的虚拟块设备

### 文件系统

| 文件系统 | 状态 | 说明 |
|---------|------|------|
| **ext4** | ✅ 完整支持 | 读取、写入、创建文件和目录 |
| **FAT32** | ✅ 完整支持 | 长文件名支持 |
| **RamFS** | ✅ 完整支持 | 内核内嵌的 ELF 程序 |

### 其他硬件

| 设备 | 状态 |
|------|------|
| VGA 文本模式 (80×25) | ✅ |
| COM1 串口 | ✅ |
| PS/2 键盘 | ✅ |
| PS/2 鼠标 | ❌ |
| 网卡 (VirtIO Net) | ✅ RISC-V only |
| ACPI 关机 | ✅ 多种方式 |

---

## 9. 故障排除

### Q: 启动时黑屏，没有 GRUB 菜单

**A**: 检查以下几点：
1. USB 是否正确写入（用 `lsblk` 查看分区）
2. BIOS 中是否禁用了 Secure Boot
3. 尝试启用 Legacy Boot / CSM 模式
4. 确认 USB 启动优先级最高

### Q: GRUB 加载内核后重启

**A**: 可能原因：
1. 内核与 CPU 不兼容（需要 x86_64 CPU）
2. 内存不足（需要 ≥128MB RAM）
3. GRUB 配置错误

### Q: 没有检测到 SATA/NVMe 硬盘

**A**:
1. 检查 BIOS 中 SATA 模式是否为 AHCI（不是 RAID）
2. NVMe 需要主板支持 PCIe
3. 查看 boot log 中的 `[pci]` 和 `[nvme]`/`[ahci]` 信息
4. 某些主板可能需要额外的 PCI 配置

### Q: QEMU 中运行正常但实机不工作

**A**: QEMU 使用虚拟设备，实机使用真实硬件：
1. 确保 AHCI 模式已启用（不是 IDE 兼容模式）
2. 某些主板可能使用非标准 AHCI PCI 位置
3. 检查内核输出中的 PCI 设备列表

### Q: 文件系统挂载失败

**A**:
1. 确认磁盘已正确分区和格式化
2. ext4 需要是标准格式（4K 块大小）
3. FAT32 兼容性更好但功能有限

### Q: 键盘不工作

**A**:
1. KarteOS 目前仅支持 PS/2 键盘
2. USB 键盘需要 USB HCI 驱动（尚未实现）
3. 如果主板有 PS/2 接口，使用 PS/2 键盘
4. 某些 BIOS 可以设置 "USB keyboard legacy support"

### Q: 如何关机

**A**: 在 shell 中输入 `exit` 命令。内核会尝试：
1. QEMU 调试端口退出
2. ACPI 关机
3. 键盘控制器复位
4. 永久 HLT

在实机上，ACPI 关机通常有效。如果不行，长按电源键强制关机。

### Q: 如何开发调试

**A**: 使用 QEMU 串口调试：
```bash
# 带调试输出
make run-x86

# 带 GDB 调试
make debug-x86
# 另一个终端:
gdb target/x86_64-unknown-none/release/karte-os-kernel
(gdb) target remote :1234
```

---

## 架构概览

```
┌───────────────────────────────────────────────────────┐
│                    KarteOS Kernel                      │
│                                                        │
│  ┌──────────┐  ┌──────────┐  ┌────────────────────┐  │
│  │  Shell    │  │ 用户程序  │  │  系统调用接口       │  │
│  │ (v0.5)   │  │ ls/cat/..│  │  (ecall/int 0x80)  │  │
│  └──────────┘  └──────────┘  └────────────────────┘  │
│                                                        │
│  ┌─────────────────────────────────────────────────┐  │
│  │              VFS (虚拟文件系统)                    │  │
│  │  ┌──────┐  ┌──────┐  ┌──────┐  ┌────────────┐  │  │
│  │  │ ext4 │  │FAT32 │  │RamFS │  │  9p (QEMU) │  │  │
│  │  └──────┘  └──────┘  └──────┘  └────────────┘  │  │
│  └─────────────────────────────────────────────────┘  │
│                                                        │
│  ┌─────────────────────────────────────────────────┐  │
│  │              块设备层                              │  │
│  │  ┌──────┐  ┌──────┐  ┌────────────┐            │  │
│  │  │ NVMe │  │ AHCI │  │ VirtIO Blk │            │  │
│  │  └──────┘  └──────┘  └────────────┘            │  │
│  └─────────────────────────────────────────────────┘  │
│                                                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐   │
│  │ 内存管理  │  │ 调度器    │  │  x86_64 架构层   │   │
│  │ PMM/VMM  │  │ Round-   │  │  IDT/GDT/TSS/    │   │
│  │ Heap     │  │ Robin    │  │  LAPIC/IOAPIC    │   │
│  └──────────┘  └──────────┘  └──────────────────┘   │
│                                                        │
│  GRUB (Multiboot2) → Boot.S → kmain → Shell          │
└───────────────────────────────────────────────────────┘
```

---

## 快速参考

| 操作 | 命令 |
|------|------|
| 编译 x86_64 | `make build-x86` |
| 编译 RISC-V | `make build` |
| QEMU 运行 x86 | `make run-x86` |
| QEMU 运行 RISC-V | `make run` |
| 创建 USB 镜像 | `make usb-image` |
| 写入 USB | `sudo dd if=target/karte-os-usb.img of=/dev/sdX bs=4M` |
| 运行测试 | `make test` |
| 退出 QEMU | `Ctrl+A` 然后 `X` |
