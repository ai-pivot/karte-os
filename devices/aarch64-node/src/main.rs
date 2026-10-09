//! KarteOS aarch64 档设备 —— 边缘 AI 节点（QEMU aarch64 virt, EL1）
//!
//! 与 MCU 档（rv32imc）、M 档（rv64 主内核）跑的是同一份 KarteOS 的
//! 不同档位构建；本档通过 PL011 串口接入 CapDesc 织物（KRT1 wire）：
//!   announce  KRT1|A|karte-a-node|<seq>|<tools>
//!   invoke    KRT1|I|karte-a-node|<seq>|<tool>
//!   result    KRT1|T|karte-a-node|<seq>|<result>
//!
//! 启动：EL1 裸入口（global_asm，主核判 mpidr 后设栈跳 kmain_aarch64）。

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

core::arch::global_asm!(
    ".section .text.boot",
    ".global _start",
    "_start:",
    "    mrs x0, mpidr_el1",
    "    and x0, x0, #3",
    "    cbz x0, 1f",
    "2:  wfe",
    "    b 2b",
    "1:  ldr x1, =__boot_stack_top",
    "    mov sp, x1",
    "    bl kmain_aarch64",
    "    b 2b",
);

const UART0: usize = 0x0900_0000; // QEMU virt PL011
const DEVICE_ID: &str = "karte-a-node";
/// 边缘 AI 节点出厂能力（CapDesc 工具集）
const TOOLS: &str = "camera.snapshot,npu.infer,node.info";

fn uart_putc(c: u8) {
    unsafe {
        while read_volatile((UART0 + 0x18) as *const u32) & (1 << 5) != 0 {} // FR.TXFF
        write_volatile(UART0 as *mut u32, c as u32);
    }
}

fn puts(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            uart_putc(b'\r');
        }
        uart_putc(b);
    }
}

/// 非阻塞单字节接收（PL011 FR.RXFE = 空）
fn uart_try_rx() -> Option<u8> {
    unsafe {
        if read_volatile((UART0 + 0x18) as *const u32) & (1 << 4) == 0 {
            Some((read_volatile(UART0 as *const u32) & 0xFF) as u8)
        } else {
            None
        }
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

/// 角色受限工具执行（边缘 AI 节点语义）
fn run_tool(tool: &str) -> &'static str {
    match tool {
        "camera.snapshot" => "640x480:ok",
        "npu.infer" => "cls:cat:0.92",
        "node.info" => "aarch64:el1:qemu-virt",
        _ => "err:tool-not-permitted",
    }
}

#[unsafe(no_mangle)]
extern "C" fn kmain_aarch64() -> ! {
    puts("\n[a-node] KarteOS aarch64-tier device up (EL1, PL011)\n");
    puts("[a-node] capabilities: ");
    puts(TOOLS);
    puts("\n");

    let mut seq: u32 = 0;
    let mut inv = [0u8; 128];
    let mut ilen = 0usize;
    loop {
        // 1) announce
        {
            let mut f = [0u8; 128];
            let mut i = 0;
            append_str(&mut f, &mut i, "KRT1|A|");
            append_str(&mut f, &mut i, DEVICE_ID);
            f[i] = b'|';
            i += 1;
            append_u32(&mut f, &mut i, seq);
            f[i] = b'|';
            i += 1;
            append_str(&mut f, &mut i, TOOLS);
            f[i] = b'\n';
            i += 1;
            for b in &f[..i] {
                uart_putc(*b);
            }
        }
        // 2) RX 聚合（遇 \n 完成一帧；静默一段后回到 announce）
        let mut ready = false;
        let mut wait: u32 = 0;
        loop {
            match uart_try_rx() {
                None => {
                    wait += 1;
                    if wait > 3_000_000 {
                        break;
                    }
                }
                Some(b) => {
                    if b == b'\n' || b == b'\r' {
                        ready = true;
                        break;
                    }
                    if ilen < inv.len() {
                        inv[ilen] = b;
                        ilen += 1;
                    }
                }
            }
        }
        if ready && ilen >= 10 && &inv[..7] == b"KRT1|I|" {
            // KRT1|I|<id>|<seq>|<tool>
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
                puts("[a-node] invoke tool=");
                puts(tool);
                puts("\n");
                let result = run_tool(tool);
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
                r[i] = b'\n';
                i += 1;
                for b in &r[..i] {
                    uart_putc(*b);
                }
                puts("[a-node] invoke replied (uart)\n");
            }
        }
        if ready {
            ilen = 0;
        }
        seq = seq.wrapping_add(1);
    }
}

#[panic_handler]
fn ph(_info: &PanicInfo) -> ! {
    puts("[a-node] PANIC\n");
    loop {
        unsafe { asm!("wfe") }
    }
}
