# KarteOS 基准库（Benchmarks）

> 原则：每个数字必须可复现（附测量命令/方法）；每次里程碑更新一列；环境差异必须标注。
> 测量环境：2026-10-08，QEMU 6.2.0 TCG（Ubuntu 22.04 容器，x86_64 宿主），Rust stable 1.93.1 / nightly-2026-10-07。

## 基线（2026-10-08，Phase 0）

| KPI | 数值 | 测量方法 | 备注 |
|-----|------|----------|------|
| boot→shell（RISC-V, QEMU TCG） | **67 ms** | QEMU `-serial file:`（无缓冲）+ 20ms 轮询 `KarteOS Shell`，含 OpenSBI + 10 阶段 init + shell banner | 管道 stdio 法因块缓冲失真（测得 15s），弃用 |
| 测试套件总耗时（105 项，RISC-V） | **67 ms** | 同上，轮询 `TEST_RESULT` | 纯内核态测试，无用户进程 |
| 内核体积（RISC-V release） | **2,366,560 B ≈ 2.26 MB** | `ls -l target/riscv64gc-unknown-none-elf/release/karte-os-kernel` | 含内嵌用户程序 ELF |
| 内核体积（x86_64 release） | **1,699,344 B ≈ 1.62 MB** | `ls -l target/x86_64-unknown-none/release/karte-os-kernel` | Multiboot2 ELF |
| 上下文切换延迟 | TBD | P1.1 调度器 2.0 交付基准 harness 后补测 | 需要真实双任务 ping-pong 场景 |
| RT pick 决策延迟 | **128 cycles** | test kernel `run_tests` 内 rdcycle 打点：1000 次空队列 `SCHEDULER.lock()+ready.pop_next()` 平均 | QEMU TCG 虚拟时钟（rdcycle），2026-10-08（P1.1 d2e5ad2 系列）。完整 RT 抢占延迟上界 = timer tick 粒度（~10ms）+ pick + `__switch` |
| clippy error | **0** | CI 同款 clippy 命令（见 AGENTS.md Testing） | 2026-10-08 清零 |
| 测试 | RISC-V 112/112；x86_64 138/138 | `make test` / `make test-x86` | x86_64 需 `grub-pc-bin` 已装 |

## 复现命令

```bash
# boot→shell（把 <Q> 换成内核路径）
qemu-system-riscv64 -machine virt -cpu rv64 -bios default -display none -m 128M \
  -drive id=blk0,file=disk.img,format=raw,if=none -device virtio-blk-device,drive=blk0 \
  -netdev user,id=net0 -device virtio-net-device,netdev=net0 \
  -kernel <Q> -serial file:/tmp/bt.log &
# 轮询 /tmp/bt.log 直到出现 "KarteOS Shell"，墙钟差即结果

# 测试套件耗时：make build-test 后同法轮询 "TEST_RESULT"
```

## 历史列（每里程碑追加）

| KPI | P0 基线 (2026-10-08) | M1 目标 | M2 目标 |
|-----|----------------------|---------|---------|
| boot→shell (ms) | 67 | 基线 ±10% | 基线 ±10% |
| 上下文切换 (µs) | TBD | 记录 | 记录 |
| LLM tokens/s | ~0.1 (TCG 软浮点实测) | >0 (机制验证✓) | ≥2 (KV cache+QEMU7+/KVM) |
| clippy error | 0 | 0 | 0 |

## LLM 推理（P1.3 M1，2026-10-08 实测）

- **模型**：char-GPT 0.81M 参数（V=65, D=128, L=4, T=64, F=512）；weights.bin 3.24MB 经 `include_bytes!` 嵌入 llm.elf（rodata）
- **tokens/s 实测**：**~0.1 tok/s** @ QEMU 6.2 TCG 软浮点（全块 forward）；第一个 token 的 forward ~90s（`[llm] gen tick` 计时法）；60min/120min 终验均未跑完 32 token
- **内存峰值**：llm.elf 3,943,544 B（含 3.24MB 权重 rodata）+ .bss 384KB（X/H/QKV/ATT/YY/F1 静态缓冲）+ 用户栈 2MB（每进程预映射）
- **方法**：`--features llm_demo` boot 自跑（规避 QEMU stdin 时序不可靠）→ UART 日志 gen tick 间隔推算 tokens/s → 日志入库
- **提速路径**：KV-cache 单 token 推理（~9x MAC 削减，已实现，fault 谜团 parked 见 AGENTS.md）；QEMU 7+ 的 RVV 或 KVM 加速
- **机制验证链**：exec 流式加载 3.9MB ELF ✓ → FPU_OK（软浮点）✓ → forward 真实运行（gen tick）✓ → 温度 0.8 + CDF 采样 + CHARSET 解码就绪 ✓
