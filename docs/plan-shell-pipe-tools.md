# 计划：Shell 管道/重定向 + 用户工具 + Shell增强 + 信号 + Fork

> 生成时间：2026-05-29
> 状态：执行中

## 背景与目标

让 KarteOS 从"能跑程序"升级到"能工作"。实现管道、重定向、核心文本工具、Shell 增强、信号系统、fork。

## 阶段一：内核管道子系统

### 核心改动

1. **新建 `kernel/src/driver/pipe.rs`** — 管道数据结构和操作
   - `Pipe` 结构：环形缓冲区(4KB) + read_pos/write_pos + read_closed/write_closed + reader_proc/writer_proc
   - 全局 `PIPE_TABLE: SpinLock<[Option<Pipe>; MAX_PIPES]>` (MAX_PIPES=16)
   - `pipe_alloc()` → 返回 pipe_id
   - `pipe_read(pipe_id, buf, len) -> isize` — 阻塞读（空+写端开 → schedule_block）
   - `pipe_write(pipe_id, buf, len) -> isize` — 阻塞写（满 → schedule_block）
   - `pipe_close_read(pipe_id)` / `pipe_close_write(pipe_id)` — 关闭端+唤醒

2. **扩展 `FileDescriptor`** (`kernel/src/driver/fs.rs`)
   - 添加 `FdType` 枚举：`File`, `Stdio`, `PipeRead`, `PipeWrite`
   - 添加 `pipe_id: Option<usize>` 字段
   - `FdTable::set_fd(fd, desc)` 方法 — 允许覆盖 fd[0]/[1]/[2]

3. **新增 syscall** (`kernel/src/syscall/mod.rs`)
   - `SYS_PIPE = 7` — `sys_pipe(fd_ptr: usize)` — 创建管道，写回两个 fd 到用户态
   - `SYS_DUP2 = 8` — `sys_dup2(old_fd, new_fd)` — 复制 fd

4. **修改 `sys_read`/`sys_write`/`sys_close`**
   - fd=0: 先查 FD 表，是 PipeRead → pipe_read，否则 → tty::read
   - fd=1/2: 先查 FD 表，是 PipeWrite → pipe_write，否则 → console_putchar
   - fd≥3: 查 FD 表，检查 FdType → File → 现有逻辑 / Pipe → pipe 操作
   - sys_close: 检查 Pipe 类型 → 调用 pipe_close_read/write

5. **初始化** — 在 `main.rs` 中初始化 pipe 子系统

## 阶段二：Shell 重定向

### 改动文件：`user/shell.rs`

1. **解析重定向运算符** — 在 `launch()` 调用前扫描命令行
   - `> file` — stdout 重定向到文件（O_CREAT|O_TRUNC）
   - `>> file` — stdout 追加到文件（O_CREAT|O_APPEND）
   - `< file` — stdin 从文件读取

2. **修改 `launch()`** — 接受 stdin_fd/stdout_fd 参数
   - 重定向文件时：open 文件获取 fd
   - 如果需要 pipe fd：从参数传入
   - exec 后子进程继承这些 fd 设置

3. **syscall.rs 新增** — `sys_dup2(old, new)` wrapper

## 阶段三：Shell 管道

### 改动文件：`user/shell.rs`

1. **解析管道符 `|`** — 拆分命令链
   - `cmd1 | cmd2 | cmd3` → [cmd1, cmd2, cmd3]

2. **管道执行模型**（spawn-based，不需要 fork）
   ```
   cmd1 | cmd2:
     1. sys_pipe(&fds) → [read_fd, write_fd]
     2. 修改 shell 进程自己的 fd[0]=read_fd (临时保存原 stdin)
     3. launch(cmd1, stdout_fd=write_fd) — 子进程 stdout=pipe 写端
     4. sys_close(write_fd) — shell 关闭写端（子进程持有）
     5. launch(cmd2, stdin_fd=read_fd) — 子进程 stdin=pipe 读端
     6. sys_close(read_fd)
     7. wait_for(pid1) + wait_for(pid2)
   ```

3. **多级管道** — `cmd1 | cmd2 | cmd3` 需要多对 pipe，shell 自己管理中间 fd

## 阶段四：用户工具

### 新增程序（全部基于 `user/cat.rs` 模板）

1. **`user/grep.rs`** — 文本搜索
   - 支持基本正则（子串匹配即可）
   - 从 stdin 或文件读取，匹配行输出到 stdout
   - 参数：`grep PATTERN [FILE]`

2. **`user/sed.rs`** — 流编辑器
   - 支持 `s/pattern/replacement/[g]` 替换命令
   - 从 stdin 或文件读取，处理后输出到 stdout
   - 参数：`sed 's/old/new/g' [FILE]`

3. **`user/wc.rs`** — 字数统计
   - 输出行数、单词数、字节数
   - 参数：`wc [FILE]`

4. **`user/head.rs`** — 输出前 N 行
   - 默认 10 行，`-n N` 自定义
   - 参数：`head [-n N] [FILE]`

5. **`user/tail.rs`** — 输出后 N 行
   - 默认 10 行，`-n N` 自定义
   - 参数：`tail [-n N] [FILE]`

6. **`user/Makefile`** — 添加所有新程序到 TARGETS 和编译规则

## 阶段五：Shell 增强

### 改动文件：`user/shell.rs`

1. **命令历史**
   - 环形缓冲区存储最近 64 条命令
   - ↑↓ 键浏览历史
   - 需要逐字符读取模式（识别 VT100 转义序列 `ESC[A`/`ESC[B`）

2. **Tab 补全**
   - 列出 PATH 中的可执行文件名
   - 单匹配：自动补全
   - 多匹配：列出所有可能

3. **行缓冲扩大** — 从 256 字节扩大到 512 字节

4. **输入模式改进**
   - 逐字符读取替代整行读取
   - 支持 Backspace 删除（在用户态处理，而非仅依赖内核 TTY）
   - 识别 ↑(ESC[A)、↓(ESC[B)、Tab(0x09)、Ctrl+C(0x03)

## 阶段六：信号系统

### 内核改动

1. **新建 `kernel/src/signal.rs`** — 信号子系统
   - 信号常量：SIGINT=2, SIGKILL=9, SIGTERM=15
   - 全局信号处理：默认动作（SIGINT→终止, SIGKILL→强制终止）
   - 信号检查点：在 trap_handler 返回用户态前检查 pending_signals

2. **新增 syscall**
   - `SYS_KILL = 9` — `sys_kill(pid, sig)` — 发送信号
   - `SYS_SIGNAL = 12` — `sys_signal(sig, handler)` — 注册信号处理器（简化版，暂只支持 SIG_DFL/SIG_IGN）

3. **进程表扩展** — `Process` 结构添加 signal 相关字段

4. **TTY 集成** — Ctrl+C 时查找前台进程并发送 SIGINT

### Shell 改动

1. **内建 `kill` 命令** — `kill PID [SIG]`
2. **前台进程跟踪** — shell 记录最近启动的子进程 PID

## 阶段七：Fork

### 内核改动

1. **新增 `SYS_FORK = 13`**
2. **`Process::fork()` 方法**
   - 克隆页表（写时复制 COW 暂不实现，直接深拷贝用户页）
   - 复制 FD 表（增加 pipe 引用计数）
   - 复制 brk/mmap 区域
   - 子进程返回 0，父进程返回 child_pid

3. **调度器集成** — fork 的子进程需要设置正确的入口点（复用父进程 sepc）

## 验证方案

- `make build` — 零错误
- `make test` — 70/70 通过
- `make boot-test` — 启动到 KarteOS Shell
- QEMU 交互测试：
  - `echo hello > test.txt && cat test.txt` — 重定向
  - `cat test.txt | grep hello` — 管道
  - `echo -e "hello\nworld" | sed 's/hello/hi/g'` — sed
  - `↑` 命令历史

## 回滚策略

每个阶段在独立 commit 上，可通过 `git revert` 回滚。
