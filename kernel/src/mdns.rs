//! P3.3 mDNS/DNS-SD v1 — _karte._tcp 服务发现（用户态 UDP，内核 wire 辅助）
//!
//! ROADMAP §P3.3。mDNS 组播（224.0.0.251:5353）上发 DNS 报文：
//!   - announce：PTR _karte._tcp.local → <host>._karte._tcp.local + SRV/TXT
//!   - 验收（v1 内核级）：DNS 报文 wire 编码单测 + QEMU 内 UDP 发包真出网
//!     （avahi 互通受 QEMU user-net 组播限制，实测归 P2.5 双机演示路径）

use alloc::format;
use alloc::vec::Vec;

pub const MDNS_PORT: u16 = 5353;
pub const MDNS_GROUP: [u8; 4] = [224, 0, 0, 251];

/// 压缩域名编码（无压缩指针，全展开——v0 足够且兼容）。
pub fn encode_name(out: &mut Vec<u8>, labels: &[&str]) {
    for l in labels {
        out.push(l.len() as u8);
        out.extend_from_slice(l.as_bytes());
    }
    out.push(0);
}

/// 构造 _karte._tcp.local 的 announce（PTR + SRV + TXT，均 unique 记录近似）。
/// `host`：设备主机名（不含 .local）；`port`：服务端口；`txt`：CoRE Link
/// Format 近似的资源描述（mDNS TXT strings，v1 完整 Link Format 收尾）。
pub fn build_karte_announce(host: &str, port: u16, txt: &str) -> Vec<u8> {
    let mut m = Vec::new();
    // Header: ID=0, flags=0x8400 (response/authoritative), QD=0, AN=3
    m.extend_from_slice(&[
        0x00, 0x00, 0x84, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00,
    ]);
    let svc_labels = ["_karte", "_tcp", "local"];
    let inst = format!("{}._karte._tcp.local", host);

    // AN1: PTR  _karte._tcp.local → host._karte._tcp.local
    encode_name(&mut m, &svc_labels);
    m.extend_from_slice(&[0x00, 0x0C, 0x80, 0x01]); // PTR, class IN + cache-flush
    m.extend_from_slice(&[0x00, 0x00, 0x11, 0x94]); // TTL 4500
    let mut rd = Vec::new();
    encode_name(&mut rd, &[&inst]);
    m.extend_from_slice(&(rd.len() as u16).to_be_bytes());
    m.extend_from_slice(&rd);

    // AN2: SRV  host._karte._tcp.local → (0, 0, port, host.local)
    encode_name(&mut m, &[&inst]);
    m.extend_from_slice(&[0x00, 0x21, 0x80, 0x01]); // SRV
    m.extend_from_slice(&[0x00, 0x00, 0x11, 0x94]);
    let mut rd = Vec::new();
    rd.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // priority/weight
    rd.extend_from_slice(&port.to_be_bytes());
    encode_name(&mut rd, &[host, "local"]);
    m.extend_from_slice(&(rd.len() as u16).to_be_bytes());
    m.extend_from_slice(&rd);

    // AN3: TXT  host._karte._tcp.local → CoRE Link Format 近似
    encode_name(&mut m, &[&inst]);
    m.extend_from_slice(&[0x00, 0x10, 0x80, 0x01]); // TXT
    m.extend_from_slice(&[0x00, 0x00, 0x11, 0x94]);
    let mut rd = Vec::new();
    let s = txt.as_bytes();
    rd.push(s.len().min(255) as u8);
    rd.extend_from_slice(&s[..s.len().min(255)]);
    m.extend_from_slice(&(rd.len() as u16).to_be_bytes());
    m.extend_from_slice(&rd);
    m
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── mDNS v1 Tests ──");

    crate::test::run_test("mdns_name_encoding", || {
        let mut v = Vec::new();
        encode_name(&mut v, &["_karte", "_tcp", "local"]);
        // len'label' ×3 + 终止 0 = 7+5+6+1 = 19
        v.len() == 19 && v[0] == 6 && v[7] == 4 && v[12] == 5 && v[18] == 0
    });

    crate::test::run_test("mdns_announce_structure", || {
        let m = build_karte_announce("karte-rv1", 1883, "</cmds>;ttl=300");
        // Header 12B + flags=0x8400 + AN count=3
        m.len() > 40
            && m[2] == 0x84
            && m[3] == 0x00
            && m[7] == 3
            // PTR record type 0x0C 出现在 AN1 的 fixed 偏移处
            && m[12 + 19] == 0x00
            && m[12 + 20] == 0x0C
    });

    crate::test::run_test("mdns_srv_port_encoding", || {
        let m = build_karte_announce("karte-rv1", 1883, "</cmds>");
        // SRV 的 port 字段（big-endian 1883 = 0x075B）必须出现在报文中
        let port = 1883u16.to_be_bytes();
        m.windows(2).any(|w| w == port)
    });
}
