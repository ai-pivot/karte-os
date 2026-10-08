//! P2.4 L4 安全层 — 能力令牌 + 设备签名清单（附录 A.3 v1）
//!
//! 能力令牌链路：脑端（或内核）为 (device, tool, perm) 组合颁发
//! CapToken；设备侧校验 存在/未过期/未吊销/perm 匹配 四道关，任一
//! 不满足即拒绝调用。
//!
//! 设备注册签名清单（signed manifest）：设备用注册密钥派生的
//! manifest_hash 证明身份（信任锚在内核 REGISTRY 秘钥表）。hash 不
//! 匹配即伪冒注册，拒绝。v0 哈希用 FNV-1a 64（语义完整；真 ed25519
//! 签名归 Phase3 安全升级）。

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// 能力令牌（附录 A.3）
#[derive(Clone)]
pub struct CapToken {
    pub token_id: u64,
    pub device_id: String,
    pub tool_name: String,
    pub perm_bits: u8, // capability::perm 位集
    pub issued_ms: u64,
    pub expires_ms: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VerifyResult {
    Ok,
    NoToken,
    Expired,
    Revoked,
    PermDenied,
}

/// 令牌管理器：颁发/校验/吊销/过期清理
pub struct TokenManager {
    pub tokens: Vec<CapToken>,
    pub revoked: Vec<u64>,
    pub next_id: u64,
}

impl TokenManager {
    pub fn new() -> Self {
        TokenManager {
            tokens: Vec::new(),
            revoked: Vec::new(),
            next_id: 1,
        }
    }

    /// 颁发：返回新令牌 id
    pub fn issue(
        &mut self,
        device_id: &str,
        tool_name: &str,
        perm_bits: u8,
        now_ms: u64,
        ttl_ms: u64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.tokens.push(CapToken {
            token_id: id,
            device_id: device_id.into(),
            tool_name: tool_name.into(),
            perm_bits,
            issued_ms: now_ms,
            expires_ms: now_ms + ttl_ms,
        });
        id
    }

    /// 校验：四道关（存在 → 过期 → 吊销 → perm 匹配）
    pub fn verify(
        &mut self,
        token_id: u64,
        device_id: &str,
        tool_name: &str,
        need_perm: u8,
        now_ms: u64,
    ) -> VerifyResult {
        let Some(t) = self.tokens.iter().find(|t| t.token_id == token_id) else {
            return VerifyResult::NoToken;
        };
        if now_ms >= t.expires_ms {
            return VerifyResult::Expired;
        }
        if self.revoked.contains(&token_id) {
            return VerifyResult::Revoked;
        }
        if t.device_id != device_id || t.tool_name != tool_name {
            return VerifyResult::NoToken; // 令牌不匹配该 (device, tool)
        }
        if t.perm_bits & need_perm != need_perm {
            return VerifyResult::PermDenied;
        }
        VerifyResult::Ok
    }

    /// 吊销：任何时刻生效（即使已过期也记录，防止时钟回拨复活）
    pub fn revoke(&mut self, token_id: u64) -> bool {
        if self.revoked.contains(&token_id) {
            return false;
        }
        self.revoked.push(token_id);
        true
    }

    /// 过期清理（物理删除过期令牌；已吊销 id 保留）
    pub fn sweep(&mut self, now_ms: u64) -> usize {
        let before = self.tokens.len();
        self.tokens.retain(|t| t.expires_ms > now_ms);
        before - self.tokens.len()
    }
}

/// 全局令牌管理器（脑端颁发，网关/设备侧校验）
pub static TOKENS: Mutex<TokenManager> = Mutex::new(TokenManager {
    tokens: Vec::new(),
    revoked: Vec::new(),
    next_id: 1,
});

// ── 设备签名清单（signed manifest） ──

/// FNV-1a 64 位哈希（v0 manifest 签名原语）
pub const fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < data.len() {
        h ^= data[i] as u64;
        h = h.wrapping_mul(0x100000001b3);
        i += 1;
    }
    h
}

/// 注册密钥表（信任锚：v0 内置三个示范设备的密钥；真设备链归 Phase3）
pub fn registration_key(device_id: &str) -> Option<&'static str> {
    match device_id {
        "vfs0" => Some("kartevfs0key"),
        "timer0" => Some("kartetimer0key"),
        "gpio0" => Some("kartegpio0key"),
        _ => None,
    }
}

/// 生成 manifest_hash（设备侧：用注册密钥对 device_id 签名）
pub fn manifest_hash(device_id: &str, key: &str) -> u64 {
    let mut buf = Vec::new();
    buf.extend_from_slice(device_id.as_bytes());
    buf.push(0);
    buf.extend_from_slice(key.as_bytes());
    fnv1a64(&buf)
}

/// 注册校验（网关侧）：manifest_hash 与信任锚密钥重算匹配才放行。
/// 伪冒设备（不在密钥表 / hash 错）被拒。
pub fn verify_manifest(device_id: &str, claimed_hash: u64) -> bool {
    let Some(key) = registration_key(device_id) else {
        return false;
    };
    manifest_hash(device_id, key) == claimed_hash
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    use crate::capability::perm;

    crate::console_println!("");
    crate::console_println!("── CapAuth (P2.4 Security) Tests ──");

    crate::test::run_test("capauth_issue_verify_ok", || {
        let mut t = TokenManager::new();
        let id = t.issue("gpio0", "write", perm::WRITE, 1000, 10_000);
        t.verify(id, "gpio0", "write", perm::WRITE, 2000) == VerifyResult::Ok
    });

    crate::test::run_test("capauth_no_token_rejected", || {
        let mut t = TokenManager::new();
        t.verify(999, "gpio0", "write", perm::WRITE, 1000) == VerifyResult::NoToken
    });

    crate::test::run_test("capauth_expired_rejected", || {
        let mut t = TokenManager::new();
        let id = t.issue("gpio0", "write", perm::WRITE, 1000, 1000);
        t.verify(id, "gpio0", "write", perm::WRITE, 5000) == VerifyResult::Expired
    });

    crate::test::run_test("capauth_revoked_rejected", || {
        let mut t = TokenManager::new();
        let id = t.issue("gpio0", "write", perm::WRITE, 1000, 10_000);
        let was_valid = t.verify(id, "gpio0", "write", perm::WRITE, 2000) == VerifyResult::Ok;
        t.revoke(id);
        let after = t.verify(id, "gpio0", "write", perm::WRITE, 2000) == VerifyResult::Revoked;
        // 重复吊销幂等
        !t.revoke(id) && was_valid && after
    });

    crate::test::run_test("capauth_perm_mismatch_denied", || {
        let mut t = TokenManager::new();
        // 只授 READ，却要 WRITE
        let id = t.issue("gpio0", "read", perm::READ, 1000, 10_000);
        t.verify(id, "gpio0", "read", perm::WRITE, 2000) == VerifyResult::PermDenied
    });

    crate::test::run_test("capauth_device_tool_mismatch_rejected", || {
        let mut t = TokenManager::new();
        let id = t.issue("gpio0", "write", perm::WRITE, 1000, 10_000);
        t.verify(id, "gpio0", "read", perm::WRITE, 2000) == VerifyResult::NoToken
            && t.verify(id, "vfs0", "write", perm::WRITE, 2000) == VerifyResult::NoToken
    });

    crate::test::run_test("capauth_sweep_removes_expired_keeps_revoked_list", || {
        let mut t = TokenManager::new();
        let a = t.issue("gpio0", "write", perm::WRITE, 1000, 1000);
        let _b = t.issue("gpio0", "read", perm::READ, 1000, 60_000);
        t.revoke(a);
        let removed = t.sweep(5000);
        removed == 1 && t.tokens.len() == 1 && t.revoked.contains(&a)
    });

    crate::test::run_test("capauth_manifest_valid_registration", || {
        let h = manifest_hash("gpio0", "kartegpio0key");
        verify_manifest("gpio0", h)
    });

    crate::test::run_test("capauth_manifest_spoofed_rejected", || {
        // 伪冒1：hash 错
        let bad = !verify_manifest("gpio0", 0xdeadbeef);
        // 伪冒2：未知设备（不在信任锚表）
        let unknown = !verify_manifest("evil_dev", manifest_hash("evil_dev", "anykey"));
        // 伪冒3：密钥错
        let wrongkey = !verify_manifest("gpio0", manifest_hash("gpio0", "wrongkey"));
        bad && unknown && wrongkey
    });
}
