//! KarteOS S 档（MCU）设备固件 —— 参数化多设备
//!
//! 同一份 KarteOS S 档内核构建出不同的 IoT 设备（编译期参数）：
//!   KARTE_DEVICE_ID  设备 id（默认 esp32-1）
//!   KARTE_ROLE       角色 ∈ {sensor, relay}（决定 CapDesc 能力表 + invoke 语义）
//!
//! 织物协议（KRT1，与主内核 kernel/src/drt.rs 同构）：
//!   announce  KRT1|A|<id>|<seq>|<tool1,tool2,...>
//!   invoke    KRT1|I|<id>|<seq>|<tool>
//!   result    KRT1|T|<id>|<seq>|<result>
//!   心跳      KRT1|H|<id>|<seq>
//!
//! 传输：UART（网关串行织物）——QEMU riscv32 virt 的 virtio DMA 深层问题
//! 已诚实记录在 AGENTS.md；UDP 通道作为附加尝试（virtio up 时）。
//!
//! 启动：M-mode 裸入口（global_asm 无编译器序言——sp 就绪前写栈会崩）。

#![no_std]
#![no_main]

mod mqtt32;
mod net32;

use core::arch::asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

const UART0: usize = 0x1000_0000;

core::arch::global_asm!(
    ".section .text.init",
    ".global _start",
    "_start:",
    "    la sp, _stack_top",
    "    call kmain32",
    "1:  wfi",
    "    j 1b",
);

/// 编译期设备身份（同一内核源码 → 不同设备固件）。
const DEVICE_ID: &str = match option_env!("KARTE_DEVICE_ID") {
    Some(s) => s,
    None => "esp32-1",
};
const ROLE: &str = match option_env!("KARTE_ROLE") {
    Some(s) => s,
    None => "sensor",
};

/// CapDesc 能力表（角色决定出厂能力——「驱动即工具」的设备侧表达）。
fn tools() -> &'static str {
    match ROLE {
        "sensor" => "sensor.temp,sensor.humidity,mqtt.ping",
        "relay" => "actuator.on,actuator.off,actuator.status",
        _ => "echo",
    }
}

fn uart_putc(c: u8) {
    unsafe {
        while read_volatile((UART0 + 5) as *const u8) & 0x20 == 0 {} // LSR.THRE
        write_volatile(UART0 as *mut u8, c);
    }
}

/// UART 原始字节流输出（MQTT wire 帧）。
pub fn uart_tx_bytes(b: &[u8]) {
    for c in b {
        uart_putc(*c);
    }
}

/// UART 单字节接收（LSR.DR 轮询 + 100M 次上限防死锁；None = 超时）。
/// host 侧响应延迟可达秒级（python 事件循环），超时必须远大于 RTT。
pub fn uart_rx_byte() -> Option<u8> {
    for _ in 0..100_000_000 {
        unsafe {
            if read_volatile((UART0 + 5) as *const u8) & 1 != 0 {
                return Some(read_volatile(UART0 as *const u8));
            }
        }
    }
    None
}

pub fn uart_puts(s: &str) {
    for b in s.bytes() {
        uart_putc(b);
    }
}

pub fn uart_dec(mut v: u32) {
    let mut b = [0u8; 12];
    let mut i = b.len();
    if v == 0 {
        i -= 1;
        b[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    uart_puts(core::str::from_utf8(&b[i..]).unwrap_or("?"));
}

fn busy_delay() {
    // 轮询节奏：兼顾 announce 频率与 RX 及时读取（FIFO 16 字节无流控）
    for _ in 0..800_000 {
        unsafe { asm!("nop") };
    }
}

fn append_u32(out: &mut [u8], i: &mut usize, mut v: u32) {
    let mut tmp = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        tmp[0] = b'0';
        n = 1;
    }
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
    }
    for k in (0..n).rev() {
        out[*i] = tmp[k];
        *i += 1;
    }
}

fn append_str(out: &mut [u8], i: &mut usize, s: &str) {
    for b in s.bytes() {
        out[*i] = b;
        *i += 1;
    }
}

/// announce: KRT1|A|<id>|<seq>|<tools>
fn fmt_announce(seq: u32, out: &mut [u8]) -> usize {
    let mut i = 0;
    append_str(out, &mut i, "KRT1|A|");
    append_str(out, &mut i, DEVICE_ID);
    out[i] = b'|';
    i += 1;
    append_u32(out, &mut i, seq);
    out[i] = b'|';
    i += 1;
    append_str(out, &mut i, tools());
    i
}

/// 角色受限的工具执行——设备侧真实语义。
fn run_tool(tool: &str, seq: u32) -> &'static str {
    match (ROLE, tool) {
        ("sensor", "sensor.temp") => "23.5",
        ("sensor", "sensor.humidity") => "41",
        ("sensor", "mqtt.ping") => {
            let ok = mqtt32::heartbeat(1);
            if ok > 0 { "mqtt-ok" } else { "mqtt-fail" }
        }
        ("relay", "actuator.on") => "on",
        ("relay", "actuator.off") => "off",
        ("relay", "actuator.status") => {
            // 状态随 seq 奇偶翻转（可观测的确定性行为）
            if seq % 2 == 0 { "on" } else { "off" }
        }
        _ => "err:tool-not-permitted",
    }
}

#[unsafe(no_mangle)]
extern "C" fn kmain32() -> ! {
    // 16550 FIFO 使能（FCR @ 0x02）：默认禁用 RX 仅 1 字节移位寄存器，
    // invoke 帧会溢出丢失——bit0=FIFOEN + bit1=RxFIFO reset
    unsafe {
        write_volatile((UART0 + 2) as *mut u8, 0x07);
    }
    uart_puts("\n[karte32] KarteOS S-tier device id=");
    uart_puts(DEVICE_ID);
    uart_puts(" role=");
    uart_puts(ROLE);
    uart_puts(" (riscv32imc, QEMU virt M-mode)\n");
    uart_puts("[karte32] capabilities: ");
    uart_puts(tools());
    uart_puts("\n");

    let net_ok = net32::init();
    if net_ok {
        for _ in 0..10 {
            net32::arp_request();
            net32::poll(|_, _, _| {});
            if net32::gw_mac_known() {
                break;
            }
            busy_delay();
        }
    }
    uart_puts("[karte32] transport: uart");
    if net_ok && net32::gw_mac_known() {
        uart_puts("+udp");
    }
    uart_puts("\n");

    let mut seq: u32 = 0;
    // 帧聚合缓冲跨主循环保留：UART 是无流控字节流，一帧可能分多轮到达
    let mut inv = [0u8; 128];
    let mut ilen = 0usize;
    loop {
        // 1) CapDesc announce（KRT1 wire，UART 主 + UDP 附加）
        {
            let mut msg = [0u8; 128];
            let n = fmt_announce(seq, &mut msg);
            if net_ok && net32::gw_mac_known() {
                let _ = net32::udp_send(net32::GW_IP, 43110, 43110, &msg[..n]);
            }
            uart_tx_bytes(&msg[..n]);
            uart_puts("\n");
        }
        // 2) UART RX 轮询：追加到帧缓冲，遇 \n/\r 表示一帧完成（消费终止符）
        let mut frame_ready = false;
        loop {
            unsafe {
                if read_volatile((UART0 + 5) as *const u8) & 1 == 0 {
                    break; // 当前无数据：保留部分帧，下轮继续
                }
                let b = read_volatile(UART0 as *const u8);
                if b == b'\n' || b == b'\r' {
                    frame_ready = true;
                    break;
                }
                if ilen < inv.len() {
                    inv[ilen] = b;
                    ilen += 1;
                }
            }
        }
        // 帧超长保护：无终止符时按最大长度截断处理，避免缓冲卡死
        if ilen >= inv.len() {
            frame_ready = true;
        }
        if frame_ready && ilen >= 10 && &inv[..7] == b"KRT1|I|" {
            // KRT1|I|<id>|<seq>|<tool>：取第 4 个 '|' 之后的 tool
            let mut bars = 0;
            let mut tool_at = None;
            for (k, c) in inv[..ilen].iter().enumerate() {
                if *c == b'|' {
                    bars += 1;
                    if bars == 4 {
                        tool_at = Some(k + 1);
                        break;
                    }
                }
            }
            if let Some(ta) = tool_at {
                let tool = core::str::from_utf8(&inv[ta..ilen]).unwrap_or("");
                uart_puts("[karte32] invoke tool=");
                uart_puts(tool);
                uart_puts("\n");
                let result = run_tool(tool, seq);
                // 应答：KRT1|T|<id>|<seq>|<result>
                let mut r = [0u8; 96];
                let mut i = 0;
                append_str(&mut r, &mut i, "KRT1|T|");
                append_str(&mut r, &mut i, DEVICE_ID);
                r[i] = b'|';
                i += 1;
                append_u32(&mut r, &mut i, seq);
                r[i] = b'|';
                i += 1;
                append_str(&mut r, &mut i, result);
                uart_tx_bytes(&r[..i]);
                uart_puts("\n");
                uart_puts("[karte32] invoke replied (uart)\n");
            }
        }
        if frame_ready {
            ilen = 0;
        }
        // 3) virtio 收帧（UDP 通道附加，若设备可达）
        if net_ok {
            net32::poll(|_src, _sport, _payload| {});
        }
        seq = seq.wrapping_add(1);
        busy_delay();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    uart_puts("[karte32] PANIC: ");
    if let Some(loc) = info.location() {
        uart_dec(loc.line() as u32);
    }
    uart_puts("\n");
    loop {
        unsafe { asm!("wfi") };
    }
}
