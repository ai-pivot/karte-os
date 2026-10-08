// user/vadd.rs — RVV vector-add smoke test (M0-V③)
//
// Adds two 8-element u64 arrays with the V extension (e64, m1) and prints a
// checksum. Compiled with `-C target-feature=+v`; the vector path is selected
// via cfg(target_feature = "v"), so the same source also builds a scalar
// fallback for non-RVV toolchains. On a hart without V the vector build
// traps per-instruction (illegal) — the kernel's illegal handler skips them,
// so the process still exits instead of hanging; the real path requires
// QEMU >= 7 (`make run-rvv`).
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

const N: usize = 8;

fn putc(c: u8) {
    let buf = [c];
    unsafe {
        syscall3(SYS_WRITE, 1, buf.as_ptr() as usize, 1);
    }
}

fn print_hex(mut v: u64) {
    let digits = b"0123456789abcdef";
    let mut buf = [0u8; 16];
    let mut i = 16;
    if v == 0 {
        putc(b'0');
        return;
    }
    while v > 0 && i > 0 {
        i -= 1;
        buf[i] = digits[(v & 0xf) as usize];
        v >>= 4;
    }
    let s = &buf[i..];
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as usize, s.len());
    }
}

fn print(s: &[u8]) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as usize, s.len());
    }
}

#[cfg(target_feature = "v")]
fn vec_add(a: &[u64; N], b: &[u64; N], out: &mut [u64; N]) {
    unsafe {
        core::arch::asm!(
            ".option push",
            ".option arch, +v",
            "vsetvli t0, zero, e64, m1, ta, ma",
            "addi    t3, {a}, 0",
            "vle64.v v1, (t3)",
            "addi    t3, {b}, 0",
            "vle64.v v2, (t3)",
            "vadd.vv v3, v1, v2",
            "addi    t3, {out}, 0",
            "vse64.v v3, (t3)",
            ".option pop",
            a = in(reg) a.as_ptr(),
            b = in(reg) b.as_ptr(),
            out = in(reg) out.as_mut_ptr(),
            out("t0") _, out("t3") _,
        );
    }
}

#[cfg(not(target_feature = "v"))]
fn vec_add(a: &[u64; N], b: &[u64; N], out: &mut [u64; N]) {
    for i in 0..N {
        out[i] = a[i].wrapping_add(b[i]);
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let a: [u64; N] = [1, 2, 3, 4, 5, 6, 7, 8];
    let b: [u64; N] = [10, 20, 30, 40, 50, 60, 70, 80];
    let mut out = [0u64; N];
    vec_add(&a, &b, &mut out);

    // Expected: 11,22,...,88 → checksum
    let mut sum: u64 = 0;
    for i in 0..N {
        sum = sum.wrapping_add(out[i]);
    }
    print(b"VADD_SUM=0x");
    print_hex(sum);
    print(b"\n");
    // 11+22+...+88 = 99*4 = 396 = 0x18c
    if sum == 0x18c {
        print(b"VADD_OK\n");
    } else {
        print(b"VADD_BAD\n");
    }
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
