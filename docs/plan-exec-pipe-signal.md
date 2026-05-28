# 计划：exec + pipe/dup2 + 信号

> 生成时间：2026-05-28
> 状态：执行中

## 背景与目标

KarteOS 当前只能运行编译时 `include_bytes!` 嵌入的程序。需要实现三大特性：
1. **sys_exec + 持久化 FS** — 让 shell 能执行磁盘上的任意程序
2. **sys_pipe + sys_dup2** — 让 shell 支持 `|` 和重定向
3. **信号** — Ctrl+C 杀前台进程

## 实施顺序

```
阶段一：FdTable 重构（pipe/dup2 的前提）
  │
  ├──→ 阶段二：sys_pipe + sys_dup2
  │
  ├──→ 阶段三：sys_exec + 简单磁盘文件系统
  │
  └──→ 阶段四：信号 + Ctrl+C
```

FdTable 重构必须先做——当前 name-based 设计无法表示 pipe 和 dup2。

---

## 阶段一：FdTable 重构

### 核心思路

把 `FileDescriptor { name, pos, flags }` 替换为 `FileDescriptor { inner: Arc<FileObject> }`，
让 fd 变成对内核对象的引用。pipe 的两个 fd 共享同一个 `Arc<PipeBuffer>`。

### 步骤

- [ ] 1.1 在 `driver/fs.rs` 中定义 `FileObject` enum：
  ```rust
  pub struct FileObject {
      pub kind: FileKind,
      pub pos: usize,
      pub flags: u32,
  }
  pub enum FileKind {
      File(String),              // 内存 FS 文件名
      Stdin,                     // TTY read
      Stdout,                    // UART write
      Pipe(Arc<Mutex<PipeBuffer>>),  // 管道共享缓冲区
  }
  ```
  注意：不用 trait，用 enum。内核 OS 不需要过度抽象，enum 更简单、零开销。

- [ ] 1.2 重构 `FileDescriptor`：
  ```rust
  pub struct FileDescriptor {
      pub inner: Arc<Mutex<FileObject>>,
  }
  ```
  `Arc` 让 dup2 和 pipe 可以共享同一个对象。`Mutex` 保护 pos 等可变状态。

- [ ] 1.3 重构 `FdTable::new()` — fd 0/1/2 分别创建 Stdin/Stdout/Stdout 对象

- [ ] 1.4 重构 `sys_read` — 消除硬编码 `fd==0`，统一走 `FdTable → inner.read()`：
  ```rust
  fn sys_read(fd, buf, len) {
      with_fd_table(|t| {
          let obj = t.get(fd).inner.lock();
          match &obj.kind {
              Stdin => tty::read_line(buf, len),
              File(name) => /* 现有 FS 读取逻辑 */,
              Pipe(buf) => /* 从管道读 */,
              Stdout => ERR_INVAL,
          }
      })
  }
  ```

- [ ] 1.5 重构 `sys_write` — 消除硬编码 `fd==1|2`，统一走 `FdTable → inner.write()`

- [ ] 1.6 验证：`make test` 59/59 通过（有 15 个 FS 和 15 个 Syscall 测试）

**涉及文件**：`driver/fs.rs`, `syscall/mod.rs`
**风险**：现有 FS 和 Syscall 测试必须全部通过

---

## 阶段二：sys_pipe + sys_dup2

### 步骤

- [ ] 2.1 在 `driver/fs.rs` 中新增 `PipeBuffer`：
  ```rust
  pub struct PipeBuffer {
      data: [u8; 4096],
      read_pos: usize,
      write_pos: usize,
      count: usize,
  }
  ```
  固定 4KB，简单的环形缓冲。读空返回 0，写满丢弃。

- [ ] 2.2 新增 syscall 常量 `SYS_PIPE=32, SYS_DUP2=33`

- [ ] 2.3 实现 `sys_pipe(fd_ptr)`：
  1. 创建 `Arc::new(Mutex::new(PipeBuffer::new()))`
  2. alloc 一个 fd → `FileKind::Pipe(arc.clone())`（read end）
  3. alloc 另一个 fd → `FileKind::Pipe(arc)`（write end）
  4. 把两个 fd 号写入用户空间的 `fd_ptr`

- [ ] 2.4 实现 `sys_dup2(old, new)`：
  1. 取 `old` 的 `Arc` clone
  2. 替换 `new` slot 的 inner
  3. 如果 new 之前有对象，close 它

- [ ] 2.5 在 sys_read/sys_write 的 Pipe 分支中实现环形缓冲读写

- [ ] 2.6 新增测试：pipe 读写、dup2、close 一端

**涉及文件**：`driver/fs.rs`, `syscall/mod.rs`, `syscall/mod.rs`（测试）
**新增 syscall**：SYS_PIPE(32), SYS_DUP2(33)

---

## 阶段三：sys_exec + 磁盘文件系统

### 步骤

- [ ] 3.1 在 `driver/fs.rs` 中新增 `FileSystem::read_file_bytes(name) -> Option<Vec<u8>>`
  简单包装现有的 `read()` 返回 `Vec<u8>` 而非引用（避免借用问题）。

- [ ] 3.2 新增 syscall `SYS_EXEC=34`

- [ ] 3.3 实现 `sys_exec(path, path_len)`：
  ```
  1. 从用户内存读路径
  2. global_fs().read_file_bytes(&name) → elf_data
  3. 解析 ELF → 验证合法性
  4. 释放旧用户地址空间（vmm::free_user_pages）
  5. 创建新页表 + 加载 ELF segments + 映射用户栈
  6. 更新 Process 字段: entry, brk, page_table_root, user_stack_top
  7. 更新 CURRENT_PAGE_TABLE_ROOT
  8. 修改 TrapContext: sepc=new_entry, x[2]=new_user_sp, sscratch=new_user_sp
  9. 重置 fd_table（或保留，看需求）
  10. 返回 0 → sret 到新程序入口
  ```

- [ ] 3.4 实现磁盘文件系统（简单方案）：
  - 在 `driver/fs.rs` 中新增 `flush_to_disk()` 和 `load_from_disk()`
  - 磁盘布局：sector 0 = superblock (magic + file_count)，之后每个文件 = header(名字+长度) + 数据扇区
  - `make shell` 时通过 QEMU 的 virtio-blk-device 挂载磁盘镜像
  - 初始化时自动从磁盘加载文件到内存 FS

- [ ] 3.5 更新 `sys_spawn` — 当 prog_id 找不到时，尝试从文件系统路径加载

- [ ] 3.6 验证：`make test` + 手动 exec 测试

**涉及文件**：`driver/fs.rs`, `syscall/mod.rs`, `process/mod.rs`, `mm/vmm.rs`, `main.rs`

---

## 阶段四：信号 + Ctrl+C

### 步骤

- [ ] 4.1 扩展 `Process` 结构体：
  ```rust
  pub pending_signals: u64,        // pending 位图
  pub signal_handlers: [SignalAction; 32],  // 每个信号的处理方式
  pub pgid: usize,                 // 进程组 ID
  ```
  ```rust
  enum SignalAction { Default, Ignore, Handler(usize) }
  ```

- [ ] 4.2 新增 syscall: `SYS_KILL=35, SYS_SIGACTION=36, SYS_SIGRETURN=37`

- [ ] 4.3 实现 `sys_kill(pid, signum)` — 设置目标进程的 pending_signals 位

- [ ] 4.4 信号投递 — 在 `trap_handler` return ctx 之前：
  1. 检查 pending_signals
  2. 取最高优先级的 pending 信号
  3. 如果 SIG_DFL 且是 SIGINT/SIGKILL → schedule_exit()
  4. 如果有 handler → 在用户栈上构建 sigframe（保存原始 TrapContext）
     修改 ctx.sepc = handler_addr, ctx.sp -= sizeof(sigframe)
  5. 清除 pending 位

- [ ] 4.5 实现 `sys_sigreturn()` — 从用户栈的 sigframe 恢复原始 TrapContext

- [ ] 4.6 Ctrl+C 杀前台进程：
  1. TTY 新增 `FOREGROUND_PGID` 全局变量
  2. Ctrl+C 时，向所有 `pgid == FOREGROUND_PGID` 的进程发 SIGINT
  3. SIGINT 默认动作 = exit(130)
  4. shell 通过 sys_setpgid() 设置子进程的 pgid

- [ ] 4.7 新增 `SYS_SETPGID=38`

- [ ] 4.8 验证：Ctrl+C 杀前台进程

**涉及文件**：`process/mod.rs`, `arch/trap.rs`, `driver/tty.rs`, `syscall/mod.rs`

---

## 验证方案

| 阶段 | 验证 |
|------|------|
| 一 | `make test` 59/59，hello.elf 正常启动 |
| 二 | pipe 读写测试、dup2 测试 |
| 三 | `sys_exec("/hello")` 执行文件系统中的程序 |
| 四 | Ctrl+C 杀死前台进程，信号 handler 测试 |

## 注意事项

- 每个 phase 完成后必须 `make test` 确保不回归
- FdTable 重构影响面最大（15 个 FS 测试 + 15 个 Syscall 测试），要格外小心
- 信号投递的 sigframe 构建需要 SUM 位才能写用户栈
- exec 复用当前内核栈，不能释放，只替换用户地址空间
