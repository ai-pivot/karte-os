//! P3.3 IoT 协议 — MQTT 3.1.1 用户态客户端（QoS1 最小集）
//!
//! 端到端路径：QEMU user-net (10.0.2.2:1883) → host 侧
//! tools/mqtt-mini-broker.py。流程：CONNECT → CONNACK →
//! SUBSCRIBE karteo/cmd/# → SUBACK → PUBLISH karteo/telemetry →
//! PUBACK → PINGREQ → PINGRESP → DISCONNECT。
//! wire 格式与内核 mqtt.rs 单测锁定的一致。

#![no_std]
#![no_main]
#![allow(static_mut_refs)]
#![allow(unsafe_op_in_unsafe_fn)]

mod syscall;
use syscall::*;

extern crate alloc;

#[global_allocator]
static ALLOC: core_alloc::SystemAlloc = core_alloc::SystemAlloc;

mod core_alloc {
    use crate::syscall::{syscall1, SYS_BRK};

    pub struct SystemAlloc;
    unsafe impl core::alloc::GlobalAlloc for SystemAlloc {
        unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
            let size = layout.size();
            let align = layout.align();
            static mut HEAP_CUR: usize = 0;
            static mut HEAP_END: usize = 0;
            unsafe {
                let cur = HEAP_CUR;
                if cur == 0 {
                    let top = syscall1(SYS_BRK, 0) as usize;
                    HEAP_CUR = top;
                    HEAP_END = top;
                }
                let aligned = (HEAP_CUR + align - 1) & !(align - 1);
                let new_end = aligned + size;
                if new_end > HEAP_END {
                    let need = (new_end - HEAP_END + 4095) & !4095;
                    let got = syscall1(SYS_BRK, HEAP_END + need) as usize;
                    if got < new_end {
                        return core::ptr::null_mut();
                    }
                    HEAP_END = HEAP_END + need;
                }
                HEAP_CUR = new_end;
                aligned as *mut u8
            }
        }
        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
    }
}

// ── MQTT 包构造（与内核 mqtt.rs 同一 wire 格式） ──

fn remaining_len(n: usize) -> alloc_vec::Vec<u8> {
    let mut out = alloc_vec::Vec::new();
    let mut n = n;
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
    out
}

mod alloc_vec {
    extern crate alloc;
    pub use alloc::vec::Vec;
}

fn push_str(out: &mut alloc_vec::Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn connect(client_id: &str) -> alloc_vec::Vec<u8> {
    let mut body = alloc_vec::Vec::new();
    push_str(&mut body, "MQTT");
    body.push(4);
    body.push(0x02);
    body.extend_from_slice(&60u16.to_be_bytes());
    push_str(&mut body, client_id);
    let mut out = alloc_vec::Vec::new();
    out.push(0x10);
    out.extend(remaining_len(body.len()));
    out.extend(body);
    out
}

fn subscribe(topic: &str, pid: u16) -> alloc_vec::Vec<u8> {
    let mut body = alloc_vec::Vec::new();
    body.extend_from_slice(&pid.to_be_bytes());
    push_str(&mut body, topic);
    body.push(1);
    let mut out = alloc_vec::Vec::new();
    out.push(0x82);
    out.extend(remaining_len(body.len()));
    out.extend(body);
    out
}

fn publish(topic: &str, payload: &[u8], pid: u16) -> alloc_vec::Vec<u8> {
    let mut body = alloc_vec::Vec::new();
    push_str(&mut body, topic);
    body.extend_from_slice(&pid.to_be_bytes());
    body.extend_from_slice(payload);
    let mut out = alloc_vec::Vec::new();
    out.push(0x32);
    out.extend(remaining_len(body.len()));
    out.extend(body);
    out
}

// ── TCP via KarteOS socket syscalls ──

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const BROKER_IP: [u8; 4] = [10, 0, 2, 2];
const BROKER_PORT: u16 = 1883;

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16, // network order
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

unsafe fn readn(fd: usize, buf: &mut [u8]) -> isize {
    let mut got = 0usize;
    while got < buf.len() {
        let n = syscall6(
            SYS_RECVFROM,
            fd,
            buf[got..].as_mut_ptr() as usize,
            buf.len() - got,
            0,
            0,
            0,
        );
        if n <= 0 {
            if got > 0 {
                return got as isize;
            }
            return n;
        }
        got += n as usize;
    }
    got as isize
}

unsafe fn expect(fd: usize, first: u8, name: &str) -> bool {
    let mut h = [0u8; 1];
    if syscall6(SYS_RECVFROM, fd, h.as_mut_ptr() as usize, 1, 0, 0, 0) != 1 {
        return false;
    }
    if h[0] != first {
        return false;
    }
    // remaining len（broker 响应均 < 128，单字节）
    let mut l = [0u8; 1];
    if syscall6(SYS_RECVFROM, fd, l.as_mut_ptr() as usize, 1, 0, 0, 0) != 1 {
        return false;
    }
    let mut body = [0u8; 64];
    let n = l[0] as usize;
    if n > 0 && readn(fd, &mut body[..n]) != n as isize {
        return false;
    }
    put(name);
    put(" OK\n");
    true
}

static mut OUTBUF: [u8; 128] = [0; 128];
static mut OUTLEN: usize = 0;

fn put(s: &str) {
    unsafe {
        for &b in s.as_bytes() {
            if OUTLEN < 128 {
                OUTBUF[OUTLEN] = b;
                OUTLEN += 1;
            }
        }
    }
}

fn flush() {
    unsafe {
        syscall3(SYS_WRITE, 1, OUTBUF.as_ptr() as usize, OUTLEN);
        OUTLEN = 0;
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    put("[mqtt] client up, connecting 10.0.2.2:1883\n");
    flush();
    let fd = syscall3(SYS_SOCKET, AF_INET, SOCK_STREAM, 0);
    if fd < 0 {
        put("[mqtt] socket failed\n");
        flush();
        syscall1(SYS_EXIT, 1);
    }
    // smoltcp 的 TCP connect 需要多轮 poll（SYN/SYN-ACK），客户端重试等待
    let sa = SockAddrIn {
        sin_family: AF_INET as u16,
        sin_port: BROKER_PORT.to_be(),
        sin_addr: BROKER_IP,
        sin_zero: [0; 8],
    };
    let mut ok = false;
    for i in 0..10 {
        let r = syscall3(
            SYS_CONNECT,
            fd as usize,
            &sa as *const SockAddrIn as usize,
            core::mem::size_of::<SockAddrIn>(),
        );
        if r == 0 {
            ok = true;
            break;
        }
        if i == 0 {
            put("[mqtt] waiting for TCP establish...\n");
            flush();
        }
        for _ in 0..200_000 {
            core::hint::spin_loop();
        }
    }
    if !ok {
        put("[mqtt] connect failed\n");
        flush();
        syscall1(SYS_EXIT, 1);
    }
    // CONNECT -> CONNACK
    let c = connect("karte-node");
    if syscall6(SYS_SENDTO, fd as usize, c.as_ptr() as usize, c.len(), 0, 0, 0) != c.len() as isize {
        put("[mqtt] send CONNECT failed\n");
        flush();
        syscall1(SYS_EXIT, 1);
    }
    if !expect(fd as usize, 0x20, "CONNACK") {
        put("[mqtt] no CONNACK\n");
        flush();
        syscall1(SYS_EXIT, 1);
    }
    // SUBSCRIBE karteo/cmd/#
    let s = subscribe("karteo/cmd/#", 1);
    let _ = syscall6(SYS_SENDTO, fd as usize, s.as_ptr() as usize, s.len(), 0, 0, 0);
    if !expect(fd as usize, 0x90, "SUBACK") {
        put("[mqtt] no SUBACK\n");
        flush();
        syscall1(SYS_EXIT, 1);
    }
    // PUBLISH karteo/telemetry (QoS1) -> PUBACK
    let p = publish("karteo/telemetry", b"{\"tier\":\"mcu\",\"t\":22.5}", 2);
    let _ = syscall6(SYS_SENDTO, fd as usize, p.as_ptr() as usize, p.len(), 0, 0, 0);
    if !expect(fd as usize, 0x40, "PUBACK") {
        put("[mqtt] no PUBACK\n");
        flush();
        syscall1(SYS_EXIT, 1);
    }
    put("[mqtt] MQTT QoS1 round trip OK (connect/subscribe/publish)\n");
    flush();
    syscall1(SYS_EXIT, 0);
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
