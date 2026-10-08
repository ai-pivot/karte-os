// P3.1 aarch64 启动验证 demo（独立内核骨架）
//
// 验收（ROADMAP §P3.1-1）：QEMU aarch64 virt 上真启动、PL011 UART 输出
// 横幅与心跳计数 —— 证明 KarteOS 的 aarch64 路径可行；主内核的完整
// aarch64 移植（异常向量/GIC/页表/SMP）按本骨架展开。
#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

// P3.1 boot entry (replaces boot.S — cargo 不自动编译 .S 文件):
// 主核（mpidr 低 3 位 == 0）设栈跳 kmain，副核 wfe 停车。
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

fn uart_putc(c: u8) {
    unsafe {
        // 等 TX FIFO 不满（FR bit5 = TXFF）
        while read_volatile((UART0 + 0x18) as *const u32) & (1 << 5) != 0 {}
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

fn delay(n: u32) {
    for _ in 0..n {
        unsafe { asm!("nop") }
    }
}

#[unsafe(no_mangle)]
extern "C" fn kmain_aarch64() -> ! {
    puts("\r\n====================================\r\n");
    puts("| KarteOS aarch64 demo (P3.1)     |\r\n");
    puts("| EL1 boot OK, PL011 UART @virt   |\r\n");
    puts("====================================\r\n");
    let mut n: u32 = 0;
    loop {
        puts("[aarch64] heartbeat\n");
        n += 1;
        if n >= 5 {
            puts("[aarch64] 5 heartbeats, demo done — halting\n");
            // PSCI system_off: 调用号 0x84000008（SMC/HVC 由 QEMU 处理）
            unsafe {
                asm!(
                    "ldr w0, =0x84000008",
                    "hvc #0",
                    out("w0") _,
                    out("x1") _,
                    out("x2") _,
                    out("x3") _,
                );
            }
            loop {
                unsafe { asm!("wfe") }
            }
        }
        delay(20_000_000);
    }
}

#[panic_handler]
fn ph(_info: &PanicInfo) -> ! {
    puts("[aarch64] PANIC\n");
    loop {
        unsafe { asm!("wfe") }
    }
}
