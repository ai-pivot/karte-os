# KarteOS 用户程序支持路线图

> 目标：让自研语言编译的 RISC-V ELF 在 KarteOS 上运行
> 输入：裸机 runtime，不依赖 libc，直接 syscall，ELF < 10KB
> 策略：定义 KarteOS 自己的 syscall ABI，语言编译器适配这个 ABI

---

## KarteOS Syscall ABI 定义

既然两边都是自己的项目，ABI 完全自由设计。以下是经过精简的接口：

### Syscall 调用约定

```
触发：ecall 指令（S-mode 收到 UserEnvCall，exception code = 8）
寄存器：
  a7 = syscall 号
  a0-a5 = 参数（最多 6 个）
  a0 = 返回值（成功返回 >= 0，错误返回负数）
  a1 = 额外返回值（如 read 返回实际字节数）
```

### Syscall 列表（共 20 个，分 4 个级别）

#### Level 1 — 最小可运行（Hello World）— Phase 1 实现

| 号 | 名称 | 签名 | 说明 |
|----|------|------|------|
| 0 | `sys_debug_print` | `(buf: *const u8, len: usize) -> isize` | 内核调试输出（早期用，后期删） |
| 1 | `sys_exit` | `(code: i32) -> !` | 退出进程 |
| 2 | `sys_write` | `(fd: i32, buf: *const u8, len: usize) -> isize` | 写到 fd |
| 3 | `sys_read` | `(fd: i32, buf: *mut u8, len: usize) -> isize` | 从 fd 读 |
| 4 | `sys_brk` | `(addr: *mut u8) -> *mut u8` | 设置/获取堆顶 |
| 5 | `sys_getpid` | `() -> i32` | 获取进程 ID |

#### Level 2 — 文件系统 — Phase 3 实现

| 号 | 名称 | 签名 | 说明 |
|----|------|------|------|
| 10 | `sys_open` | `(path: *const u8, flags: u32) -> i32` | 打开文件，返回 fd |
| 11 | `sys_close` | `(fd: i32) -> i32` | 关闭 fd |
| 12 | `sys_seek` | `(fd: i32, offset: isize, whence: i32) -> isize` | 文件 seek |
| 13 | `sys_stat` | `(path: *const u8, buf: *mut Stat) -> i32` | 获取文件信息 |
| 14 | `sys_opendir` | `(path: *const u8) -> i32` | 打开目录 |
| 15 | `sys_readdir` | `(fd: i32, buf: *mut Dirent) -> i32` | 读目录项 |

#### Level 3 — 网络 — Phase 4 实现

| 号 | 名称 | 签名 | 说明 |
|----|------|------|------|
| 20 | `sys_socket` | `(domain: i32, type: i32, protocol: i32) -> i32` | 创建 socket |
| 21 | `sys_connect` | `(fd: i32, addr: *const SockAddr, len: usize) -> i32` | 连接 |
| 22 | `sys_bind` | `(fd: i32, addr: *const SockAddr, len: usize) -> i32` | 绑定地址 |
| 23 | `sys_listen` | `(fd: i32, backlog: i32) -> i32` | 监听 |
| 24 | `sys_accept` | `(fd: i32, addr: *mut SockAddr, len: *mut usize) -> i32` | 接受连接 |
| 25 | `sys_send` | `(fd: i32, buf: *const u8, len: usize, flags: i32) -> isize` | 发送 |
| 26 | `sys_recv` | `(fd: i32, buf: *mut u8, len: usize, flags: i32) -> isize` | 接收 |

#### Level 4 — 线程 — Phase 5 实现

| 号 | 名称 | 签名 | 说明 |
|----|------|------|------|
| 30 | `sys_clone` | `(stack: *mut u8) -> i32` | 创建线程（共享地址空间） |
| 31 | `sys_futex_wait` | `(addr: *const u32, expected: u32) -> i32` | futex 等待 |
| 32 | `sys_futex_wake` | `(addr: *const u32, count: u32) -> i32` | futex 唤醒 |
| 33 | `sys_sleep` | `(ms: u64) -> i32` | 睡眠毫秒 |

---

## 实施阶段

### Phase 1 — MVP：跑起来 Hello World（预估 4-6 天）

> 里程碑：`print("hello")` 编译为 ELF → KarteOS 加载 → 屏幕输出 "hello"

需要实现：

```
1. 用户地址空间布局
   0x0000_0000 .. 0x0040_0000  用户代码+数据（从 ELF 加载）
   0x0040_0000 .. 0x0080_0000  用户堆（brk 管理）
   0x7FC0_0000 .. 0x8000_0000  用户栈（1MB，向下增长）
   0x8000_0000 ..              内核空间（用户不可访问）

2. ELF 加载器
   - 解析 ELF header（检查 magic \x7fELF、machine=RISC-V）
   - 加载 LOAD 段到用户地址空间
   - 设置用户栈
   - 跳转到 entry point（sret 到 U-mode）

3. U-mode 切换
   - 构造 TrapContext（设置 sepc=entry, sp=ustack_top, sstatus.SPP=0）
   - 设置 sscratch 保存内核栈指针
   - sret 跳到 U-mode 执行

4. 最小 Syscall 处理
   - trap_handler 中处理 UserEnvCall（exception code 8）
   - 实现 sys_debug_print / sys_write / sys_exit / sys_brk / sys_getpid

5. 进程管理
   - Process 结构体（pid, page_table_root, kernel_stack, user_stack, brk）
   - 从内核线程切换到用户进程

6. 修改 scheduler
   - 支持"用户进程"和"内核线程"混合调度
   - 用户进程 trap 回来后恢复 U-mode 上下文
```

文件变更：
- 新增 `kernel/src/process/mod.rs` — 进程管理 + ELF 加载
- 新增 `kernel/src/process/elf.rs` — ELF 解析器
- 新增 `kernel/src/process/addr_space.rs` — 用户地址空间管理
- 重写 `kernel/src/syscall/mod.rs` — 新 ABI 实现
- 修改 `kernel/src/arch/trap.rs` — U-mode trap 处理
- 修改 `kernel/src/sched/` — 支持用户进程调度
- 新增 `user/` 目录 — 测试用户程序（汇编写的最小 ELF）

验证标准：
```
make run
> === KarteOS v0.3.0 ===
> [init] ...
> [process] Loading user program: /hello.elf
> [process] ELF entry=0x1000, loaded 2 segments
> hello world          ← 这是用户程序输出的
> [process] User process exited with code 0
```

### Phase 2 — 堆分配（预估 2 天）

> 里程碑：用户程序可以 malloc/free，动态数据结构可用

需要实现：
- `sys_brk` 完整实现（按页分配，lazy grow）
- 用户页表动态扩展（缺页异常处理 page fault → 分配新页）
- 用户进程独立的地址空间（每个进程自己的页表根）

验证：
```c
// 用户程序
int *arr = malloc(100 * sizeof(int));
arr[99] = 42;
print(arr[99]); // 42
```

### Phase 3 — 文件系统（预估 3-4 天）

> 里程碑：用户程序可以 open/read/write 文件

需要实现：
- VirtIO 块设备驱动修复（QEMU 加 `-device virtio-blk-device`）
- 简单 FAT32 或自定义文件系统（基于 VirtIO 块设备）
- VFS 层（虚拟文件系统，fd → file 抽象）
- 文件描述符表（per-process fd table）
- sys_open / sys_close / sys_read / sys_write / sys_seek
- QEMU 启动时传入 initrd（cpio 归档）作为根文件系统

验证：
```
let f = open("test.txt", CREATE|WRITE);
write(f, "hello");
close(f);
let f2 = open("test.txt", READ);
let buf = read(f2, 100);
print(buf); // "hello"
```

### Phase 4 — 网络（预估 3-4 天）

> 里程碑：用户程序可以 TCP 连接外部

需要实现：
- VirtIO 网络驱动修复
- 简易 TCP/IP 栈（smoltcp crate 或手写）
- Socket 抽象（fd 复用）
- sys_socket / sys_connect / sys_bind / sys_listen / sys_accept / sys_send / sys_recv

### Phase 5 — 线程（预估 3-4 天）

> 里程碑：用户程序可以创建线程

需要实现：
- sys_clone（共享地址空间创建新执行流）
- 线程本地存储（TLS）
- futex 系统调用（互斥锁的基础）
- sys_sleep
- Mutex / Condvar 用户态实现（futex 之上）

---

## 用户程序 ABI 约定

你的语言编译器需要适配以下约定：

### ELF 要求
- 格式：RV64G ELF（`EM_RISCV = 243`）
- 入口：`e_entry` 指向的虚拟地址
- 只需要 PT_LOAD 段（可执行 + 可读写）
- 不需要动态链接（静态编译即可）

### Syscall 调用
```riscv
# 你的语言 runtime 中这样调 syscall：
li a7, 2          # sys_write
li a0, 1          # fd = stdout
la a1, msg        # buf
li a2, 12         # len
ecall             # → 内核处理，a0 返回值
```

### 堆管理
```riscv
# 获取当前堆顶
li a7, 4          # sys_brk
li a0, 0          # addr=0 表示查询当前值
ecall             # a0 = 当前堆顶地址

# 扩展堆
li a7, 4
li a0, new_top    # 新的堆顶地址
ecall             # a0 = 实际设置的新堆顶（可能不够）
```

### 启动约定
内核加载 ELF 后跳到 entry，此时：
- `a0` = argc（参数个数，暂时为 0）
- `a1` = argv（参数数组指针，暂时为 NULL）
- `sp` = 用户栈顶（1MB 栈）
- 所有其他寄存器 = 0
- `sstatus.SPP` = 0（U-mode）

---

## 预估时间线

```
Week 1:  Phase 1 — MVP Hello World        ← 最高优先级
Week 2:  Phase 2 — 堆分配
Week 3:  Phase 3 — 文件系统
Week 4:  Phase 4 — 网络
Week 5:  Phase 5 — 线程
```

每个 Phase 完成后你就可以让语言多使用一个特性。
Phase 1 完成后就能跑 `print("hello")` —— 这是最关键的一步。
