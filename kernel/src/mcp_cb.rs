//! P2.3 L3 调用传输层 — MCP-CB 紧凑二进制 profile + 调用语义
//!
//! MCP-CB（compact binary）v1 wire（附录 A.2）：
//!   `MCB1 | seq(u8 LE) | name_len(u8) | name | args_len(u16 LE) | args_json`
//! 网关转译器在 CB 与 JSON-RPC 之间双向翻译，保证同一工具调用双路径
//! 结果一致（验收：等价性单测）。
//!
//! 调用语义（验收：故障注入单测）：
//!   CallTracker 以 seq 为幂等键 — 重复调用直接返回首个结果（幂等）；
//!   now_ms 超过 deadline 判 TIMED_OUT；完成后状态 Done 缓存结果。

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const CB_MAGIC: &[u8; 4] = b"MCB1";

/// JSON-RPC 请求 → MCP-CB 帧（网关转译：JSON 侧 → 设备侧）
pub fn json_to_cb(json_req: &[u8], seq: u8) -> Option<Vec<u8>> {
    let name = json_str(json_req, "name")?;
    let args = find_arguments(json_req).unwrap_or(b"{}");
    let mut out = Vec::new();
    out.extend_from_slice(CB_MAGIC);
    out.push(seq);
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    out.extend_from_slice(&(args.len() as u16).to_le_bytes());
    out.extend_from_slice(args);
    Some(out)
}

/// MCP-CB 帧 → (seq, name, args_json)（设备侧解析）
pub fn cb_parse(frame: &[u8]) -> Option<(u8, String, Vec<u8>)> {
    if frame.len() < 8 || &frame[0..4] != CB_MAGIC {
        return None;
    }
    let seq = frame[4];
    let name_len = frame[5] as usize;
    if frame.len() < 6 + name_len + 2 {
        return None;
    }
    let name = core::str::from_utf8(&frame[6..6 + name_len])
        .ok()?
        .to_string();
    let args_len = u16::from_le_bytes([frame[6 + name_len], frame[7 + name_len]]) as usize;
    if frame.len() < 8 + name_len + args_len {
        return None;
    }
    let args = frame[8 + name_len..8 + name_len + args_len].to_vec();
    Some((seq, name, args))
}

/// 工具结果 → MCP-CB 响应帧：`MCB1 | seq | status(1) | payload_len(u16) | payload`
/// status: 0=OK, 1=TOOL_ERR, 2=TIMEOUT
pub fn result_to_cb(seq: u8, status: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(CB_MAGIC);
    out.push(seq);
    out.push(status);
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

// ── 调用语义：幂等 + 超时 ──

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallState {
    InFlight,
    Done,
    TimedOut,
}

pub struct CallTracker {
    pub seq: u8,
    pub state: CallState,
    pub deadline_ms: u64,
    pub cached_result: Option<Vec<u8>>,
}

pub struct CallTable {
    pub calls: Vec<CallTracker>,
    pub timeout_ms: u64,
}

impl CallTable {
    pub fn new(timeout_ms: u64) -> Self {
        CallTable {
            calls: Vec::new(),
            timeout_ms,
        }
    }

    /// 开始调用：同 seq 的 InFlight/Done 调用是重复 — 幂等返回 false。
    /// 超时后的同 seq 调用允许重试（旧记录被替换）。
    pub fn begin(&mut self, seq: u8, now_ms: u64) -> bool {
        self.sweep(now_ms);
        if let Some(c) = self.calls.iter_mut().find(|c| c.seq == seq) {
            if c.state == CallState::InFlight || c.state == CallState::Done {
                return false; // 幂等拒绝
            }
            // TimedOut → 重开
            c.state = CallState::InFlight;
            c.deadline_ms = now_ms + self.timeout_ms;
            c.cached_result = None;
            return true;
        }
        self.calls.push(CallTracker {
            seq,
            state: CallState::InFlight,
            deadline_ms: now_ms + self.timeout_ms,
            cached_result: None,
        });
        true
    }

    /// 完成调用：缓存结果（幂等重放用）
    pub fn complete(&mut self, seq: u8, result: &[u8]) -> bool {
        if let Some(c) = self
            .calls
            .iter_mut()
            .find(|c| c.seq == seq && c.state == CallState::InFlight)
        {
            c.state = CallState::Done;
            c.cached_result = Some(result.to_vec());
            return true;
        }
        false
    }

    /// 超时扫描：过期 InFlight → TimedOut
    pub fn sweep(&mut self, now_ms: u64) -> usize {
        let mut n = 0;
        for c in self.calls.iter_mut() {
            if c.state == CallState::InFlight && now_ms >= c.deadline_ms {
                c.state = CallState::TimedOut;
                n += 1;
            }
        }
        n
    }

    /// 幂等查询：Done 调用重放缓存结果
    pub fn replay(&self, seq: u8) -> Option<&[u8]> {
        self.calls
            .iter()
            .find(|c| c.seq == seq && c.state == CallState::Done)
            .and_then(|c| c.cached_result.as_deref())
    }
}

// ── JSON 提取 helper（与 toolserver 语义一致的最小实现） ──

fn json_str<'a>(json: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let pat = format!("\"{}\":\"", key);
    let pat = pat.as_bytes();
    let mut i = 0;
    while i + pat.len() <= json.len() {
        if &json[i..i + pat.len()] == pat {
            let start = i + pat.len();
            let mut end = start;
            while end < json.len() && json[end] != b'"' {
                end += 1;
            }
            return Some(&json[start..end]);
        }
        i += 1;
    }
    None
}

fn find_arguments(buf: &[u8]) -> Option<&[u8]> {
    let key = b"\"arguments\":";
    let mut i = 0;
    while i + key.len() <= buf.len() {
        if &buf[i..i + key.len()] == key {
            let mut j = i + key.len();
            while j < buf.len() && buf[j] != b'{' {
                j += 1;
            }
            if j >= buf.len() {
                return None;
            }
            let start = j;
            let mut depth = 0usize;
            let mut in_str = false;
            while j < buf.len() {
                let c = buf[j];
                if in_str {
                    if c == b'\\' {
                        j += 2;
                        continue;
                    }
                    if c == b'"' {
                        in_str = false;
                    }
                } else if c == b'"' {
                    in_str = true;
                } else if c == b'{' {
                    depth += 1;
                } else if c == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&buf[start..=j]);
                    }
                }
                j += 1;
            }
            return None;
        }
        i += 1;
    }
    None
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── MCP-CB / Call Semantics Tests ──");

    crate::test::run_test("mcpcb_json_to_cb_roundtrip", || {
        let req = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"gpio_write\",\"arguments\":{\"pin\":3,\"value\":true}}}";
        let frame = json_to_cb(req, 9).unwrap();
        let (seq, name, args) = cb_parse(&frame).unwrap();
        seq == 9 && name == "gpio_write" && args == b"{\"pin\":3,\"value\":true}"
    });

    crate::test::run_test("mcpcb_reject_bad_magic_and_trunc", || {
        cb_parse(b"XXXX1\x02gpio").is_none() && cb_parse(b"MCB1\x01\x04gpi").is_none()
    });

    crate::test::run_test("mcpcb_result_frame_roundtrip", || {
        let f = result_to_cb(5, 0, b"{\"ok\":true}");
        f.starts_with(b"MCB1") && f[4] == 5 && f[5] == 0 && &f[8..] == b"{\"ok\":true}"
    });

    crate::test::run_test("mcpcb_dualpath_equivalence", || {
        // 同一工具调用：CB 路径解析出的 name/args 必须与 JSON 路径提取的一致
        let req = b"{\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"timer_sleep_ms\",\"arguments\":{\"ms\":250}}}";
        let frame = json_to_cb(req, 1).unwrap();
        let (_, name_cb, args_cb) = cb_parse(&frame).unwrap();
        let name_json = json_str(req, "name").unwrap();
        let args_json = find_arguments(req).unwrap();
        name_cb.as_bytes() == name_json && args_cb.as_slice() == args_json
    });

    crate::test::run_test("calltable_idempotent_duplicate_rejected", || {
        let mut t = CallTable::new(1000);
        t.begin(1, 100) && !t.begin(1, 150) // 重复 InFlight 被拒
    });

    crate::test::run_test("calltable_timeout_then_retry", || {
        let mut t = CallTable::new(1000);
        t.begin(2, 100);
        t.sweep(1200); // 超时
        let timed_out = t
            .calls
            .iter()
            .any(|c| c.seq == 2 && c.state == CallState::TimedOut);
        let retried = t.begin(2, 1300); // 超时后允许重试
        timed_out && retried
    });

    crate::test::run_test("calltable_done_replay_cached", || {
        let mut t = CallTable::new(1000);
        t.begin(3, 100);
        t.complete(3, b"{\"ok\":1}");
        !t.begin(3, 200) // Done 调用幂等拒绝
            && t.replay(3) == Some(&b"{\"ok\":1}"[..])
    });

    crate::test::run_test("calltable_complete_unknown_ignored", || {
        let mut t = CallTable::new(1000);
        !t.complete(9, b"x")
    });
}
