// user/dmesg.rs — print kernel log buffer
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use core::arch::asm;

const SYS_EXIT: usize = 1;
const SYS_WRITE: usize = 2;
/// Syscall 81: syslog(buf, len, offset) → bytes_read
const SYS_SYSLOG: usize = 81;

#[cfg(target_arch = "riscv64")]
unsafe fn syscall3(n: usize, a0: usize, a1: usize, a2: usize) -> isize {
    let ret: usize;
    asm!(
        "ecall",
        inlateout("a0") a0 => ret,
        inlateout("a1") a1 => _,
        inlateout("a2") a2 => _,
        in("a7") n,
        out("a3") _,
        out("a4") _,
        out("a5") _,
        out("a6") _,
    );
    ret as isize
}

#[cfg(target_arch = "x86_64")]
unsafe fn syscall3(n: usize, a0: usize, a1: usize, a2: usize) -> isize {
    let ret: isize;
    asm!(
        "int 0x80",
        inlateout("rax") n as usize => ret,
        in("rdi") a0,
        in("rsi") a1,
        in("rdx") a2,
        out("rcx") _,
        out("r11") _,
    );
    ret
}

unsafe fn syscall1(n: usize, a0: usize) -> isize {
    syscall3(n, a0, 0, 0)
}

fn print(s: &[u8]) {
    unsafe { syscall3(SYS_WRITE, 1, s.as_ptr() as usize, s.len()); }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let mut offset: usize = 0;
    let buf = [0u8; 1024];
    loop {
        let n = syscall3(SYS_SYSLOG, buf.as_ptr() as usize, buf.len(), offset);
        if n <= 0 {
            break;
        }
        let len = n as usize;
        let mut end = 0;
        while end < len && buf[end] != 0 {
            end += 1;
        }
        if end > 0 {
            print(&buf[..end]);
        }
        offset += len;
        if end < 1024 {
            break;
        }
    }
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
