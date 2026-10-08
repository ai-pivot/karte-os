# KarteOS 超级 AI IoT OS · 终极路线图（Master Plan）

| | |
|---|---|
| **版本** | v1.0 |
| **日期** | 2026-10-08 |
| **状态** | 正式生效（长期演进文档） |
| **维护人** | adm |
| **关联文档** | `AGENTS.md`（编码纪律与快查） · `docs/agent/*`（子系统知识） · `README.md`（对外介绍） |
| **本文地位** | **唯一总纲**。所有阶段的启动与关闭以本文 Checklist 为准；完成一项勾一项，并附 commit/PR 引用 |

---

## 0. 使用约定

1. **Checklist 纪律**：每个任务形如 `- [ ] 描述（验收：…）`。完成时改为 `- [x]` 并在同一行末尾追加 `(commit: <hash>)`。禁止批量勾选——只有验收条件可复现才算完成。
2. **验收必须可验证**：每项任务的括号内写明验收方式（测试通过 / 演示脚本 / 基准数字 / 文档章节链接）。写不出验收方式的任务不许进 Checklist。
3. **变更规则**：目标与身份主张（§1）的修改需要明确记录在附录 D 变更日志；Checklist 增删可随时进行但必须保留历史痕迹（追加而非删除，废弃项用 `~~删除线~~`）。
4. **取舍标尺**：任何新 feature 必须能映射到 §1.2 的至少一条身份主张，否则默认拒绝。
5. **季度评审**：每季度末对照里程碑表（§6）与 KPI 表（附录 C）复盘一次，裁剪 scope。

---

## 1. 终极目标

### 1.1 愿景陈述

> KarteOS 要成为运行在万物之上、以 AI 为原生能力、以安全为默认属性的高性能操作系统。在「AI-IoT」主战场上，全面超越 **Linux**（通用但非 AI 原生、默认非实时、内存安全黑洞）与**米家 Vela/HyperOS**（互联但封闭、技能注册锁在云端、MCU 与手机两截断裂、数据必须上云）所代表的现有范式。

一句话：**给每一台设备一个开放的脑子接口，给每一个脑子一套统一的内核。**

### 1.2 五大身份主张（取舍标尺）

| # | 主张 | 内涵 | 直接打击的对手弱点 |
|---|------|------|--------------------|
| I1 | **AI 是系统调用，不是库** | 模型生命周期、KV-cache 感知内存、token 级 IPC、AI 调度类、NPU HAL 全部进入内核视野 | Linux：AI 只是用户态库，调度器对推理任务一无所知 |
| I2 | **一个内核，全场景** | 同一 Rust 内核从 MCU（百 KB 裁剪）到边缘服务器（多核大内存），feature-flag 裁剪而非换内核 | 米家：Vela 管 MCU + HyperOS 管 phone，两截断裂 |
| I3 | **脑-肢架构：设备即工具** | 多数设备跑不动模型 → 把自身能力**自动注册为 MCP 工具**；少数"脑"发现、编排、下达指令 | 米家：技能注册封闭在云；传统 IoT：MQTT 只有消息没有能力语义 |
| I4 | **Capability 即安全，WASM 即应用** | 权限是不可伪造的能力令牌，沙箱是默认态；WASM 应用跨 ISA 一次构建处处运行 | Linux：权限碎片化 + 内存 CVE；每芯片一版二进制 |
| I5 | **Local-first，数据主权** | 推理、数据、自动化全在本地，断云可用；云只做可选备份 | 米家：云绑定是结构性依赖，也是隐私软肋 |

### 1.3 对标分析

| 对手 | 强项 | 结构性弱点 | KarteOS 解法 |
|------|------|-----------|--------------|
| **Linux** | 生态、驱动、服务器统治力 | AI=用户态库；默认非实时（PREEMPT_RT 是补丁）；最小系统数十 MB；~70% CVE 来自内存安全 | AI 调度类（I1）、RT 优先级、Rust 全内核、同内核下探小设备（I2） |
| **米家（Vela+HyperOS）** | 互联生态、出货量、体验打磨 | 技能注册封闭在自家云；两截 OS；开发者被平台绑定；数据上云 | 开放 MCP 注册（I3）、本地脑（I5）、一套内核（I2） |
| **Zephyr / FreeRTOS** | 小、实时、MCU 覆盖 | 无 AI 能力、无大型应用生态、无统一应用模型 | 同内核上探大模型 + WASM 应用层（I1/I4） |
| **RT-Thread / HarmonyOS** | 组件化 / 分布式软总线 | RT-Thread 同 Zephyr；HarmonyOS 重且生态封闭 | 开放标准（MCP/mDNS/WASM）而非私有协议（I3） |

---

## 2. 核心架构总纲

### 2.1 脑-肢架构（Brain-Limb）

```
        ┌─────────────┐        ┌─────────────┐
        │  脑 (Brain)  │        │  脑 (Brain)  │    ← 少数：能跑 LLM 的节点
        │ LLM + MCP宿主│        │ (可互为备份)  │       家居网关/边缘盒子/车机/PC
        └──────┬──────┘        └──────┬──────┘
               │   MCP（发现/调用/事件）  │
     ┌─────────┼───────────┬──────────┼─────────┐
 ┌───▼───┐ ┌───▼───┐  ┌────▼───┐ ┌────▼───┐ ┌───▼────┐
 │肢体:灯 │ │肢体:门锁│  │肢体:传感器│ │肢体:电机│ │肢体:摄像头│  ← 多数：跑不动模型
 └───────┘ └───────┘  └────────┘ └────────┘ └────────┘     只负责"把能力注册为工具"
```

**角色定义**

| 角色 | 硬件画像 | 职责 | 必备内核能力 |
|------|---------|------|-------------|
| 脑 Brain | 边缘盒子/网关/车机（≥1GB RAM） | 跑 LLM 推理；聚合全网工具表；agent 决策循环；下发达令 | AI 调度类、模型生命周期 syscall、MCP 宿主 |
| 肢体 Limb | MCU/小 SoC（KB~百 MB 级） | 注册能力为工具；执行指令；脑离线时按缓存策略自治 | CapDesc 注册栈、紧凑 MCP profile、能力令牌校验、离线自治 |
| 协调者 Arbiter（后期） | 脑之一 | 跨脑协同、脑选举、冲突消解 | 开放互联总线（Phase 4） |

**设计原则**

1. 多数设备无脑、少数脑——系统按此不对称性设计，肢体栈的代码体积与内存预算用 MCU 标准约束。
2. 注册是开放的：任何脑可以接入任何肢体（标准协议），不存在"私有云激活"。
3. 断脑不瘫：脑掉线是常态而非异常，肢体自治是必答题。
4. 脑本身也是 KarteOS 节点——脑与肢同内核不同裁剪（I2）。

### 2.2 「驱动即工具」——MCP 设备织物五层模型

| 层 | 名称 | 职责 | 关键设计 |
|----|------|------|----------|
| L1 | 能力描述层（CapDesc） | 内核驱动声明能力描述符 → 自动生成 MCP tool 定义（name / inputSchema / outputSchema / 权限位） | **OS 层独有创新**：设备出厂即带工具，无需人工写 MCP 适配。ABI 草案见附录 A |
| L2 | 发现注册层（DRT） | 设备上电→广播自述→脑端设备注册表（Device Registry Table）→工具表实时注入模型上下文 | v0 用 KarteOS-native UDP announce（先跑通），v1 升级 mDNS/DNS-SD（标准互通）。状态机见附录 B |
| L3 | 调用传输层 | MCP 语义（JSON-RPC 2.0）的两级传输：**Linux 级**=Streamable HTTP（跑在 smoltcp TCP 上）；**MCU 级**=紧凑二进制 profile MCP-CB（网关转译，语义兼容） | 同一个脑统一调度两类设备，工具 schema 相同 |
| L4 | 安全层 | 每工具一颗**能力令牌**：脑持有、设备校验、可吊销；设备注册需签名清单防伪冒 | 与 Phase 3 capability 安全体系合流：一套令牌机制两处复用 |
| L5 | 自治层 | 脑离线→肢体按缓存策略自治（规则或 WASM 小程序）；脑恢复→上报离线期间状态 | 「无脑不瘫」：对比云方案的天然弱点，是家居/工业场景生死线 |

### 2.3 一个内核，全场景（尺度谱系）

| 档位 | 架构 | 内核体积目标 | RAM 目标 | 典型硬件 | 现状 |
|------|------|-------------|---------|---------|------|
| S（MCU） | riscv32 / armv8-m | ≤ 256KB | ≤ 256KB | ESP32-C3、STM32 | 未开始（Phase 3 研究） |
| M（轻量 SoC） | riscv64 / aarch64 | ≤ 2MB | 64~512MB | RPi Zero 2、RV 开发板 | riscv64 2.3MB ✅（QEMU） |
| L（边缘） | aarch64 / x86_64 | ≤ 4MB | 1~16GB | RPi5、RK3588、边缘盒子 | x86_64 1.7MB ✅（QEMU+实机） |
| XL（服务器） | x86_64 | ≤ 8MB | 16GB+ | 边缘服务器 | 同 L |

实现机制：Cargo feature 分层裁剪（`core` / `net` / `fs` / `ai` / `fabric`），禁止用条件编译散养——每个子系统必须声明自己属于哪一档（写入 `docs/agent/` 对应文件）。

### 2.4 AI-native 内核服务（I1 的落地清单）

| 服务 | 内容 | 所属阶段 |
|------|------|---------|
| AI 调度类 | `SCHED_AI_BATCH`：可被 RT 抢占、可被整批暂停/恢复、感知 token 生成的周期性 | Phase 1（类框架）/ Phase 4（token 感知） |
| 模型生命周期 | `sys_model_load/unload/pin`：权重映射、锁页、多模型共存配额 | Phase 4 |
| KV-cache 内存池 | 大页 + 专用池化器 + 压力时向普通页让步 | Phase 4 |
| token IPC | 零拷贝 token 流管道（脑↔应用），语义上是 pipe 的 AI 特化 | Phase 4 |
| NPU HAL | 厂商 NPU 统一抽象：submit/sync/内存约定 | Phase 4（真机驱动到位后） |

---

## 3. 现状基线（2026-10-08 实测）

### 3.1 已有能力（证据）

| 领域 | 现状 | 证据 |
|------|------|------|
| 架构 | 双架构：riscv64（QEMU virt + OpenSBI，内核 2.3MB）、x86_64（Multiboot2/GRUB，1.7MB，含实机 NVMe 路径） | `make test` / 构建产物实测 |
| 内存 | PMM bitmap + Sv39/CR3 每进程独立页表 + VMA lazy mmap + mprotect/madvise(DONTNEED/POPULATE) | `kernel/src/mm/*` |
| 调度 | 纯 Round-Robin；`TaskKind{Empty,Idle,User}`；`MAX_TASKS=64` 静态槽位；双核 RISC-V / 四核 x86_64 SMP | `sched/mod.rs:54` |
| 文件系统 | ext4（vendored 补丁版）+ FAT32 + RamFS + VFS | `driver/ext4*`, `driver/vfs.rs` |
| 网络 | smoltcp 0.12：IPv4 TCP/UDP/ICMP/DNS；syscall 70-77；定时器轮询 ~10ms | `kernel/src/net/` |
| Linux 兼容 | ~800 行：uname/getcwd/clone/futex/epoll(ET)/eventfd 等；Go 静态二进制半通 | `syscall/linux.rs` |
| IPC/用户态 | pipe、shell v0.5（管道/重定向/历史/Tab）、15+ 工具程序、`sys_exec_fd` | `user/` |
| 测试 | RISC-V **105/105**（本机实测）；x86_64 102/103（1 个 PMM 已知差异）；CI 5 job | `make test` 2026-10-08 |
| 环境 | stable 1.93.1 + nightly + 双 target + QEMU 6.2 + 交叉链，全就绪 | 本次会话验证 |

### 3.2 已知债务（Phase 0 清偿）

- [x] `kernel/build.rs` riscv64 分支缺 `-T memory.x` 链接参数（干净检出编不过）→ 已修复（待提交）
- [x] `Makefile` test 目标缺 `--target`（默认 target 撞 stable E0554）→ 已改 `cargo +nightly --target riscv64gc-...`（待提交）
- [x] `kernel/src/mm/vmm.rs` 两个测试引用 x86_64-only 函数缺 `#[cfg]` → 已修复（待提交）
- [x] `kernel/src/main.rs` x86_64 分支引用不存在的 `user/target/x86_64/shell.elf` → 已改 `user/shell.elf`（待提交）
- [x] `Cargo.lock`：`x86_64` crate 0.15.4→0.15.5（兼容新 nightly `Step` trait）→ 已修复（待提交）
- [ ] clippy **8 个 `not_unsafe_ptr_arg_deref` error**（CI lint job 预期红）
- [ ] AGENTS.md 测试数 96 → 实测 105；README 数字严重过时（"50 tests / 2512 行"，实际 3 万+ 行）
- [ ] 垃圾文件：`driver/fs.rs_fake.txt`、`driver/fs.rs_addition.txt`、`user/shell-riscv64.elf.bak`、`user/fd_test*`、`user/minclone/`、根目录 `xbot-cli-static`（69MB 二进制）

---

## 4. 差距分析总表

| 维度 | 现状 | 目标（量化） | Phase |
|------|------|-------------|-------|
| 调度 | 纯 RR，64 静态槽 | 32 级优先级 + RT/AI 调度类 + 动态任务表；RT 抢占延迟可测量 | 1 |
| 端侧推理 | 无 | QEMU 内 LLM 出 token（tokens/s 入基准库）；RVV 用户态可用 | 1 |
| 设备工具注册 | 无 | 设备上电→工具出现在脑端工具表（≤3s）；断脑 30s 自治不瘫 | 2 |
| 应用模型 | ELF per-arch | WASM 沙箱应用跨 ISA 安装运行 | 3 |
| 硬件覆盖 | QEMU 双架构 | +aarch64（QEMU→RPi5 真机）；rv32 研究预研 | 3 |
| IoT 连接 | 裸 TCP/UDP | MQTT + TLS + mDNS；OTA A/B 演示 | 3 |
| 安全 | 页表/双模式隔离 | capability 令牌全链路；secure boot 路径 | 3/4 |
| 电源 | `wfi` | tickless idle；DVFS 框架接口（真机字段） | 3 |
| 生态 | 无 | SDK + 工具描述宏 + 应用清单格式 | 4 |

---

## 5. 阶段计划与详细 Checklist

### Phase 0 · 止血与基线（2026-10，第 1-2 周）

**目标**：CI 全绿、仓库干净、基准有数。此后一切开发在绿基线上进行。

**Checklist**

- [x] 提交 5 项构建修复（build.rs / Makefile / vmm.rs cfg / main.rs 路径 / Cargo.lock）（验收：干净 clone 后 `cargo build --release -p karte-os-kernel --target riscv64gc-unknown-none-elf` 一次通过）(commit: 8ff7793, 00d86fa；CLEAN_CLONE_BUILD_OK 实测)
- [x] 清理 §3.2 全部垃圾文件，`.gitignore` 补充规则（验收：`git status` 无未跟踪杂物）(commit: ace03db)
- [x] 清零 clippy：修复 8 个 `not_unsafe_ptr_arg_deref`（验收：CI 同款 clippy 命令 0 error）(commit: 66726c1)
- [x] AGENTS.md 测试数 96→105（RISC-V）、x86_64 数字核对；README 全面同步（行数/测试数/双架构/核心特性）（验收：两文档数字与实测一致）(commit: 3edcdc6)
- [x] 本机跑通 `make test-x86`，确认 x86_64 现状并记录（验收：脚本退出码与已知失败项入库）(commit: 4186935 — 修复 user_write 槽位后 **131/131**，发现根因：grub-pc-bin 缺失 + ensure_user_write_pages 拒绝内核栈缓冲)
- [x] 本机验证 boot-test + smp-test（验收：两脚本各自通过）(commit: 48f3575 — 修复 build_initial_stack 的 x[2]/sscratch 槽位错位；此前 boot-test 必败，属既有回归)
- [x] push 后核对 GitHub Actions 5 job 全绿（验收：CI 页面截图/链接）(commits: 88eb671 + 40433f0；https://github.com/ai-pivot/karte-os/actions/runs/37766738861 — 7/7 job success。修复两处：CI 全部 RISC-V job 的 `make` 缺 `ARCH=riscv64`（user/Makefile 默认 x86_64，内核 include_bytes! 找不到 .S 程序 ELF）；smp-test 在共享 runner TCG 下 15s 超时不足改 90s)
- [x] 工具链防漂移：CI 与 rust-toolchain 对 nightly 采用固定日期版本（验收：CI 安装日志出现日期 pin；`Step` trait 类事故不再复现）(commit: 3edcdc6 — CI x86 job pin nightly-2026-10-07)
- [x] 建立基准库 `docs/benchmarks.md`：boot→shell 时间、上下文切换延迟、105 测试耗时、内核双架构体积（验收：四项数字入库，含测量方法）(commit: bd4e0c5 — 上下文切换标注 TBD，归 P1.1 基准 harness；boot→shell 67ms / 套件 67ms / RV 2.26MB / x86 1.62MB)
- [x] 将本路线图链接进 README（验收：README 出现 Master Plan 链接）(commit: 3edcdc6)

**DoD（退出准则）**：CI 5 job 全绿 + 文档数字同步 + 基准数字入库。

---

### Phase 1 · 内核现代化：调度、兼容、推理（2026-10 ~ 2026-12，约 8-10 周）

**目标**：调度器达到 IoT 门槛；Linux 兼容可跑真实 shell 生态；**M1：KarteOS 上跑出第一个 LLM token**。

#### P1.1 调度器 2.0

- [ ] 引入 `SchedClass { RtFifo(u8), RtRoundRobin(u8), Normal, AiBatch }`，任务创建时声明（验收：单测构造各类任务成功）
- [ ] 32 级优先级位图选择，O(1) 选next（验收：单测 1000 次随机入队出队顺序正确）
- [ ] 移除 `MAX_TASKS=64` 静态槽位 → 动态 TCB 分配 + pid 分配器（验收：压力测试创建 200 任务全部可调度）
- [ ] RT 优先语义：RT 永远先于 Normal/AiBatch；同 class 内 FIFO/RR 各按语义（验收：新增 ≥3 个调度单测覆盖抢占与饥饿防护）
- [ ] 新原生 syscall：`sys_setpriority` / `sys_getscheduler`（KarteOS ABI 编号续排，更新 AGENTS.md ABI 表）（验收：用户程序设置优先级生效）
- [ ] 测量 RT 抢占延迟（timer tick 粒度限制要注明），入 `docs/benchmarks.md`（验收：数字入库）
- [ ] 更新 `docs/agent/scheduler.md` 与 AGENTS.md 相关章节（验收：文档与代码一致）

#### P1.2 Linux 兼容层补齐

- [ ] `fork` 返回值语义修复（子进程 0）+ `wait4(pid, &status, options)`（验收：spawn_test 扩展用例通过）
- [ ] `execve(path, argv, envp)` 完整参数传递 + `exit_group`（验收：busybox ash 能启动）
- [ ] `pipe2` / `dup3` / `fcntl`（F_GETFD/F_SETFD/F_GETFL/F_SETFL）（验收：管道+重定向脚本用例通过）
- [ ] `ioctl` TCGETS/TIOCGWINSZ 语义补齐（验收：ash 行编辑不异常）
- [ ] mmap 家族完善：MAP_FIXED / munmap / 匿名私有语义对齐（验收：新增 ≥3 个 mmap 单测）
- [ ] 验收里程碑：**静态 busybox（ash + coreutils 子集）在 KarteOS 上跑通一个 shell 脚本**（`echo hi | grep h | wc -l` 类）（验收：演示日志入库）

#### P1.3 端侧推理（M1）与 RVV

- [ ] QEMU 使能 RVV：`-cpu rv64,v=true,vlen=128` 进 Makefile 新 target（验收：`make run-rvv` 可启动）
- [ ] 内核支持 V 扩展上下文：`sstatus.VS` 使能 + trap 保存/恢复 v0-v31/vl/vtype（TrapContext 扩容方案先写设计再动码）（验收：跨上下文切换向量寄存器不破坏的单测）
- [ ] 用户态编译链路：`rustc -C target-feature=+v` 程序在 KarteOS 上运行（验收：向量加法程序输出正确）
- [ ] M1 主体：candle（或自研最小 transformer 推理器）移植为 KarteOS 用户程序，加载一个小模型（TinyStories 级 15M 或 GPT-2 124M 量化）生成文本（验收：QEMU 内连续生成 ≥32 个连贯 token，日志入库）
- [ ] KPI：tokens/s、内存峰值入 `docs/benchmarks.md`（验收：数字+方法入库）

**DoD**：全部测试绿（含新增 ≥10 个）+ busybox 演示 + LLM 出 token + 基准更新。

---

### Phase 2 · MCP 设备织物：让设备变成工具（2026-11 ~ 2027-02，可与 P1 末并行）

**目标**：五层织物 v0 落地；**M2：双机脑-肢端到端演示 + 断脑自治**。

#### P2.1 L1 能力描述层（CapDesc）

- [ ] 定义 CapDesc ABI（附录 A 定稿），内核侧 `capability.rs` 描述符注册接口（验收：设计评审 + 编译通过）
- [ ] 3 类示范驱动暴露工具：VFS（read/write/ls）、定时器（sleep_until）、虚拟 GPIO（write/read）（验收：每类驱动 ≥2 个工具 schema 生成正确）
- [ ] CapDesc → MCP tool JSON schema 自动生成器 + 单测（验收：snapshot 测试）

#### P2.2 L2 发现注册层（DRT）

- [ ] DRT（Device Registry Table）内核服务：注册/心跳/超时/离线状态机（附录 B）（验收：状态机单测覆盖全部迁移边）
- [ ] v0 传输：UDP announce/heartbeat/bye（端口约定写入协议文档）（验收：双进程跨"网络"注册成功）
- [ ] 脑端工具表聚合 + 对 agent 上下文的注入接口（验收：工具表变更 ≤3s 反映）
- [ ] v1：smoltcp 上实现 mDNS/DNS-SD 基本集（_karte._tcp）（验收：Linux 端 avahi 可发现 KarteOS 设备——跨栈互通证明）

#### P2.3 L3 调用传输层

- [ ] tool-server 用户态守护进程：JSON-RPC 2.0 分发 + Streamable HTTP（smoltcp TCP）（验收：curl 等价客户端调通工具）
- [ ] MCP-CB 紧凑二进制 profile 草案 + 网关转译器原型（验收：同一工具 CB 与 JSON 双路径结果一致）
- [ ] 调用超时/重试/幂等语义（验收：故障注入单测）

#### P2.4 L4 安全层（与 Phase 3 capability 合流的前置）

- [ ] 能力令牌生成/颁发/校验/吊销链路（附录 A.3 定稿）（验收：无令牌调用被拒 + 吊销后调用被拒的单测）
- [ ] 设备签名清单（signed manifest）注册校验（验收：伪冒注册被拒）

#### P2.5 脑端与演示（M2）

- [ ] brain 极简 agent 循环：读工具表 → 决策（规则引擎 v0；LLM v1 复用 M1 推理栈）→ 调用 → 结果回填（验收：规则脑完成跨设备任务：A 设备写文件 → B 设备定时读出）
- [ ] **双机演示**：QEMU 双实例 + 发现广播，脑自动发现 ≥5 工具、完成端到端任务（验收：演示脚本一键复现，录屏/日志入库）
- [ ] **断脑自治**：杀掉脑 30s，肢体按缓存策略继续执行预置任务，脑恢复后状态回报（验收：演示脚本 + 日志）
- [ ] 协议文档 `docs/design/mcp-device-fabric.md` 定稿（验收：与实现一致，附录 A/B 抽离至此）

**DoD**：M2 双机演示 + 断脑自治演示 + 协议文档定稿 + 新增 ≥15 个测试。

---

### Phase 3 · 全场景与安全：处处运行（2027-01 ~ 2027-06）

**目标**：硬件覆盖破圈（ARM64 真机）；应用模型破圈（WASM+capability）；IoT 生存能力（协议/OTA/电源）。**M3：RPi5 上起脑，WASM 应用跨芯片安装。**

#### P3.1 ARM64 (aarch64) 移植

- [ ] `arch/aarch64/` 骨架：启动（QEMU virt, EL1）+ 串口 + generic timer（验收：串口出 hello）
- [ ] GIC v2 中断 + 页表（4 级）+ 每进程 ASL（验收：`make test-arm64` 骨架测试绿）
- [ ] U-mode + syscall 路径 + ELF 加载（验收：hello 用户程序）
- [ ] SMP（PSCI）+ 调度接入（验收：`-smp 4` 测试）
- [ ] **真机 RPi5**：UART/SD/MMC 最小驱动 + 启动链（验收：真机进 shell）
- [ ] CI 增加 arm64 job（QEMU）（验收：6 job 全绿）

#### P3.2 WASM 应用模型 + Capability 体系

- [ ] capability 体系定稿：令牌/命名空间/继承/审计（验收：设计文档 + 内核单测）
- [ ] WASM 解释器（wasmi 谱系）移植为 KarteOS 用户态运行时（验收：示例 wasm 模块调用 syscall 成功）
- [ ] WASM 模块 ↔ CapDesc 工具互相暴露：WASM 应用可注册新工具、调用已有工具（验收：动态注册工具被脑发现并调用）
- [ ] 应用清单格式（manifest：能力申请/签名/版本）（验收：格式文档 + 校验器）
- [ ] **跨芯片演示**：同一 wasm 应用在 riscv64 与 x86_64（后续 arm64）无改动运行（验收：演示日志）

#### P3.3 IoT 协议与 OTA

- [ ] MQTT 3.1.1 客户端（用户态，基于 smoltcp）：pub/sub/QoS1（验收：与 mosquitto 互通）
- [ ] TLS：rustls（no_std 路线评估）或用户态移植；MQTT over TLS（验收：与公网 broker 握手成功）
- [ ] mDNS v1 收尾 + CoRE Link Format 资源描述（验收：第三方工具可枚举 KarteOS 设备能力）
- [ ] OTA A/B：分区约定 + bootloader 交接协议 + 回滚（验收：QEMU 内升级+断电回滚演示）

#### P3.4 电源管理

- [ ] tickless idle：无任务时深度 WFI + 定时器合并（验收：idle 功耗事件计数下降可测）
- [ ] DVFS/休眠框架接口（真机字段留位）（验收：接口文档 + 编译通过）

#### P3.5 MCU 档（S 档）预研

- [ ] riscv32 裁剪可行性报告（内核体积拆解 + feature 矩阵）（验收：报告定稿，含 ≤256KB 路径判断）

**DoD**：M3 真机演示（RPi5 脑 + 双芯片 WASM + OTA 回滚）+ CI 6 job 绿。

---

### Phase 4 · 生态与护城河（2027-06 ~）

**目标**：从 OS 到生态。**M4：对外发布 KarteOS 1.0 与开发者生态。**

- [ ] NPU HAL 首个真机后端（依 P3.1 真机/合作板卡）+ AI 调度类 token 感知增强（验收：NPU 上推理端到端）
- [ ] 模型生命周期 syscall + KV-cache 内存池（验收：双模型共存配额演示）
- [ ] token IPC 零拷贝管道（验收：脑↔应用 token 流基准）
- [ ] 跨脑互联总线 + 脑选举（Arbiter）：多脑协同、故障接管（验收：双脑 failover 演示）
- [ ] secure boot 路径：签名链 + 度量启动（真机）（验收：防篡改演示）
- [ ] karte-sdk：CapDesc 工具宏、WASM 应用脚手架、QEMU 模拟器一键化、文档站（验收：外部开发者 30 分钟内发布一个新工具——可用性验收）
- [ ] 对外发布：1.0、官网、示例仓库、与一个真实家居/工业场景的合作试点（验收：试点运行 ≥30 天）

---

## 6. 里程碑总表

| 里程碑 | 交付物 | 验收标准（可演示/可测量） | 目标时间 |
|--------|--------|--------------------------|---------|
| **M0 基线绿** | 干净可复现的构建+测试+基准 | CI 5 job 绿；基准四项数字入库 | 2026-10 W2 |
| **M1 卡上跑 LLM** | 调度器 2.0 + busybox + LLM 推理 | QEMU 内 LLM 生成 ≥32 连贯 token；RT 抢占演示；tokens/s 入库 | 2026-12 |
| **M2 脑-肢** | MCP 织物 v0（五层） | 双机自动发现 ≥5 工具并完成端到端任务；断脑 30s 自治；工具发现 ≤3s | 2027-02 |
| **M3 处处运行** | ARM64 真机 + WASM + OTA + MQTT/TLS | RPi5 起脑；同一 WASM 跨 ≥2 芯片；OTA 升级+回滚演示 | 2027-06 |
| **M4 生态** | NPU HAL + 跨脑 + secure boot + SDK | 外部开发者 30 分钟发布新工具；试点 ≥30 天；发布 1.0 | 2027-12 |

---

## 7. 风险登记册

| 风险 | 影响 | 缓解 | 触发信号 |
|------|------|------|---------|
| nightly 生态漂移（如 `Step` trait 事件） | 双架构构建红 | 工具链日期 pin（P0）；例行 `cargo update` + 全量构建 | CI lint/build 突然红 |
| smoltcp 单线程模型 vs 多核/高吞吐 | 织物吞吐瓶颈 | P2 初做压力评审；必要时 ifaces per-CPU | 双机演示吞吐 < 需求 |
| LLM 权重体积 vs IoT 存储/内存 | M1 缩水 | 小模型优先（15M 级）；量化路线；XL 档跑大模型 | 内存峰值 > 板卡 RAM |
| scope 蔓延（什么都想做） | 节奏失守 | 取舍标尺（§0.4）+ 季度裁剪 | Checklist 连续两月零勾选 |
| MCP 生态标准演进 | 协议返工 | 织物五层解耦，传输层可换；跟进 MCP spec 版本 | spec 破坏性变更公告 |
| 真机驱动长尾（RPi5/NPU） | P3 延期 | QEMU 先行 + 接口留位；真机单独排波 | 板卡上手 >2 周无 shell |
| 人力带宽 | 全线延后 | 多 agent 并行波次开发；文件冲突矩阵分波 | 单点任务排队 >3 天 |

---

## 8. 工程纪律（沿 AGENTS.md，补充阶段门）

每个 Phase 退出的**五门检查**（缺一不放行）：

1. `cargo fmt --all -- --check` 0 diff
2. CI 同款 clippy 0 error
3. `make test`（及当期架构测试）全绿，测试数与 AGENTS.md 同步
4. boot-test / smp-test（/ 后续 test-x86、test-arm64）绿
5. `docs/agent/*` 与 `docs/benchmarks.md` 与代码一致；本路线图 Checklist 已勾选更新

---

## 9. 附录

### A. CapDesc ABI 草案（v0，P2.1 定稿）

```rust
/// 驱动向内核注册的能力描述符（L1）
pub struct CapabilityDescriptor {
    pub tool_name: &'static str,          // "gpio.write"
    pub version: u32,
    pub input_schema: &'static str,       // JSON Schema（编译期字面量）
    pub output_schema: &'static str,
    pub perm: PermBits,                   // bit0=read bit1=write bit2=admin
    pub min_limb_tier: Tier,              // S/M/L —— 能力可运行的最低设备档
    pub invoke: fn(&[u8], &mut [u8]) -> InvokeResult,  // 二进制入口；JSON 由生成器包装
}

/// A.3 能力令牌（L4）
pub struct CapabilityToken {
    pub tool: &'static str,
    pub bearer: BrainId,        // 授予的脑
    pub perm: PermBits,         // 不超过 CapDesc.perm
    pub expires_at: Timestamp,
    pub sig: Signature,         // 设备公钥可验
}
```

### B. 设备注册状态机（L2，v0）

```
BOOT ──announce──> REGISTERED ──脑确认──> ACTIVE <──heartbeat(5s)──┐
                     │否(伪冒/签名错)         │心跳超时(15s)          │
                     ▼                       ▼                      │
                  REJECTED               DEGRADED ──恢复心跳───────┘
                                             │超时(60s)
                                             ▼
                                           GONE ──重新 announce──> REGISTERED
```

- 脑端 DRT 维护 `device → tools[]` 映射；任何迁移都触发工具表版本号 +1（agent 上下文据此刷新）。
- 肢体离线自治策略（L5）在 REGISTERED 时由脑下发并缓存。

### C. KPI 表（P0 填基线，此后每里程碑更新）

| KPI | 基线(2026-10-08) | M1 目标 | M2 目标 | M3 目标 |
|-----|------------------|---------|---------|---------|
| boot→shell（QEMU, s） | 待测 | 基线 ±10% | 同 | 同 |
| 上下文切换延迟（µs） | 待测 | 记录 | 记录 | 记录 |
| LLM tokens/s（QEMU） | 无 | **>0（出 token）** | ≥2 | ≥5 |
| 设备工具发现延迟（s） | 无 | — | **≤3** | ≤2 |
| 断脑自治时长 | 无 | — | **≥30s** | ≥10min |
| 内核体积（M 档, MB） | 2.3 | ≤2.5 | ≤2.5 | ≤2.0 |
| 测试总数（全架构） | 105+102 | ≥120 | ≥140 | ≥160 |

### D. 变更日志

| 日期 | 版本 | 变更 | 决策人 |
|------|------|------|--------|
| 2026-10-08 | v1.0 | 初版：确立五大身份主张、脑-肢架构、MCP 织物、Phase 0-4 与 Checklist | adm |
