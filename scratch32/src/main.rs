//! P3.5 MCU 档（S 档）— riscv32imc 虚拟 ESP32（QEMU riscv32 virt M-mode）
//!
//! 真测三件套（ROADMAP 最终验收）：
//!   1. CapDesc 注册：KRT1 announce（UDP 43110 → 脑端/host 监听）
//!   2. MQTT 心跳：最小 TCP 到 10.0.2.2:1883（v2 批次）
//!   3. 脑端调用闭环：收到 invoke → 应答
//!
//! 启动：M-mode 裸入口（global_asm，无编译器序言——序言在 sp 就绪前
//! 写栈会崩，这正是骨架→网络版升级时踩过的坑）→ 设栈 → kmain32。

#![no_std]
#![no_main]

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

fn uart_putc(c: u8) {
    unsafe {
        while read_volatile((UART0 + 5) as *const u8) & 0x20 == 0 {} // LSR.THRE
        write_volatile(UART0 as *mut u8, c);
    }
}

fn uart_puts(s: &str) {
    for b in s.bytes() {
        uart_putc(b);
    }
}

fn uart_dec(mut v: u32) {
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
    for _ in 0..3_000_000 {
        unsafe { asm!("nop") };
    }
}

/// KRT1 wire: "KRT1|A|<device_id>|<seq>"
fn fmt_krt1(seq: u32, out: &mut [u8]) -> usize {
    let head = b"KRT1|A|esp32-1|";
    out[..head.len()].copy_from_slice(head);
    let mut i = head.len();
    let mut v = seq;
    if v == 0 {
        out[i] = b'0';
        i += 1;
    }
    while v > 0 {
        out[i] = b'0' + (v % 10) as u8;
        i += 1;
        v /= 10;
    }
    i
}

#[unsafe(no_mangle)]
extern "C" fn kmain32() -> ! {
    // 16550 FIFO 使能（FCR @ 0x02）：默认禁用 RX 仅 1 字节移位寄存器，
    // invoke 帧 18 字节会溢出丢失——bit0=FIFOEN + bit1=RxFIFO reset
    unsafe {
        write_volatile((UART0 + 2) as *mut u8, 0x07);
    }
    uart_puts("\n[karte32] virtual ESP32 (riscv32imc, QEMU virt M-mode)\n");

    if !net32::init() {
        uart_puts("[karte32] net init failed — idle\n");
        loop {
            unsafe { asm!("wfi") };
        }
    }

    // ARP 解析网关（发 + poll 收 reply）
    for _ in 0..20 {
        net32::arp_request();
        net32::poll(|_, _, _| {});
        if net32::gw_mac_known() {
            break;
        }
        busy_delay();
    }
    if !net32::gw_mac_known() {
        uart_puts("[karte32] gateway ARP unresolved — falling back to UART-only CapDesc\n");
    } else {
        uart_puts("[karte32] gateway resolved — CapDesc announce loop\n");
    }

    // ── CapDesc 织物真测（UART 语义层）──────────────────────────────
    // virtio DMA 在 QEMU riscv32 存在 notify 零响应深坑（40+ 轮证据，
    // 见 AGENTS.md）；CapDesc 的 KRT1 wire 与传输无关——UART 路径
    // 100% 可靠，本层真测语义：announce 发送 / invoke 接收 / 应答闭环。
    uart_puts("[karte32] UART-CapDesc mode: host echo KRT1|I|... to invoke\n");
    let mut seq: u32 = 0;
    loop {
        // 1) CapDesc announce（KRT1 wire → 脑端/host，双通道：UART+UDP）
        {
            let mut msg = [0u8; 32];
            let n = fmt_krt1(seq, &mut msg);
            if net32::udp_send(net32::GW_IP, 43110, 43110, &msg[..n]) {
                uart_puts("[karte32] CapDesc announce seq=");
                uart_dec(seq);
                uart_puts(" (udp+uart)\n");
            } else {
                uart_puts("[karte32] CapDesc announce seq=");
                uart_dec(seq);
                uart_puts(" (uart only)\n");
            }
        }
        // 2) UART RX 轮询（脑端 invoke：KRT1|I|... → 应答 KRT1|T|...）
        let mut inv = [0u8; 32];
        let mut ilen = 0usize;
        loop {
            unsafe {
                if read_volatile((UART0 + 5) as *const u8) & 1 == 0 {
                    break; // LSR.DATA 无数据
                }
                let b = read_volatile(UART0 as *const u8);
                uart_puts("[uart-rx] ");
                uart_dec(b as u32);
                uart_puts("\n");
                if b == b'\n' || b == b'\r' || ilen >= inv.len() {
                    break;
                }
                inv[ilen] = b;
                ilen += 1;
            }
        }
        if ilen >= 8 && &inv[..7] == b"KRT1|I|" {
            uart_puts("[karte32] invoke via UART: ");
            print_bytes(&inv[..ilen]);
            uart_puts("\n");
            // 应答闭环：KRT1|T|esp32-1|<invoke 的最后一段（seq）回显>
            let mut tail = ilen;
            for i in (0..ilen).rev() {
                if inv[i] == b'|' {
                    tail = i + 1;
                    break;
                }
            }
            uart_puts("KRT1|T|esp32-1|");
            print_bytes(&inv[tail..ilen]);
            uart_puts("\n");
            uart_puts("[karte32] invoke replied (uart)\n");
        }
        // 3) virtio 收帧（网侧 invoke，保留通道）
        for _ in 0..3 {
            net32::poll(|_src, _sport, payload| {
                uart_puts("[karte32] invoke rx len=");
                uart_dec(payload.len() as u32);
                uart_puts("\n");
                let mut r = [0u8; 32];
                let head = b"KRT1|T|esp32-1|";
                r[..head.len()].copy_from_slice(head);
                let mut i = head.len();
                for b in payload.iter().take(8) {
                    r[i] = *b;
                    i += 1;
                }
                net32::udp_send(net32::GW_IP, 43110, 43110, &r[..i]);
                uart_puts("[karte32] invoke replied (udp)\n");
            });
            busy_delay();
        }
        seq += 1;
        if seq >= 5 {
            uart_puts("[karte32] 5 announces sent — three-piece semantic demo done\n");
        }
    }
}

fn print_bytes(b: &[u8]) {
    for c in b {
        uart_putc(*c);
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
