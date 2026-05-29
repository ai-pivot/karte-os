// user/syscall.rs — 共享 syscall wrapper，供所有二进制使用
#![no_std]
#![allow(unsafe_op_in_unsafe_fn)]

pub const SYS_EXIT: usize = 1;
pub const SYS_WRITE: usize = 2;
pub const SYS_READ: usize = 3;
pub const SYS_OPEN: usize = 10;
pub const SYS_CLOSE: usize = 11;
pub const SYS_EXEC: usize = 32;
pub const SYS_LS: usize = 40;
pub const SYS_MKDIR: usize = 41;
pub const SYS_UNLINK: usize = 42;
pub const SYS_SETENV: usize = 50;
pub const SYS_GETENV: usize = 51;
pub const SYS_CHDIR: usize = 52;

#[inline(always)]
pub unsafe fn syscall1(id: usize, a0: usize) -> isize {
    let ret: isize;
    core::arch::asm!("ecall", in("a7") id, inlateout("a0") a0 => ret);
    ret
}

#[inline(always)]
pub unsafe fn syscall2(id: usize, a0: usize, a1: usize) -> isize {
    let ret: isize;
    core::arch::asm!("ecall", in("a7") id, inlateout("a0") a0 => ret, in("a1") a1);
    ret
}

#[inline(always)]
pub unsafe fn syscall3(id: usize, a0: usize, a1: usize, a2: usize) -> isize {
    let ret: isize;
    core::arch::asm!("ecall", in("a7") id, inlateout("a0") a0 => ret, in("a1") a1, in("a2") a2);
    ret
}

#[inline(always)]
pub unsafe fn syscall4(id: usize, a0: usize, a1: usize, a2: usize, a3: usize) -> isize {
    let ret: isize;
    core::arch::asm!("ecall", in("a7") id, inlateout("a0") a0 => ret, in("a1") a1, in("a2") a2, in("a3") a3);
    ret
}

// ── 输出 ──
pub fn print(s: &[u8]) {
    unsafe { syscall3(SYS_WRITE, 1, s.as_ptr() as usize, s.len()); }
}
pub fn println(s: &[u8]) { print(s); print(b"\r\n"); }

// ── 环境变量 ──
pub fn getenv(key: &[u8]) -> Option<[u8; 512]> {
    let mut buf = [0u8; 512];
    let n = unsafe { syscall4(SYS_GETENV, key.as_ptr() as usize, key.len(), buf.as_ptr() as usize, buf.len()) };
    if n > 0 && (n as usize) <= buf.len() {
        Some(buf)
    } else {
        None
    }
}

/// 获取命令行参数（从 CMD_ARGS 环境变量）
pub fn get_args() -> [u8; 512] {
    let mut args = [0u8; 512];
    match getenv(b"CMD_ARGS") {
        Some(buf) => {
            // Copy from 512-byte buf to 512-byte args
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len()).min(511);
            args[..len].copy_from_slice(&buf[..len]);
        }
        None => {}
    }
    args
}

/// Trim trailing \n, \r, spaces, nulls
pub fn trim(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && (s[end - 1] == b'\n' || s[end - 1] == b'\r' || s[end - 1] == b' ' || s[end - 1] == 0) {
        end -= 1;
    }
    let mut start = 0;
    while start < end && s[start] == b' ' { start += 1; }
    &s[start..end]
}

/// Find first space — returns (word, rest)
pub fn split_first(s: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 0;
    while i < s.len() && s[i] != b' ' { i += 1; }
    let first = &s[..i];
    let rest = if i < s.len() { &s[i+1..] } else { &s[i..] };
    (first, rest)
}

// ── helper ──
pub fn print_u64(n: u64) {
    if n == 0 { print(b"0"); return; }
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut m = n;
    while m > 0 { buf[i] = b'0' + (m % 10) as u8; m /= 10; i += 1; }
    for j in (0..i).rev() { print(&[buf[j]]); }
}
