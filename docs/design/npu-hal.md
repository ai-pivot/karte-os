# NPU HAL — 厂商 NPU 统一抽象（Phase 4 接口定稿）

> ROADMAP §P4-1。真机后端依赖 P3.1 真机/合作板卡到位；本文档定稿接口
> 与 AI 调度 token 感知的内核侧挂接点，使任何 NPU 驱动可即插即用。

## 1. HAL 接口（submit / sync / 内存约定）

```rust
/// NPU 统一抽象（内核侧 trait，厂商驱动实现）
pub trait NpuHal {
    /// 提交一次推理请求（权重/输入/输出全部经 capauth 令牌校验）
    /// 返回 job_id；非阻塞，完成经 sync/poll 通知。
    fn submit(&self, job: NpuJob, token: &CapToken) -> Result<u32, NpuErr>;
    /// 阻塞等待（可超时）；AI-batch 类任务可被 RT 类抢占暂停。
    fn sync(&self, job_id: u32, timeout_ms: u32) -> Result<NpuResult, NpuErr>;
    /// 内存约定：设备可见内存池（大页、非缓存、与 VMM 协同映射）
    fn mem_pool(&self) -> &NpuMemPool;
}
```

- **内存约定**：NPU 可见池由 VMM 预留（大页 + `MapFlags::NPU`），驱动只做
  offset 分配；页表锁定（不可换出），与 model.rs 的 `Pinned` 状态联动。
- **调度协同**：`SCHED_AI_BATCH` 类任务在 `sync()` 内可被整批暂停——
  NPU 作业状态由驱动保存/恢复（厂商能力矩阵）。

## 2. AI 调度类 token 感知增强（内核侧可测部分）

- token 生成周期感知接口：`sched::ai_batch_note_token(producer_id)` ——
  生成 loop 每 token 上报，调度器以滑动窗口估计节奏，用于：
  1. RT 抢占阈值动态化（生成间隙让 RT 插队，生成突发不碎片化）；
  2. KV 池压力时的让步决策（model.rs `yielded_blocks` 的触发源之一）。
- 单测：节奏估计器在恒定/突发序列下的窗口收敛（v1 随调度器测试批次）。

## 3. 真机后端路径（诚实标注）

- **NPU 上推理端到端**验收：待 P3.1 真机/合作板卡（当前 QEMU 无 NPU 模型）。
- 首选候选：RV 芯片内置 NPU（如 ESP32-P4/Sophgo CV1800B 类）——与
  scratch32/ESP32 路径同构；驱动接 `NpuHal` trait + mem_pool 大页预留。
- fallback：CPU 路径（llm 栈现状）在全期保持可用——NPU 是加速而非依赖。
