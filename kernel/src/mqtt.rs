//! P3.3 IoT 协议 — MQTT 3.1.1 包编解码（客户端侧）
//!
//! 最小子集（与 tools/mqtt-mini-broker.py 互通，QoS1）：
//!   CONNECT / CONNACK / PUBLISH / PUBACK / SUBSCRIBE / SUBACK / PING*
//! 编解码为纯函数，内核单测锁定 wire 正确性；user/mqtt.rs 消费。

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// 剩余长度 varint（MQTT 规范 1 字节..4 字节）
pub fn encode_remaining_len(mut n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
    out
}

pub fn decode_remaining_len(buf: &[u8]) -> Option<(usize, usize)> {
    let mut mult = 1usize;
    let mut val = 0usize;
    for (i, &b) in buf.iter().enumerate() {
        val += (b as usize & 0x7F) * mult;
        if b & 0x80 == 0 {
            return Some((val, i + 1));
        }
        mult *= 128;
        if mult > 128 * 128 * 128 {
            return None; // 超过 4 字节
        }
    }
    None
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// CONNECT：proto MQTT/4 + clean session + client id
pub fn connect(client_id: &str) -> Vec<u8> {
    let mut body = Vec::new();
    push_str(&mut body, "MQTT");
    body.push(4); // level
    body.push(0x02); // clean session
    body.extend_from_slice(&60u16.to_be_bytes()); // keepalive 60s
    push_str(&mut body, client_id);
    let mut out = vec![0x10];
    out.extend(encode_remaining_len(body.len()));
    out.extend(body);
    out
}

/// PUBLISH（QoS1）：topic + packet id + payload
pub fn publish(topic: &str, payload: &[u8], pid: u16) -> Vec<u8> {
    let mut body = Vec::new();
    push_str(&mut body, topic);
    body.extend_from_slice(&pid.to_be_bytes());
    body.extend_from_slice(payload);
    let mut out = vec![0x32]; // PUBLISH, DUP=0, QoS1, RETAIN=0
    out.extend(encode_remaining_len(body.len()));
    out.extend(body);
    out
}

/// SUBSCRIBE：topic filter + max QoS
pub fn subscribe(topic_filter: &str, pid: u16, max_qos: u8) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&pid.to_be_bytes());
    push_str(&mut body, topic_filter);
    body.push(max_qos.min(1)); // broker 子集支持 QoS0/1
    let mut out = vec![0x82];
    out.extend(encode_remaining_len(body.len()));
    out.extend(body);
    out
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pkt {
    ConnAck(u8), // return code
    Publish {
        qos: u8,
        pid: Option<u16>,
        topic: String,
        payload: Vec<u8>,
    },
    PubAck(u16),
    SubAck {
        pid: u16,
        codes: Vec<u8>,
    },
    PingResp,
    Other,
}

/// 解析一帧（fixed header + 剩余长度 + body 已读齐）
pub fn parse(first: u8, body: &[u8]) -> Pkt {
    match first >> 4 {
        2 => Pkt::ConnAck(*body.get(1).unwrap_or(&0xFF)),
        3 => {
            let qos = (first >> 1) & 0x3;
            if body.len() < 2 {
                return Pkt::Other;
            }
            let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
            if body.len() < 2 + tlen {
                return Pkt::Other;
            }
            let topic = core::str::from_utf8(&body[2..2 + tlen])
                .unwrap_or("")
                .into();
            let mut off = 2 + tlen;
            let mut pid = None;
            if qos > 0 && body.len() >= off + 2 {
                pid = Some(u16::from_be_bytes([body[off], body[off + 1]]));
                off += 2;
            }
            Pkt::Publish {
                qos,
                pid,
                topic,
                payload: body[off..].to_vec(),
            }
        }
        4 => Pkt::PubAck(u16::from_be_bytes([
            *body.first().unwrap_or(&0),
            *body.get(1).unwrap_or(&0),
        ])),
        9 => Pkt::SubAck {
            pid: u16::from_be_bytes([*body.first().unwrap_or(&0), *body.get(1).unwrap_or(&0)]),
            codes: body.get(2..).unwrap_or(&[]).to_vec(),
        },
        13 => Pkt::PingResp,
        _ => Pkt::Other,
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── MQTT 3.1.1 Wire Tests ──");

    crate::test::run_test("mqtt_remaining_len_roundtrip", || {
        for n in [0usize, 1, 127, 128, 16383, 16384, 2097151] {
            let enc = encode_remaining_len(n);
            if enc.len() > 4 {
                return false;
            }
            match decode_remaining_len(&enc) {
                Some((v, used)) if v == n && used == enc.len() => {}
                _ => return false,
            }
        }
        decode_remaining_len(&[0xFF, 0xFF, 0xFF, 0xFF]).is_none()
    });

    crate::test::run_test("mqtt_connect_wire_shape", || {
        let c = connect("karte-node");
        c[0] == 0x10
            && &c[2..8] == b"\x00\x04MQTT"
            && c[8] == 4
            && c[9] == 0x02
            && &c[10..12] == b"\x00\x3C"
    });

    crate::test::run_test("mqtt_publish_qos1_roundtrip", || {
        let p = publish("karteo/telemetry", b"{\"t\":22.5}", 0x0102);
        p[0] == 0x32
            && p[1] != 0
            && match parse(p[0], &p[2..]) {
                Pkt::Publish {
                    qos: 1,
                    pid: Some(0x0102),
                    topic,
                    payload,
                } => topic == "karteo/telemetry" && payload == b"{\"t\":22.5}",
                _ => false,
            }
    });

    crate::test::run_test("mqtt_puback_parse", || {
        let ack = [0x40, 0x02, 0x01, 0x02];
        parse(ack[0], &ack[2..]) == Pkt::PubAck(0x0102)
    });

    crate::test::run_test("mqtt_subscribe_and_suback", || {
        let s = subscribe("karteo/cmd/#", 0x0007, 1);
        s[0] == 0x82
            && &s[2..4] == b"\x00\x07"
            && &s[4..6] == b"\x00\x0C"
            && &s[6..18] == b"karteo/cmd/#"
            && s[18] == 1
            && matches!(parse(0x90, &[0x00, 0x07, 0x01]), Pkt::SubAck { pid: 0x0007, ref codes } if codes == &[1u8])
            && parse(0xD0, &[]) == Pkt::PingResp
    });
}
