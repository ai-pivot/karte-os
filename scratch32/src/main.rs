//! P3.5 MCU 档（S 档）骨架 — riscv32imc 最小内核（ESP32-C3 同构 ISA）
//!
//! QEMU riscv32 virt（-bios none, M-mode 直起，无 MMU）：
//!   UART0 = 16550 @ 0x10000000（与 RV64 virt 相同外设布局）
//!   CLINT mtime @ 0x02000000（VFt=time 0x0200bff8）
//! 本骨架验证 MCU 档工具链与启动链；ESP32-C3 真机外设映射（UART0
//! 0x60000000 等）在接入主内核 feature 矩阵时按 SoC cfg 区分。

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

const UART0: usize = 0x1000_0000;

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

fn mstatusie_on() {
    unsafe {
        asm!("csrsi mstatus, 8"); // MIE
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.init")]
unsafe extern "C" fn _start() -> ! {
    // 设 sp（链接脚本定义 _stack_top）
    unsafe {
        asm!("la sp, _stack_top", options(nomem, nostack));
    }
    uart_puts("\n[karte32] MCU tier skeleton (riscv32imc, QEMU virt M-mode)\n");
    uart_puts("[karte32] hello from KarteOS S-tier!\n");
    mstatusie_on();
    loop {
        unsafe { asm!("wfi") };
        uart_puts("[karte32] wfi wakeup tick\n");
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    uart_puts("[karte32] PANIC: ");
    if let Some(loc) = info.location() {
        let mut buf = [0u8; 32];
        let msg = format_u32(loc.line(), &mut buf);
        uart_puts(msg);
    }
    uart_puts("\n");
    loop {
        unsafe { asm!("wfi") };
    }
}

fn format_u32(mut v: u32, buf: &mut [u8; 32]) -> &str {
    let mut i = buf.len();
    if v == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while v > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("?")
}
