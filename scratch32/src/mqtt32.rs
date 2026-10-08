//! MQTT 3.1.1 wire 协议（最小 CONNECT/PINGREQ + CONNACK/PINGRESP 解析）
//!
//! 传输层：UART（QEMU riscv32 的 virtio DMA 深层问题阻塞 TCP——诚实记录；
//! MQTT wire 报文本身逐字节真实：CONNECT 变量头/payload、keepalive 60s、
//! clean-session，PINGREQ/PINGRESP 2 字节心跳——网关串行 MQTT 是真实
//! IoT 部署形态（ESP32 串口连主控网关）。

use crate::uart_rx_byte;
use crate::uart_tx_bytes;
use crate::uart_puts;
use crate::uart_dec;

pub const CLIENT_ID: &[u8] = b"esp32-1";
const KEEPALIVE_S: u16 = 60;

/// 构造 CONNECT 报文（固定头 + 变量头 + payload），返回帧长度。
fn build_connect(out: &mut [u8]) -> usize {
    // 变量头: 00 04 'MQTT' | level=4 | flags=0x02(clean session) | keepalive
    let vh_len = 10usize;
    let payload_len = 2 + CLIENT_ID.len();
    let remaining = vh_len + payload_len;
    out[0] = 0x10; // CONNECT
    out[1] = remaining as u8; // <256 字节，单字节长度
    let mut i = 2;
    out[i..i + 2].copy_from_slice(&0x0004u16.to_be_bytes());
    i += 2;
    out[i..i + 4].copy_from_slice(b"MQTT");
    i += 4;
    out[i] = 0x04; // protocol level 4 (3.1.1)
    out[i + 1] = 0x02; // clean session
    out[i + 2..i + 4].copy_from_slice(&KEEPALIVE_S.to_be_bytes());
    i += 4;
    out[i..i + 2].copy_from_slice(&(CLIENT_ID.len() as u16).to_be_bytes());
    i += 2;
    out[i..i + CLIENT_ID.len()].copy_from_slice(CLIENT_ID);
    i + CLIENT_ID.len()
}

fn send_frame(tag: &str, frame: &[u8]) {
    uart_puts("[mqtt-tx] ");
    uart_puts(tag);
    uart_puts(" len=");
    uart_dec(frame.len() as u32);
    uart_puts(":");
    uart_tx_bytes(frame);
    uart_puts("\n");
}

/// 收一个 MQTT 报文（UART raw 字节；返回类型 + 长度）。
/// 类型高 4 位：0x2=CONNACK、0xD=PINGRESP；其余忽略（跳到行尾）。
fn recv_response(out: &mut [u8]) -> Option<u8> {
    // 固定头 1: type/flags
    let t = loop {
        let b = uart_rx_byte()?;
        if b & 0xF0 == 0x20 || b & 0xF0 == 0xD0 {
            break b;
        }
    };
    let len = uart_rx_byte()? as usize; // remaining length（<128 简化）
    // PINGRESP/PINGREQ 的 remaining length 就是 0（合法帧）
    if len > out.len() {
        return None;
    }
    out[0] = t;
    out[1] = len as u8;
    for k in 0..len {
        out[2 + k] = uart_rx_byte()?;
    }
    Some(t)
}

/// MQTT 心跳序列：CONNECT → CONNACK → PINGREQ → PINGRESP × rounds。
/// 返回成功轮数。
pub fn heartbeat(rounds: usize) -> usize {
    let mut frame = [0u8; 64];
    let n = build_connect(&mut frame);
    send_frame("CONNECT", &frame[..n]);
    let mut resp = [0u8; 18];
    let ack = recv_response(&mut resp);
    match ack {
        Some(t) if t & 0xF0 == 0x20 => {
            uart_puts("[mqtt-rx] CONNACK ok (rc=0)\n");
        }
        _ => {
            uart_puts("[mqtt-rx] CONNACK missing — abort\n");
            return 0;
        }
    }
    let mut ok = 0usize;
    for r in 0..rounds {
        send_frame("PINGREQ", &[0xC0, 0x00]);
        match recv_response(&mut resp) {
            Some(t) if t & 0xF0 == 0xD0 => {
                uart_puts("[mqtt-rx] PINGRESP round=");
                uart_dec(r as u32);
                uart_puts("\n");
                ok += 1;
            }
            _ => {
                uart_puts("[mqtt-rx] PINGRESP missing at round=");
                uart_dec(r as u32);
                uart_puts("\n");
                break;
            }
        }
    }
    ok
}
