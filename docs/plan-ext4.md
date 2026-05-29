# 计划：为 KarteOS 实现 ext4 文件系统支持

> 生成时间：2026-05-29
> 状态：待确认

## 背景与目标

### 为什么做
KarteOS 当前使用 FAT32（via starry-fatfs）作为持久化文件系统，FAT32 不支持文件权限、符号链接、大文件（>4GB）、日志等现代特性。ext4 是 Linux 标准文件系统，支持这些特性，实现 ext4 支持可以让 OS 更接近生产级。

### 目标
1. 在 KarteOS 中集成 `ext4_rs` crate（纯 Rust, `#![no_std]`，完整读写支持）
2. ext4 作为可挂载的文件系统，与现有 FAT32/RamFS 并存
3. Syscall 层能透明访问 ext4 上的文件（ls、cat、run 等命令正常工作）
4. 保持现有 FAT32/RamFS 功能不变，所有现有测试通过
5. 支持目录层级（subdirectory），这是 ext4 的核心优势

## 现状分析

### 关键文件
| 文件 | 职责 | 修改类型 |
|------|------|----------|
| `kernel/src/driver/vfs.rs` (482行) | VFS 抽象层（FileSystem trait、挂载表、OpenFileTable） | **修改** — 完善 VFS 层使其能承载 ext4 |
| `kernel/src/driver/fs.rs` (515行) | Legacy 编排器（RamFS + FAT32 统一入口 + FdTable） | **修改** — 增加到 VFS 层的桥接 |
| `kernel/src/driver/fat32.rs` (272行) | FAT32 实现（starry-fatfs） | **修改** — 包装为 FileSystem trait 实现 |
| `kernel/src/driver/block.rs` (66行) | 块设备抽象（BlockDevice trait + VirtIOBlock） | **修改** — 适配 ext4_rs 的 BlockDevice trait |
| `kernel/src/driver/virtio.rs` (225行) | VirtIO 块设备驱动 | 不变 |
| `kernel/src/driver/ext4.rs` | **新建** — ext4 文件系统驱动 | **新建** |
| `kernel/src/driver/mod.rs` | 驱动模块入口 | **修改** — 注册 ext4 模块 |
| `kernel/src/syscall/mod.rs` | 系统调用分发 | **修改** — 接入 VFS 路径 |
| `kernel/Cargo.toml` | 内核依赖 | **修改** — 添加 ext4_rs 依赖 |
| `kernel/src/main.rs` | 内核入口 | **修改** — 添加 ext4 初始化步骤 |
| `tools/mkdisk.sh` | 磁盘镜像工具 | **修改** — 支持 ext4 格式化 |
| `user/shell.rs` | 用户态 shell | **修改** — 支持目录导航（cd/ls 等） |

### 技术选型：ext4_rs crate

**选择**: [`ext4_rs`](https://crates.io/crates/ext4_rs) v1.3.3

理由：
- `#![no_std]` + `extern crate alloc` — 与 KarteOS 内核完全兼容
- 仅依赖 `bitflags` 和 `log` — 极轻量
- OS 无关设计：只需实现 `BlockDevice` trait（`read_offset` / `write_offset`）
- 功能完整：mount, open, close, read, write, mkdir, lsdir, unlink, truncate, remove
- 纯 Rust 实现，无 C FFI 依赖
- 支持创建文件系统（mkfs 等价）

注意事项：
- 需要 `#![feature(error_in_core)]`（nightly feature），KarteOS 已使用 Rust 2024 Edition，应已支持
- `BlockDevice` trait 接口：`read_offset(offset: usize) -> Vec<u8>` 和 `write_offset(offset: usize, data: &[u8])`
- 使用 `Arc<dyn BlockDevice>` 需要内核支持 `Arc`

### 架构决策

**核心决策：统一到 VFS 层**

当前系统有双轨架构：
- **Legacy 路径**（实际在用）：`syscall → fs.rs → fat32/ramfs`
- **VFS 路径**（已实现未接入）：`vfs.rs FileSystem trait → ramfs.rs`

计划将 ext4 实现为 VFS `FileSystem` trait，同时把 syscall 层切换到 VFS 路径。这是正确的架构方向：
- ext4 是 inode-based 文件系统，天然适配 VFS 的 inode 模型
- 目录层级需要路径解析，VFS 已有 `walk_path` 实现
- 避免在 legacy `fs.rs` 中继续堆叠条件分支

### 依赖关系

```
用户 syscall (open/read/write/exec/ls)
    │
    ▼
VFS 层 (vfs.rs)  ← 统一入口
    │
    ├── mount "/" → RamFS (ramfs.rs)      ← 内核嵌入文件
    ├── mount "/disk" → Ext4FS (ext4.rs)  ← VirtIO 块设备
    └── mount "/fat32" → Fat32FS (fat32.rs wrapper) ← 可选
    │
    ▼
BlockDevice 抽象 (block.rs)
    │
    ▼
VirtIO 块设备 (virtio.rs) → QEMU VirtIO MMIO
```

### 风险点

1. **ext4_rs 的 nightly feature 依赖** — `error_in_core` feature 可能与 stable Rust 2024 不兼容。如果不行，需要 patch 或使用其他方案。
2. **内存压力** — ext4_rs 需要读取超级块、块组描述符等元数据，当前 1MB 内核堆可能紧张。
3. **Arc 依赖** — ext4_rs 使用 `Arc<dyn BlockDevice>`，内核需要支持 `alloc::sync::Arc`。
4. **SpinLock 嵌套死锁** — VFS 层全局 SpinLock + ext4_rs 内部状态，如果 ext4_rs 操作中再获取 VFS 锁会死锁。
5. **log crate 兼容** — ext4_rs 依赖 `log` crate，需要提供 `no_std` 的 log 实现。

## 详细计划

### 阶段一：前置准备（基础设施）

#### 1.1 验证 ext4_rs crate 兼容性
- [ ] 在 `kernel/Cargo.toml` 中添加 `ext4_rs = "1.3.3"` 依赖
- [ ] 确认 `#![feature(error_in_core)]` 在当前 Rust 工具链下可用
- [ ] 确认 `alloc::sync::Arc` 可用（`spin::Arc` 或 `alloc::sync::Arc`）
- [ ] 为 `log` crate 提供 `no_std` 兼容层（空实现或转发到 console_println）
- [ ] 尝试 `cargo build --release -p karte-os-kernel` 确认编译通过

#### 1.2 扩展块设备抽象
- [ ] 在 `kernel/src/driver/block.rs` 中为 `BlockDevice` 添加多块读写方法：
  ```rust
  fn read_blocks(&self, start_block: usize, count: usize, buf: &mut [u8]) -> Result<(), VfsError>;
  fn write_blocks(&self, start_block: usize, count: usize, buf: &[u8]) -> Result<(), VfsError>;
  ```
- [ ] 为 `VirtIOBlock` 实现多块读写（调用 VirtIO 的 `read_blocks`/`write_blocks`）
- [ ] 统一 `block::VfsError` 和 `vfs::VfsError`（删除 block.rs 中的重复定义，统一用 vfs.rs 的）

#### 1.3 扩大内核堆
- [ ] 在 `kernel/src/mm/heap.rs` 中将 `HEAP_PAGES` 从 256 (1MB) 增至 1024 (4MB) 或更大
- [ ] 验证内核构建和测试通过

#### 1.4 PMM 连续帧分配（可选但推荐）
- [ ] 在 `kernel/src/mm/pmm.rs` 中添加 `alloc_contiguous_frames(count: usize) -> Option<usize>`
- [ ] 修复 VirtIO DMA 分配使用连续帧分配
- [ ] 验证 FAT32 仍正常工作

### 阶段二：Ext4 驱动核心实现

#### 2.1 创建 ext4 块设备适配器
- [ ] 新建 `kernel/src/driver/ext4.rs`
- [ ] 实现 `ext4_rs::BlockDevice` trait 的内核适配器：
  ```rust
  pub struct KarteBlockDevice;
  
  impl ext4_rs::BlockDevice for KarteBlockDevice {
      fn read_offset(&self, offset: usize) -> Vec<u8> {
          // 将字节偏移转为扇区号，调用 VirtIO 读取
      }
      fn write_offset(&self, offset: usize, data: &[u8]) {
          // 将字节偏移转为扇区号，调用 VirtIO 写入
      }
  }
  ```

#### 2.2 ext4 初始化和挂载
- [ ] 在 `ext4.rs` 中实现 `pub fn init() -> Result<(), &'static str>`：
  - 创建 `KarteBlockDevice` 实例
  - 调用 `ext4_rs::Ext4::open(disk)` 尝试打开 ext4 文件系统
  - 如果成功，将 ext4 包装为 VFS `FileSystem` trait 实现并挂载
  - 如果失败（非 ext4 分区），打印信息并跳过
- [ ] 在 `kernel/src/main.rs` 的初始化流程中调用 `driver::ext4::init()`

#### 2.3 实现 VFS FileSystem trait for ext4
- [ ] 创建 `Ext4FileSystem` struct 包装 `ext4_rs::Ext4` 实例
- [ ] 实现 `FileSystem` trait 的所有方法：
  - `name()` → "ext4"
  - `root_inode()` → ext4 根目录 inode 号（2）
  - `lookup(dir, name)` → 在目录中查找子项
  - `metadata(inode)` → 获取文件元数据（大小、类型）
  - `readdir(dir, idx)` → 列出目录项
  - `read_file(inode, offset, buf)` → 读取文件数据
  - `write_file(inode, offset, data)` → 写入文件数据
  - `create_file(dir, name)` → 创建新文件
  - `create_dir(dir, name)` → 创建目录
  - `unlink(dir, name)` → 删除文件/目录
  - `set_file_size(inode, size)` → 截断/扩展文件

#### 2.4 ext4_rs 日志和错误处理
- [ ] 实现 `log::Log` trait 的 no_st 空适配器，或转发到 `console_println!`
- [ ] 确保 ext4 错误能正确转换为 `VfsError`

### 阶段三：VFS 统一接入

#### 3.1 FAT32 适配为 FileSystem trait
- [ ] 在 `fat32.rs` 中创建 `Fat32FileSystem` struct，实现 VFS `FileSystem` trait
- [ ] 当前 FAT32 只操作根目录，inode 可用 cluster 号表示
- [ ] `root_inode()` → 0（根目录）
- [ ] `lookup(dir, name)` → 在根目录中搜索文件
- [ ] `readdir(dir, idx)` → 返回根目录文件列表
- [ ] `read_file(inode, offset, buf)` → 按文件名读取（inode 映射到文件名）

#### 3.2 修改初始化流程
- [ ] 在 `fs.rs::init()` 中：
  1. 初始化 RamFS 并 `vfs::mount("/", Box::new(ramfs_instance))`
  2. 初始化 FAT32 并 `vfs::mount("/fat32", Box::new(fat32_instance))` （可选）
  3. 初始化 ext4 并 `vfs::mount("/disk", Box::new(ext4_instance))`
  4. 保持嵌入式 ELF 文件的注入逻辑
- [ ] 确保启动顺序：VirtIO probe → RamFS init → FAT32 init → ext4 init

#### 3.3 Syscall 切换到 VFS
- [ ] 修改 `sys_open(path, path_len, flags)`:
  - 路径前缀 `/disk/` → VFS 查找 ext4 挂载点
  - 路径前缀 `/` → VFS 查找 RamFS 挂载点
  - 或者更简单：统一走 `vfs::open(path, flags)`，由 VFS 自动路由
- [ ] 修改 `sys_read(fd, buf, len)` → `vfs::read(fd, buf)`
- [ ] 修改 `sys_write(fd, buf, len)` → `vfs::write(fd, buf)`
- [ ] 修改 `sys_close(fd)` → `vfs::close(fd)`
- [ ] 修改 `sys_exec(path, path_len)` → 使用 VFS 路径解析
- [ ] 修改 `sys_ls(buf, len)` → `vfs::ls(path, buf, buf_len)`
- [ ] 保持 fd 0/1/2 的特殊处理（stdin/stdout/stderr 不变）

#### 3.4 文件描述符迁移
- [ ] 将 `FdTable`（fs.rs）迁移为 VFS 的 `OpenFileTable`（vfs.rs）
- [ ] `FileDescriptor.name: String` → `OpenFile.inode: u64 + mount_id: usize`
- [ ] 进程的 `fd_table` 字段类型从 `Option<FdTable>` 改为使用 VFS 全局 `OpenFileTable`（或进程级）

### 阶段四：工具链和测试

#### 4.1 磁盘镜像工具
- [ ] 修改 `tools/mkdisk.sh` 支持 ext4：
  - `tools/mkdisk.sh init-ext4` — 创建 ext4 格式的磁盘镜像
  - 或在现有 `init` 命令中添加选项
  - 使用 `mkfs.ext4 -b 4096` 格式化
- [ ] 确保主机可以挂载 ext4 镜像并放入文件
- [ ] 验证 `tools/mkdisk.sh put <file>` 在 ext4 镜像上工作

#### 4.2 Shell 命令扩展
- [ ] 添加 `cd <dir>` 命令支持目录切换
- [ ] 修改 `ls` 命令支持列出当前目录
- [ ] 修改 `cat` 和 `run` 支持路径
- [ ] 可选：添加 `mkdir <dir>`, `touch <file>` 命令

#### 4.3 集成测试
- [ ] 在 `kernel/src/driver/ext4.rs` 中添加 `#[cfg(feature = "test_mode")] run_tests()`
- [ ] 测试项：
  - ext4 超级块解析
  - 根目录列举
  - 文件读取
  - 文件写入
  - 目录创建和列举
  - 路径解析（多级目录）
- [ ] 更新 `kernel/src/test.rs` 注册 ext4 测试
- [ ] 更新 AGENTS.md 中的测试计数
- [ ] 验证 `make test` 全部通过
- [ ] 验证 `make boot-test` 启动到 Shell

#### 4.4 Boot 测试
- [ ] 创建 ext4 格式磁盘镜像
- [ ] 在其中放入 hello.elf 等文件
- [ ] 验证 `run /disk/hello` 能加载执行 ext4 上的程序
- [ ] 验证 `ls` 能看到 ext4 上的文件

## 验证方案

1. **编译通过**：`cargo build --release -p karte-os-kernel` 零错误
2. **现有测试通过**：`make test` 59 个测试全部通过
3. **Boot 测试通过**：`make boot-test` 看到 "KarteOS Shell"
4. **ext4 挂载**：启动日志中出现 `[ext4] ext4 filesystem mounted on /disk`
5. **文件读取**：在 shell 中 `ls` 能看到 ext4 根目录文件
6. **文件执行**：`run /disk/hello` 能加载执行 ext4 上的 ELF
7. **目录操作**：`ls /disk/subdir` 列出子目录内容
8. **文件写入**：`echo test > /disk/testfile` 写入成功，重启后数据仍在

## 回滚策略

- 所有修改通过 git 分支隔离，可随时回退
- ext4 初始化失败时自动降级到 FAT32/RamFS（不影响现有功能）
- VFS 统一路径如果出问题，可临时回退到 legacy `fs.rs` 路径

## 注意事项

- **Rust 2024 Edition**：`#[unsafe(no_mangle)]`、`unsafe extern "C"`、避免 `static mut`
- **SpinLock 死锁**：VFS 操作中不应嵌套获取其他 SpinLock
- **SMP 安全**：ext4 驱动需要考虑多核并发（至少用 SpinLock 保护）
- **QEMU 磁盘**：需要用 `mkfs.ext4 -b 4096` 格式化磁盘镜像，block size 必须是 4096
- **内存限制**：128MB 物理内存，内核堆至少需要 4MB 来支持 ext4 元数据缓存
- **error_in_core feature**：如果 stable Rust 不支持，需要在 lib.rs 顶部添加 `#![feature(error_in_core)]`

## 审查发现与修订

> 以下内容基于门下省审查报告，包含 6 个必须解决的问题和改进建议。

### 必须解决的问题

#### 问题 1：ext4_rs 的 nightly feature 阻塞编译
`ext4_rs` v1.3.3 在 `lib.rs:1` 和 `prelude.rs:2` 使用 `#![feature(error_in_core)]`。
该 feature 已在 Rust 1.81.0 稳定化，但 `#![feature]` 宏本身在 stable 上禁止使用。

**解决方案**：使用 `[patch.crates-io]` 指向去除此 feature gate 的 fork，或直接 vendor 源码。

#### 问题 2：VFS → ext4 I/O → VirtIO 嵌套 SpinLock 死锁
当前 `SpinLock` 不关中断，块 I/O 耗时较长，定时器中断可能触发死锁。

**解决方案**：改造 `SpinLock` 为中断安全版本（lock 时保存 `sstatus.SIE` 并禁用中断，unlock 时恢复）。

#### 问题 3：ext4_rs 写操作使用 `&self`，VFS trait 需要 `&mut self`
ext4_rs 的 `write_at(&self, ...)`, `create(&self, ...)` 等都是 `&self`，而 VFS trait 的 `write_file(&mut self, ...)` 需要 `&mut self`。

**解决方案**：`Ext4FileSystem` 使用 `spin::Mutex<Ext4>` 或 `UnsafeCell<Ext4>` 包装。

#### 问题 4：ext4 和 FAT32 共享 VirtIO 块设备
计划未明确是替换 FAT32 还是并存。

**决策**：**ext4 替换 FAT32 作为持久化文件系统**。磁盘镜像从 FAT32 格式改为 ext4 格式。FAT32 代码保留但不再默认挂载。RamFS 保持不变。

#### 问题 5：heap.rs 连续帧分配逻辑缺陷
`allocate_contiguous_pages` 只记录首帧地址，不验证物理连续性。

**解决方案**：使用 PMM bitmap 扫描连续空闲帧，修复此 bug。

#### 问题 6：ext4_rs `read_offset` 必须返回精确 4KB Vec
适配器实现需确保返回长度为 4096 字节的 Vec。

**解决方案**：在适配器中添加 `assert` 并正确实现 8×512B → 1×4KB 的拼装逻辑。

### 实施优先级建议

考虑到复杂性，建议按以下优先级分批实施：

**Gate 0（编译验证，必须先通过）**：
- vendor ext4_rs 源码，移除 nightly feature gate
- 在 kernel/Cargo.toml 中添加依赖
- `cargo build --release` 编译通过
- 提供 `log` crate 的 no_std 空实现

**第一批（MVP — 只读 ext4）**：
- 修复 SpinLock 为中断安全版本
- 扩大内核堆至 4MB
- 实现 ext4 BlockDevice 适配器
- ext4 只读路径（open, read, ls, metadata）
- 最小化的 VFS 接入（sys_exec/sys_open 读 ext4 文件）

**第二批（完整读写 + 替换 FAT32）**：
- ext4 写入支持（create, write, mkdir, unlink）
- 完整的 VFS 统一路径
- 替换 FAT32 为 ext4 作为默认持久化 FS
- 更新 mkdisk.sh 为 ext4 格式

**第三批（增强）**：
- Shell 目录导航命令（cd, ls 路径支持）
- 性能优化（块缓存）
- 更多测试
- 更新 AGENTS.md 和文档
