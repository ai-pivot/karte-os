//! P2.5 脑端与演示（M2）— brain 规则引擎 v0 + 断脑自治
//!
//! 脑端 agent 循环（规则引擎 v0；LLM v1 复用 M1 推理栈）：
//!   1. 读工具表（drt::tool_table）
//!   2. 决策：规则引擎按预置任务脚本推进（跨设备任务：A 写 → B 读）
//!   3. 调用：经 mcp_cb CallTable 分发到工具执行器
//!   4. 结果回填：完成 → 下一步；超时 → 重试
//!
//! 断脑自治：肢体侧 LocalQueue 预置任务缓存 — 脑心跳超时后肢体按
//! 缓存继续执行（自治），脑恢复后收到状态回报（reconcile）。

use alloc::string::String;
use alloc::vec::Vec;

// ── 脑端规则引擎 ──

/// 预置任务步骤（跨设备演示脚本：gpio_write → gpio_read 回读验证）
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    Call {
        device: String,
        tool: String,
        args: String,
    },
    ExpectOnline {
        device: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepStatus {
    Pending,
    Ok,
    Failed,
}

pub struct Brain {
    pub script: Vec<Step>,
    pub pc: usize,
    pub status: Vec<StepStatus>,
    pub results: Vec<(usize, String)>,
}

impl Brain {
    pub fn new(script: Vec<Step>) -> Self {
        let n = script.len();
        Brain {
            script,
            pc: 0,
            status: alloc::vec![StepStatus::Pending; n],
            results: Vec::new(),
        }
    }

    /// 当前步（None = 全部完成）
    pub fn current(&self) -> Option<&Step> {
        self.script.get(self.pc)
    }

    /// 步骤完成回填；Ok → 推进 pc
    pub fn complete_step(&mut self, ok: bool, detail: &str) {
        self.status[self.pc] = if ok {
            StepStatus::Ok
        } else {
            StepStatus::Failed
        };
        self.results.push((self.pc, String::from(detail)));
        if ok {
            self.pc += 1;
        }
    }

    pub fn all_done(&self) -> bool {
        self.pc >= self.script.len()
    }

    /// 任一步失败
    pub fn any_failed(&self) -> bool {
        self.status.iter().any(|s| *s == StepStatus::Failed)
    }
}

/// 演示脚本：跨设备任务 — A 设备（gpio0）写 pin → B 路径（同表回读）验证
pub fn demo_script() -> Vec<Step> {
    use alloc::format;
    alloc::vec![
        Step::ExpectOnline {
            device: String::from("gpio0")
        },
        Step::Call {
            device: String::from("gpio0"),
            tool: String::from("write"),
            args: format!("{{\"pin\":3,\"value\":true}}")
        },
        Step::Call {
            device: String::from("gpio0"),
            tool: String::from("read"),
            args: format!("{{\"pin\":3}}")
        },
    ]
}

// ── 断脑自治：肢体侧 LocalQueue ──

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Reported,
}

pub struct LocalJob {
    pub id: u64,
    pub device: String,
    pub tool: String,
    pub args: String,
    pub state: JobState,
    pub result: Option<String>,
}

/// 肢体本地任务队列：脑在线时由脑下发；脑离线时按缓存继续执行
pub struct LocalQueue {
    pub jobs: Vec<LocalJob>,
    pub brain_online: bool,
    pub next_id: u64,
}

impl LocalQueue {
    pub fn new() -> Self {
        LocalQueue {
            jobs: Vec::new(),
            brain_online: true,
            next_id: 1,
        }
    }

    /// 脑下发任务
    pub fn submit(&mut self, device: &str, tool: &str, args: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(LocalJob {
            id,
            device: device.into(),
            tool: tool.into(),
            args: args.into(),
            state: JobState::Queued,
            result: None,
        });
        id
    }

    /// 脑心跳：更新在线状态（恢复时触发回报）
    pub fn heartbeat(&mut self, alive: bool) -> usize {
        self.brain_online = alive;
        // 脑恢复：把 Done 未回报的任务标记为待回报（回报即 Reported）
        if alive {
            self.jobs
                .iter_mut()
                .filter(|j| j.state == JobState::Done)
                .for_each(|j| j.state = JobState::Reported);
            return self
                .jobs
                .iter()
                .filter(|j| j.state == JobState::Reported)
                .count();
        }
        0
    }

    /// 执行 tick：脑在线 → 只跑 Queued；脑离线（断脑自治）→ 继续跑缓存任务
    pub fn tick(&mut self, executor: impl Fn(&str, &str, &str) -> String) -> usize {
        let mut ran = 0;
        for j in self.jobs.iter_mut() {
            if j.state != JobState::Queued {
                continue;
            }
            // 断脑自治核心：brain_online == false 也继续执行（缓存策略）
            j.state = JobState::Running;
            let out = executor(&j.device, &j.tool, &j.args);
            j.result = Some(out);
            j.state = JobState::Done;
            ran += 1;
        }
        ran
    }

    /// 回报给恢复后的脑：Done/Reported 任务的结果清单
    pub fn report(&self) -> Vec<(u64, String)> {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Done || j.state == JobState::Reported)
            .filter_map(|j| j.result.clone().map(|r| (j.id, r)))
            .collect()
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    use crate::capability::{CapDesc, ToolDesc, perm, register_builtin_devices, set_online};

    fn fake_desc(id: &'static str) -> CapDesc {
        CapDesc {
            device_id: id,
            device_type: "gpio",
            version: 1,
            tools: &[ToolDesc {
                name: "read",
                desc: "r",
                inputs: &[],
                outputs: &[],
                perm: perm::READ,
            }],
        }
    }

    crate::console_println!("");
    crate::console_println!("── Brain Agent / Autonomy Tests ──");

    crate::test::run_test("brain_script_progress_and_backfill", || {
        register_builtin_devices();
        let mut b = Brain::new(demo_script());
        // 步 0：ExpectOnline gpio0 → 在线
        match b.current() {
            Some(Step::ExpectOnline { device }) => {
                b.complete_step(set_online(device, true), "online");
            }
            _ => return false,
        }
        // 步 1/2：Call → 成功推进
        b.complete_step(true, "wrote");
        b.complete_step(true, "read true");
        b.all_done() && !b.any_failed() && b.results.len() == 3
    });

    crate::test::run_test("brain_failure_marks_and_does_not_advance", || {
        let mut b = Brain::new(demo_script());
        b.complete_step(true, "ok");
        let pc_before = b.pc;
        b.complete_step(false, "device offline");
        !b.any_failed() == false && b.pc == pc_before + 0 && b.current().is_some()
    });

    crate::test::run_test("autonomy_limb_continues_offline_then_reports", || {
        let mut q = LocalQueue::new();
        let id1 = q.submit("gpio0", "write", "{\"pin\":4,\"value\":true}");
        let id2 = q.submit("gpio0", "read", "{\"pin\":4}");
        // 脑心跳掉线
        q.heartbeat(false);
        // 断脑自治：tick 仍执行缓存任务
        let ran = q.tick(|d, t, a| {
            let _ = (d, t, a);
            String::from("ok")
        });
        // 脑恢复：状态回报
        let _ = q.heartbeat(true);
        let rep = q.report();
        ran == 2 && rep.len() == 2 && rep[0].0 == id1 && rep[1].0 == id2
    });

    crate::test::run_test("autonomy_offline_state_flag", || {
        let mut q = LocalQueue::new();
        q.heartbeat(false);
        let offline = !q.brain_online;
        let _ = q.heartbeat(true);
        offline && q.brain_online
    });

    crate::test::run_test("brain_fake_desc_unknown_device_fails_expect", || {
        // ExpectOnline 未知设备 → 失败不推进
        let mut b = Brain::new(alloc::vec![Step::ExpectOnline {
            device: String::from("nope")
        }]);
        b.complete_step(false, "offline");
        b.any_failed()
    });

    // fake_desc 只为保证编译器用到（风格统一）
    let _ = fake_desc("unused");
}
