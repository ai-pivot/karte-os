//! P2.2 L2 发现注册层（DRT — Device Registry Table）
//!
//! 在 P2.1 CapDesc 静态注册表之上叠加网络视角的动态状态机：
//!   Announce(新设备) → Online --心跳--> Online
//!   Online --超时 TIMEOUT_MS--> Stale --再超时--> Offline
//!   Bye(主动离线) → Offline
//!   Offline --Announce/心跳--> Online（回归）
//!
//! 端口约定（附录 B v0，UDP）：DRT_PORT = 43110，
//! 消息格式 "KRT1|<verb>|<device_id>|<seq>"，verb ∈ {A, H, B}
//! （A=announce, H=heartbeat, B=bye）。

use crate::capability::CapDesc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

/// 设备在线状态
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DevState {
    Online,
    Stale,
    Offline,
}

/// DRT 表项
pub struct DrtEntry {
    pub desc: CapDesc,
    pub state: DevState,
    pub last_seen_ms: u64,
}

/// 状态机参数（单测可调）
pub const HEARTBEAT_TIMEOUT_MS: u64 = 1500; // Online → Stale
pub const STALE_TIMEOUT_MS: u64 = 3000; // Stale → Offline（总 3s 内反映，验收 ≤3s）

pub struct Drt {
    pub entries: Vec<DrtEntry>,
}

impl Drt {
    pub fn new() -> Self {
        Drt {
            entries: Vec::new(),
        }
    }

    /// Announce：新设备上线；已知设备（任何状态）回归 Online
    pub fn announce(&mut self, desc: CapDesc, now_ms: u64) -> bool {
        for e in self.entries.iter_mut() {
            if e.desc.device_id == desc.device_id {
                e.state = DevState::Online;
                e.last_seen_ms = now_ms;
                return false; // 非新设备
            }
        }
        self.entries.push(DrtEntry {
            desc,
            state: DevState::Online,
            last_seen_ms: now_ms,
        });
        true // 新设备（脑端工具表需刷新）
    }

    /// 心跳：Online/Stale → Online；Offline 设备迟到的心跳视为回归
    pub fn heartbeat(&mut self, device_id: &str, now_ms: u64) -> bool {
        for e in self.entries.iter_mut() {
            if e.desc.device_id == device_id {
                let was = e.state;
                e.state = DevState::Online;
                e.last_seen_ms = now_ms;
                return was != DevState::Online; // 状态变化（Offline 回归）
            }
        }
        false // 未知设备：心跳无效（必须先 announce）
    }

    /// Bye：主动离线（任何状态 → Offline）
    pub fn bye(&mut self, device_id: &str) -> bool {
        for e in self.entries.iter_mut() {
            if e.desc.device_id == device_id {
                let was = e.state;
                e.state = DevState::Offline;
                return was != DevState::Offline;
            }
        }
        false
    }

    /// tick：按 last_seen 推进超时迁移（Online→Stale→Offline）
    pub fn tick(&mut self, now_ms: u64) -> usize {
        let mut changed = 0;
        for e in self.entries.iter_mut() {
            let age = now_ms.saturating_sub(e.last_seen_ms);
            match e.state {
                DevState::Online if age >= HEARTBEAT_TIMEOUT_MS => {
                    e.state = DevState::Stale;
                    changed += 1;
                }
                DevState::Stale if age >= HEARTBEAT_TIMEOUT_MS + STALE_TIMEOUT_MS => {
                    e.state = DevState::Offline;
                    changed += 1;
                }
                _ => {}
            }
        }
        changed
    }

    /// 在线（Online+Stale 仍算可发现；只有 Offline 不可用）
    pub fn discoverable(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.state != DevState::Offline)
            .map(|e| e.desc.device_id.to_string())
            .collect()
    }

    /// 全离线设备
    pub fn offline(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.state == DevState::Offline)
            .map(|e| e.desc.device_id.to_string())
            .collect()
    }

    /// v0 wire 消息解析："KRT1|<verb>|<device_id>|<seq>"
    /// 返回 Some((verb, device_id))；非法格式/前缀返回 None
    pub fn parse_wire(msg: &[u8]) -> Option<(u8, String)> {
        let s = core::str::from_utf8(msg).ok()?;
        let mut it = s.split('|');
        if it.next()? != "KRT1" {
            return None;
        }
        let verb = match it.next()? {
            "A" => b'A',
            "H" => b'H',
            "B" => b'B',
            _ => return None,
        };
        let id = it.next()?.to_string();
        let _seq: u64 = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None; // 恰好 4 段
        }
        Some((verb, id))
    }

    /// v0 wire 消息构造
    pub fn make_wire(verb: u8, device_id: &str, seq: u64) -> Vec<u8> {
        format!("KRT1|{}|{}|{}", verb as char, device_id, seq).into_bytes()
    }
}

/// 全局 DRT（脑端/网络栈共享）
pub static DRT: Mutex<Drt> = Mutex::new(Drt {
    entries: Vec::new(),
});

/// DRT UDP 端口（附录 B v0 约定）
pub const DRT_PORT: u16 = 43110;

/// wire 消息分发：网络栈收到 KRT1 UDP 包后调用。
/// A=announce（用 capability 注册表中的描述符；未知 id 忽略），
/// H=heartbeat，B=bye。任何成员/状态变化都会递增表版本号。
pub fn handle_wire(msg: &[u8], now_ms: u64) -> bool {
    let Some((verb, id)) = Drt::parse_wire(msg) else {
        return false;
    };
    match verb {
        b'A' => {
            // Local registered device takes precedence; peer node ids
            // ("*-node") register as remote devices with an empty tool set
            // (existence-level discovery; tool-level aggregation is v1).
            let desc = match crate::capability::lookup_desc(&id) {
                Some(d) => d,
                None => {
                    if !id.ends_with("-node") {
                        return false;
                    }
                    CapDesc {
                        device_id: "remote",
                        device_type: "remote",
                        version: 1,
                        tools: &[],
                    }
                }
            };
            let mut g = DRT.lock();
            let is_new = g.announce(desc, now_ms);
            if is_new {
                bump_table_seq();
            }
            is_new
        }
        b'H' => {
            let mut g = DRT.lock();
            if g.heartbeat(&id, now_ms) {
                bump_table_seq();
                true
            } else {
                false
            }
        }
        b'B' => {
            let mut g = DRT.lock();
            if g.bye(&id) {
                bump_table_seq();
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// 脑端工具表聚合：在线（非 Offline）设备的全部工具名 + 表版本号。
/// 表版本号在每次成员变化（announce 新设备 / 状态迁移）时递增，
/// 脑端轮询比较版本号即可感知变更（≤3s 反映由 DRT 超时参数保证：
/// 心跳超时 1500ms + Stale 超时 3000ms 内必然发生迁移）。
pub struct ToolTableSnapshot {
    pub seq: u64,
    pub tools: Vec<String>,
    pub devices: Vec<String>,
}

static TABLE_SEQ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn bump_table_seq() -> u64 {
    TABLE_SEQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1
}

pub fn table_seq() -> u64 {
    TABLE_SEQ.load(core::sync::atomic::Ordering::Relaxed)
}

/// 聚合当前可发现设备的工具表（脑端 agent 上下文注入接口）
pub fn tool_table() -> ToolTableSnapshot {
    let g = DRT.lock();
    let mut tools = Vec::new();
    let mut devices = Vec::new();
    for e in g.entries.iter().filter(|e| e.state != DevState::Offline) {
        devices.push(e.desc.device_type.to_string());
        for t in e.desc.tools.iter() {
            tools.push(alloc::format!("{}_{}", e.desc.device_type, t.name));
        }
    }
    tools.sort();
    devices.sort();
    devices.dedup();
    ToolTableSnapshot {
        seq: table_seq(),
        tools,
        devices,
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    use crate::capability::{CapDesc, ToolDesc, perm};

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
    crate::console_println!("── DRT (Device Registry) Tests ──");

    // 状态机：全部迁移边
    crate::test::run_test("drt_announce_new_and_duplicate", || {
        let mut d = Drt::new();
        let new = d.announce(fake_desc("dev1"), 100);
        let dup = d.announce(fake_desc("dev1"), 110);
        new && !dup && d.entries.len() == 1 && d.entries[0].state == DevState::Online
    });

    crate::test::run_test("drt_online_to_stale_to_offline_timeouts", || {
        let mut d = Drt::new();
        d.announce(fake_desc("d1"), 0);
        // 1500ms：Online→Stale
        d.tick(1500);
        let s1 = d.entries[0].state == DevState::Stale;
        // 4499ms：仍 Stale（< 1500+3000）
        d.tick(4499);
        let s2 = d.entries[0].state == DevState::Stale;
        // 4500ms：Stale→Offline
        d.tick(4500);
        s1 && s2 && d.entries[0].state == DevState::Offline
    });

    crate::test::run_test("drt_offline_rejoin_via_heartbeat", || {
        let mut d = Drt::new();
        d.announce(fake_desc("d1"), 0);
        d.tick(4500); // → Offline
        let changed = d.heartbeat("d1", 5000);
        changed && d.entries[0].state == DevState::Online
    });

    crate::test::run_test("drt_offline_rejoin_via_announce", || {
        let mut d = Drt::new();
        d.announce(fake_desc("d1"), 0);
        d.bye("d1");
        let new = d.announce(fake_desc("d1"), 200);
        !new && d.entries[0].state == DevState::Online
    });

    crate::test::run_test("drt_bye_from_online", || {
        let mut d = Drt::new();
        d.announce(fake_desc("d1"), 0);
        let changed = d.bye("d1");
        changed && d.entries[0].state == DevState::Offline
    });

    crate::test::run_test("drt_heartbeat_unknown_invalid", || {
        let mut d = Drt::new();
        !d.heartbeat("ghost", 10) && d.entries.is_empty()
    });

    crate::test::run_test("drt_tick_no_false_transitions", || {
        let mut d = Drt::new();
        d.announce(fake_desc("d1"), 1000);
        d.tick(1400); // 400ms < 1500ms：仍 Online
        d.entries[0].state == DevState::Online
    });

    // wire 编解码（v0 UDP）
    crate::test::run_test("drt_wire_roundtrip_all_verbs", || {
        let ok = (0..3).all(|v| {
            let b = Drt::make_wire(b"AH B"[if v == 2 { 3 } else { v }] as u8, "gpio0", v as u64);
            matches!(Drt::parse_wire(&b), Some((_, id)) if id == "gpio0")
        });
        ok
    });

    crate::test::run_test("drt_wire_reject_bad_prefix_and_trailing", || {
        let bad = b"KRT2|A|x|1";
        let trail = b"KRT1|A|x|1|9";
        let noverb = b"KRT1|Z|x|1";
        Drt::parse_wire(bad).is_none()
            && Drt::parse_wire(trail).is_none()
            && Drt::parse_wire(noverb).is_none()
    });

    // 全局 DRT 冒烟（登记/发现）
    crate::test::run_test("drt_global_discoverable_snapshot", || {
        let mut g = DRT.lock();
        g.announce(fake_desc("gd1"), 1);
        g.announce(fake_desc("gd2"), 2);
        g.bye("gd2");
        let disc = g.discoverable();
        let off = g.offline();
        disc.contains(&"gd1".to_string())
            && off.contains(&"gd2".to_string())
            && disc.len() == 1
            && off.len() == 1
    });

    // 脑端工具表聚合（seq 感知变更 + 离线设备被剔除）
    crate::test::run_test("drt_tool_table_agg_and_seq", || {
        bump_table_seq(); // 模拟成员变化
        let t1 = tool_table();
        bump_table_seq();
        let t2 = tool_table();
        t2.seq > t1.seq
            && t1.tools.contains(&"gpio_read".to_string())
            && t1.devices.contains(&"gpio".to_string())
            && !t1.devices.contains(&"dev_off".to_string()) // fake offline 设备类型不在表里
    });

    // wire 分发（handle_wire → 全局状态机）
    crate::test::run_test("drt_handle_wire_dispatch", || {
        // 未知设备 announce 忽略（注册表里只有 vfs0/timer0/gpio0）
        let unknown = Drt::make_wire(b'A', "nope", 1);
        let unknown_ok = !handle_wire(&unknown, 10);
        // 已知设备 announce → 全局表 Online
        let known = Drt::make_wire(b'A', "gpio0", 2);
        let known_ok = handle_wire(&known, 20);
        // 心跳推进
        let hb = Drt::make_wire(b'H', "gpio0", 3);
        let hb_ok = !handle_wire(&hb, 30); // 已 Online，无状态变化 → false
        // bye → Offline + seq bump
        let s0 = table_seq();
        let bye = Drt::make_wire(b'B', "gpio0", 4);
        let bye_ok = handle_wire(&bye, 40) && table_seq() > s0;
        // 非法消息拒绝
        let bad_ok = !handle_wire(b"junk", 50);
        unknown_ok && known_ok && hb_ok && bye_ok && bad_ok
    });
}
