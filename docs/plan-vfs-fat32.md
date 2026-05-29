# 计划：VFS + FAT32 持久化文件系统

> 生成时间：2026-05-29
> 状态：待确认

## 背景与目标

KarteOS 当前只有一个纯内存文件系统（`driver/fs.rs`），文件通过 `include_bytes!()` 编译时嵌入，重启丢失。目标是实现一个**真正的持久化文件系统**，使 OS 能与 Ubuntu 双向共享磁盘镜像上的文件，且架构为未来 ext4 预留扩展空间。

**期望最终状态**：
- shell 的 `ls`/`cat`/`run` 命令操作 FAT32 磁盘上的真实文件
- 宿主机可以通过 `mcopy`/`mount` 读写 disk.img
- `demo_karte` 等外部 ELF 可以从 FAT32 直接加载运行
- VFS 层设计让 ext2/ext4 可以作为插件式文件系统接入

## 现状分析

### 关键文件
| 文件 | 职责 | 修改类型 |
|------|------|----------|
| `kernel/src/driver/fs.rs` | in-memory 文件系统 + FdTable | 重构（保留 FdTable，FS 逻辑迁移到 VFS） |
| `kernel/src/driver/virtio.rs` | VirtIO 块设备驱动 | 修改（增加多扇区读写） |
| `kernel/src/driver/mod.rs` | driver 模块声明 | 修改 |
| `kernel/src/syscall/mod.rs` | 文件相关 syscall | 重构 |
| `kernel/src/main.rs` | 内核启动流程 | 修改 |
| `user/shell.rs` | shell 命令 | 修改 |
| `Makefile` | QEMU 启动 + 构建 | 修改 |
| `kernel/Cargo.toml` | 依赖 | 修改 |

### 新增文件
| 文件 | 职责 |
|------|------|
| `kernel/src/driver/vfs.rs` | VFS 抽象层（FileSystem trait、mount table、path resolution） |
| `kernel/src/driver/virtio_blk.rs` | VirtIO Block → fatfs IO trait 适配器 |
| `tools/mkdisk.sh` | 宿主机端磁盘管理脚本 |

### 依赖关系
```
Shell (用户态)
    ↓ ecall syscall
Syscall 分发 (syscall/mod.rs)
    ↓ 路径解析
VFS 层 (driver/vfs.rs)         ← 新增
    ↓ FileSystem trait
┌──────┬──────────┐
RamFS  FAT32      (ext4)
(内置) (starry-   (未来)
       fatfs)
    ↓ BlockDevice trait
VirtIO Block Driver (driver/virtio.rs)
    ↓ MMIO
QEMU virtio-blk-device
```

### 风险点
1. **starry-fatfs 是 preview 版**：可能有未发现的 bug。缓解：API 和 fatfs 原版几乎一致，可以快速切换。
2. **SpinLock 嵌套死锁**：VFS 操作可能同时持有 FS 锁和进程表锁。缓解：严格定义锁获取顺序。
3. **现有 59 个测试全部通过**：不能破坏。RamFS 保留给 test_mode 使用。
4. **RefCell 线程安全**：starry-fatfs 内部用 `RefCell<IO>`，单核安全但 SMP 需注意。当前单核，未来用 SpinLock 包装。

## 详细计划

### 设计决策

#### 1. VFS 层设计

FileSystem trait 是核心抽象，FAT32、RamFS、未来 ext4 都实现它：

```rust
pub type InodeNo = u64;

pub trait FileSystem: Send + Sync {
    fn name(&self) -> &str;
    fn root_inode(&self) -> InodeNo;
    fn lookup(&self, dir: InodeNo, name: &str) -> Result<InodeNo, VfsError>;
    fn metadata(&self, inode: InodeNo) -> Result<VfsMetadata, VfsError>;
    fn readdir(&self, dir: InodeNo, idx: usize) -> Result<Option<VfsDirEntry>, VfsError>;
    fn read(&self, inode: InodeNo, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError>;
    fn write(&mut self, inode: InodeNo, offset: usize, data: &[u8]) -> Result<usize, VfsError>;
    fn create(&mut self, dir: InodeNo, name: &str, kind: VfsFileType) -> Result<InodeNo, VfsError>;
    fn unlink(&mut self, dir: InodeNo, name: &str) -> Result<(), VfsError>;
    fn mkdir(&mut self, dir: InodeNo, name: &str) -> Result<InodeNo, VfsError>;
    fn truncate(&mut self, inode: InodeNo, size: usize) -> Result<(), VfsError>;
}
```

每个挂载的文件系统实例存储在 `MountTable` 中，支持路径前缀路由。

#### 2. RamFS (in-memory) 保留为 initramfs

- 保留现有的 `FileSystem` 数据结构，实现 `vfs::FileSystem` trait
- 存放 init/shell 等编译时嵌入的程序
- test_mode 下 RamFS 是唯一文件系统
- 路径前缀 `/builtin/` 或直接挂载到根（无 FAT32 时）

#### 3. FAT32 使用 starry-fatfs

- 实现 `fatfs::io::{Read, Write, Seek}` trait 的 VirtIO 适配器
- 启动时挂载到根目录 `/`
- RamFS 挂载到 `/builtin/`（或直接合并根目录）
- 内置程序（hello, shell）同时在 RamFS 和 FAT32 中可访问

#### 4. mount 架构

```
/          → FAT32 (disk.img)  ← 主文件系统，Ubuntu 互通
/builtin/  → RamFS (嵌入式)    ← init/shell/hello 等内置程序
```

或者更简单：RamFS 的文件在启动时 copy 到 FAT32 根目录（如果 FAT32 中不存在），这样只有一个根 `/`。

**选择**：启动时将 RamFS 的文件注入 FAT32（如果不存在），统一命名空间为 `/`。这样 shell 的 `ls` 能看到所有文件（内置+磁盘），`run hello` 既可以是内置的也可以是磁盘上的。

### Phase 1: VFS 抽象层

- [ ] 1.1：新建 `kernel/src/driver/vfs.rs`，定义 `FileSystem` trait、`VfsError`、`VfsFileType`、`VfsDirEntry`、`VfsMetadata`
- [ ] 1.2：实现 `MountTable`（最多 8 个挂载点），路径前缀匹配
- [ ] 1.3：实现路径解析（`resolve_path`：拆分路径组件、逐级 lookup）
- [ ] 1.4：实现全局 VFS 操作（`vfs_open`/`vfs_read`/`vfs_write`/`vfs_close`/`vfs_ls`/`vfs_stat`）
- [ ] 1.5：VFS 维护一个 `OpenFileTable`（全局文件描述符表），每进程 fd → 全局 ofd

### Phase 2: BlockDevice trait + VirtIO 适配器

- [ ] 2.1：在 `vfs.rs` 中定义 `BlockDevice` trait（`read_block`/`write_block`/`capacity`）
- [ ] 2.2：为现有 VirtIO 驱动实现 `BlockDevice` trait
- [ ] 2.3：增加多扇区批量读写 API（`read_blocks`/`write_blocks`）
- [ ] 2.4：实现简单的块缓存层（可选，提升性能）

### Phase 3: FAT32 集成

- [ ] 3.1：添加 `starry-fatfs` 依赖到 `kernel/Cargo.toml`
- [ ] 3.2：新建 `kernel/src/driver/fat32.rs`，实现 VirtIO → `fatfs::io` trait 适配器
- [ ] 3.3：实现 `FileSystem` trait 的 FAT32 包装器（FAT32 → VFS inode 映射）
- [ ] 3.4：启动时格式化空磁盘（首次）或挂载已有 FAT32 卷
- [ ] 3.5：将 RamFS 内置文件注入 FAT32（如果 FAT32 中不存在）

### Phase 4: Syscall 改造

- [ ] 4.1：`sys_open` 改为走 VFS（路径解析 → 找到挂载点 → 调用对应 FS 的 create/lookup）
- [ ] 4.2：`sys_read`/`sys_write` 改为通过 inode 号操作（而非文件名字符串）
- [ ] 4.3：`sys_ls` 改为通过 VFS 的 readdir 操作
- [ ] 4.4：`sys_spawn` 改为按路径名加载 ELF（不再硬编码 prog_id）
- [ ] 4.5：FD table 从 `{name, pos, flags}` 改为 `{inode, pos, flags, mount_id}`
- [ ] 4.6：test_mode 下保持使用 RamFS（不依赖块设备）

### Phase 5: Shell + Makefile + 磁盘管理

- [ ] 5.1：shell 的 `run` 命令改为 `run /demo_karte`（按路径名）
- [ ] 5.2：Makefile 中 `disk.img` 改为 64MB FAT32 格式
- [ ] 5.3：创建 `tools/mkdisk.sh` 管理脚本（格式化、复制文件、列出内容）
- [ ] 5.4：更新 CI（boot-test 检测字符串不变，但需要 FAT32 disk.img）
- [ ] 5.5：更新 AGENTS.md 文档

### Phase 6: 测试验证

- [ ] 6.1：59 个现有测试全部通过（RamFS 在 test_mode 下独立运行）
- [ ] 6.2：boot-test 通过（shell 从 FAT32 加载）
- [ ] 6.3：QEMU 中 `ls` 显示 FAT32 文件 + 内置文件
- [ ] 6.4：`cat test.txt` 读取 FAT32 文件
- [ ] 6.5：`run /demo_karte` 从 FAT32 加载并执行外部 ELF
- [ ] 6.6：宿主机 `mcopy` 写入文件 → OS 中可见
- [ ] 6.7：OS 中创建的文件 → 宿主机 `mcopy` 可读

## 验证方案

1. **单元测试**：RamFS 实现 FileSystem trait 的测试（现有 15 个 FS 测试迁移）
2. **集成测试**：test_mode 下全部 59 测试通过
3. **boot-test**：`make boot-test` 检测 "KarteOS Shell"
4. **交互测试**：QEMU TTY 测试 ls/cat/run 命令
5. **宿主机互通**：`mcopy -i disk.img demo_karte ::` → OS 中 `run /demo_karte`
6. **持久化验证**：OS 中写文件 → 重启 → 文件仍在

## 回滚策略

- VFS 是纯新增模块，不修改现有 fs.rs → 可以安全回退
- FAT32 集成通过 Cargo feature flag 控制
- test_mode 完全不依赖 FAT32 → 不会破坏现有测试
- 如果 starry-fatfs 有问题，可以切换到自实现 FAT32 或 fork fatfs

## 注意事项

- **锁顺序**：VFS lock → FS lock → Process table lock（严格有序，防止死锁）
- **SMP 安全**：FAT32 的 RefCell 用 SpinLock 包装
- **ELF 加载**：from_elf 需要接受 `&[u8]`，FAT32 的文件数据需要读入内存缓冲区
- **磁盘大小**：从 1MB 扩大到 64MB，确保 QEMU -m 128M 足够
- **FAT32 簇大小**：64MB 磁盘推荐 4KB 簇（SecPerClus=8），减少浪费

✅ 自审通过
