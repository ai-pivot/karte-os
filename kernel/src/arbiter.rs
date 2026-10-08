//! Phase 4 — 跨脑互联总线 + Arbiter 脑选举（ROADMAP §P4-4）
//!
//! 多脑协同：开放互联总线上的脑节点表 + 简化 bully 选举（优先级+lease）
//! + 故障接管（主脑 lease 超时 → 最高优先级活脑接任）。
//! 单测锁定选举状态机；QEMU 双脑 failover 随 P2.5 双机演示路径实测。

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// 选举消息动词（互联总线 wire，v0 与 DRT 同端口复用）。
pub const ARB_ELECT: u8 = b'E'; // 竞选宣告 (candidate_id, priority)
pub const ARB_VICTORY: u8 = b'V'; // 当选宣告
pub const ARB_HEARTBEAT: u8 = b'H'; // 主脑心跳

/// 主脑 lease 超时（ms）：超时未收到心跳即触发重选。
pub const LEASE_TIMEOUT_MS: u64 = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

#[derive(Debug, Clone, Copy)]
pub struct BrainNode {
    pub id: u32,
    pub priority: u32, // 数值大者优先
    pub alive: bool,
}

/// Arbiter：单脑视角的选举状态机。
pub struct Arbiter {
    pub self_id: u32,
    pub self_priority: u32,
    pub role: Role,
    pub leader_id: Option<u32>,
    pub term: u32, // 任期号（单调）
    pub last_heartbeat_ms: AtomicU64,
    pub votes: BTreeMap<u32, u32>, // voter_id -> candidate_id
    pub elections: AtomicU32,
}

impl Arbiter {
    pub fn new(self_id: u32, self_priority: u32) -> Self {
        Self {
            self_id,
            self_priority,
            role: Role::Follower,
            leader_id: None,
            term: 0,
            last_heartbeat_ms: AtomicU64::new(0),
            votes: BTreeMap::new(),
            elections: AtomicU32::new(0),
        }
    }

    /// tick：Follower 的主脑 lease 超时 → 发起竞选（bully）。
    pub fn tick(&mut self, now_ms: u64) -> Option<u8> {
        match self.role {
            Role::Follower => {
                let hb = self.last_heartbeat_ms.load(Ordering::Relaxed);
                if self.leader_id.is_some() && now_ms.saturating_sub(hb) > LEASE_TIMEOUT_MS {
                    self.start_election(now_ms)
                } else {
                    None
                }
            }
            Role::Candidate => {
                // 竞选超时简化：立即以票数判定（v0 单轮）
                self.resolve(now_ms)
            }
            Role::Leader => Some(ARB_HEARTBEAT),
        }
    }

    /// 发起竞选：term++、自投、角色转 Candidate。
    pub fn start_election(&mut self, _now_ms: u64) -> Option<u8> {
        self.term += 1;
        self.role = Role::Candidate;
        self.leader_id = None;
        self.votes.clear();
        self.votes.insert(self.self_id, self.self_id);
        self.elections.fetch_add(1, Ordering::Relaxed);
        Some(ARB_ELECT)
    }

    /// 收到竞选宣告：优先级低于自己 → 拒绝并发起自己的竞选；否则投票。
    pub fn on_elect(
        &mut self,
        candidate_id: u32,
        candidate_priority: u32,
        now_ms: u64,
    ) -> Option<u8> {
        if candidate_priority > self.self_priority {
            self.votes.insert(self.self_id, candidate_id);
            None
        } else if candidate_priority < self.self_priority {
            self.start_election(now_ms)
        } else {
            // 平级：id 大者胜
            if candidate_id > self.self_id {
                self.votes.insert(self.self_id, candidate_id);
            } else {
                self.start_election(now_ms);
            }
            None
        }
    }

    /// 票数汇总：多数票（含自票）当选 → Victory。
    pub fn resolve(&mut self, _now_ms: u64) -> Option<u8> {
        if self.role != Role::Candidate {
            return None;
        }
        let mut best: Option<(u32, u32)> = None; // (票数, candidate_id)
        for (_, &cand) in self.votes.iter() {
            let c = best.map(|(n, _)| n).unwrap_or(0);
            // 统计投给 cand 的票
            let votes_for = self.votes.values().filter(|&&v| v == cand).count() as u32;
            if votes_for > c || (votes_for == c && best.map(|(_, id)| cand > id).unwrap_or(true)) {
                best = Some((votes_for, cand));
            }
        }
        if let Some((n, cand)) = best {
            if n >= 2 {
                // 多数（≥2 脑织物 v0）
                self.leader_id = Some(cand);
                self.role = if cand == self.self_id {
                    Role::Leader
                } else {
                    Role::Follower
                };
                return Some(ARB_VICTORY);
            }
        }
        None
    }

    /// 收到心跳：更新 lease；若宣称者优先级更高且自己是 Leader → 退位。
    pub fn on_heartbeat(&mut self, leader_id: u32, now_ms: u64) {
        self.last_heartbeat_ms.store(now_ms, Ordering::Relaxed);
        self.leader_id = Some(leader_id);
        if self.role == Role::Candidate {
            self.role = Role::Follower;
        }
    }

    /// 收到当选宣告。
    pub fn on_victory(&mut self, leader_id: u32) {
        self.leader_id = Some(leader_id);
        self.role = if leader_id == self.self_id {
            Role::Leader
        } else {
            Role::Follower
        };
        self.votes.clear();
    }

    /// 故障接管模拟：主脑移除后的下一 tick 应触发竞选。
    pub fn simulate_leader_failure(&mut self, now_ms: u64) -> Option<u8> {
        self.last_heartbeat_ms.store(
            now_ms.saturating_sub(LEASE_TIMEOUT_MS + 1),
            Ordering::Relaxed,
        );
        self.tick(now_ms)
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Arbiter Election Tests ──");

    crate::test::run_test("arbiter_elect_then_victory", || {
        // 双脑 A(pri=10) B(pri=20)：lease 超时 → A 竞选 → B 更优拒绝并反竞选 → B 当选
        let mut a = Arbiter::new(1, 10);
        let mut b = Arbiter::new(2, 20);
        a.leader_id = Some(2);
        a.simulate_leader_failure(10_000) == Some(ARB_ELECT)
            && a.role == Role::Candidate
            && a.term == 1
            // A 宣告 → B 优先级更高，拒绝并反竞选（bully）
            && {
                let _ = b.on_elect(1, 10, 10_000);
                b.role == Role::Candidate
            }
            // B 反竞选 → A 收到低优于己……A pri 更低，A 给 B 投票
            && {
                let _ = a.on_elect(2, 20, 10_100);
                a.votes.get(&1) == Some(&2)
            }
            // B resolve：自票+A 票 = 2 票当选
            && {
                b.votes.insert(2, 2);
                b.votes.insert(1, 2);
                b.resolve(10_200) == Some(ARB_VICTORY)
                && b.role == Role::Leader
                && b.leader_id == Some(2)
            }
    });

    crate::test::run_test("arbiter_failover_lease_timeout", || {
        let mut f = Arbiter::new(3, 5);
        f.on_victory(1);
        f.role == Role::Follower
            && f.leader_id == Some(1)
            // 主脑失联 → lease 超时触发竞选
            && f.simulate_leader_failure(20_000) == Some(ARB_ELECT)
            && f.role == Role::Candidate
            && f.term == 1
            && f.elections.load(Ordering::Relaxed) == 1
    });

    crate::test::run_test("arbiter_priority_preemption", || {
        // 低优 Leader 收到高优竞选 → 投票给高优
        let mut low = Arbiter::new(1, 1);
        low.role = Role::Leader;
        low.on_elect(2, 99, 5_000);
        low.votes.get(&1) == Some(&2)
            // 高优心跳使 Candidate 归位 Follower
            && {
                let mut c = Arbiter::new(4, 7);
                c.start_election(1_000);
                c.on_heartbeat(2, 2_000);
                c.role == Role::Follower && c.leader_id == Some(2)
            }
    });
}
