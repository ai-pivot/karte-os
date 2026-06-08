//! Minimal fd test
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[inline(always)]
unsafe fn syscall1(id: usize, a0: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") id => ret, in("rdi") a0, out("rcx") _,
    );
    ret
}
#[inline(always)]
unsafe fn syscall3(id: usize, a0: usize, a1: usize, a2: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") id => ret,
        in("rdi") a0, in("rsi") a1, in("rdx") a2, out("rcx") _,
    );
    ret
}
#[inline(always)]
unsafe fn syscall4(id: usize, a0: usize, a1: usize, a2: usize, a3: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") id => ret,
        in("rdi") a0, in("rsi") a1, in("rdx") a2, in("r10") a3, out("rcx") _,
    );
    ret
}

fn wr(s: &[u8]) { unsafe { syscall3(1, 1, s.as_ptr() as usize, s.len()); } }
fn do_close(fd: usize) { unsafe { syscall1(3, fd); } }
fn openat(path: &[u8], flags: usize) -> isize {
    unsafe { syscall4(257, 0xFFFFFFFFFFFFFF9C, path.as_ptr() as usize, flags, 0o644) }
}
fn pn(n: isize) {
    if n < 0 { wr(b"-"); pn(-n); return; }
    if n >= 10 { pn(n / 10); }
    wr(&[b'0' + (n % 10) as u8]);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    wr(b"FDTEST\n");
    
    let a = openat(b"/t1.txt\0", 0x242); // O_WRONLY|O_CREAT|O_TRUNC (Linux values)
    wr(b"a="); pn(a); wr(b"\n");
    
    let b = openat(b"/t2.txt\0", 0x242);
    wr(b"b="); pn(b); wr(b"\n");

    do_close(a as usize);
    let c = openat(b"/t3.txt\0", 0x242);
    wr(b"c="); pn(c); wr(b"\n");

    // No close — should be next fd
    let d = openat(b"/t4.txt\0", 0x242);
    wr(b"d="); pn(d); wr(b"\n");

    do_close(b as usize);
    do_close(c as usize);
    do_close(d as usize);

    let e = openat(b"/t5.txt\0", 0x242);
    wr(b"e="); pn(e); wr(b"\n");
    // NO close of e — open another file
    let f = openat(b"/t6.txt\0", 0x242);
    wr(b"f="); pn(f); wr(b"\n");

    wr(b"END\n");
    unsafe { core::arch::asm!("mov rax, 60; xor rdi, rdi; int 0x80"); }
    loop {}
}
