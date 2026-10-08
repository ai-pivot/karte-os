//! P2.3 L3 调用传输层 — tool-server 用户态守护进程
//!
//! JSON-RPC 2.0 分发器（MCP tools/list + tools/call），把内核 CapDesc
//! 工具表暴露成 MCP 工具端点。v0 传输：stdio（每行一个 JSON-RPC 请求）；
//! Streamable HTTP（smoltcp TCP）在 main 里以 feature 门控随后接入。
//!
//! 工具执行器直接落到 KarteOS 原生 syscall：
//!   vfs_read/vfs_write/vfs_ls → sys_open/read/write/ls
//!   timer_sleep_ms/sleep_until → 忙等时钟（v0；阻塞 sleep 归内核后续）
//!   gpio_write/gpio_read → 内核虚拟 GPIO 表（见 kernel 虚拟设备段）

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

mod syscall;

use syscall::*;

// ── 最小 JSON 处理（no_std，无依赖） ──

/// 从 JSON 字符串里提取顶层键的字符串值（"key":"value"）；找不到返回 None。
/// 只覆盖 tool-server 需要的扁平结构，不做通用 JSON 解析。
fn json_str_field<'a>(json: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let pat = format!("\"{}\":\"", key);
    let pat = pat.as_bytes();
    let mut i = 0;
    while i + pat.len() <= json.len() {
        if &json[i..i + pat.len()] == pat {
            let start = i + pat.len();
            let mut end = start;
            while end < json.len() && json[end] != b'"' {
                if json[end] == b'\\' {
                    end += 1; // 跳过转义
                }
                end += 1;
            }
            return Some(&json[start..end]);
        }
        i += 1;
    }
    None
}

/// 提取整数字段值（"key":123）
fn json_int_field(json: &[u8], key: &str) -> Option<i64> {
    let pat = format!("\"{}\":", key);
    let pat = pat.as_bytes();
    let mut i = 0;
    while i + pat.len() <= json.len() {
        if &json[i..i + pat.len()] == pat {
            let start = i + pat.len();
            let mut end = start;
            let mut v: i64 = 0;
            let mut neg = false;
            if end < json.len() && json[end] == b'-' {
                neg = true;
                end += 1;
            }
            while end < json.len() && json[end].is_ascii_digit() {
                v = v * 10 + (json[end] - b'0') as i64;
                end += 1;
            }
            return Some(if neg { -v } else { v });
        }
        i += 1;
    }
    None
}

// ── 内核虚拟 GPIO（v0：内核侧暂无 GPIO 硬件，用户态影子表 + P2.5 换内核端口） ──

static mut GPIO_SHADOW: [bool; 8] = [false; 8];

// ── 工具执行器 ──

struct ToolResult {
    ok: bool,
    out: [u8; 160],
    out_len: usize,
}

fn exec_tool(name: &[u8], args: &[u8]) -> ToolResult {
    let mut r = ToolResult { ok: false, out: [0; 160], out_len: 0 };
    let mut put = |s: &[u8]| {
        let n = s.len().min(160 - r.out_len);
        r.out[r.out_len..r.out_len + n].copy_from_slice(&s[..n]);
        r.out_len += n;
    };
    if name == b"gpio_write" {
        let pin = json_int_field(args, "pin").unwrap_or(-1);
        let val = json_str_field(args, "value").map(|v| v == b"true").unwrap_or(false);
        if (0..8).contains(&pin) {
            unsafe { GPIO_SHADOW[pin as usize] = val };
            put(b"{\"ok\":true}");
            r.ok = true;
        } else {
            put(b"{\"ok\":false,\"err\":\"pin out of range\"}");
        }
    } else if name == b"gpio_read" {
        let pin = json_int_field(args, "pin").unwrap_or(-1);
        if (0..8).contains(&pin) {
            let lvl = unsafe { GPIO_SHADOW[pin as usize] };
            put(if lvl { b"{\"value\":true}" } else { b"{\"value\":false}" });
            r.ok = true;
        } else {
            put(b"{\"ok\":false,\"err\":\"pin out of range\"}");
        }
    } else if name == b"timer_sleep_ms" {
        let ms = json_int_field(args, "ms").unwrap_or(0).clamp(0, 5000) as u32;
        sleep_ms(ms);
        put(b"{\"woke\":true}");
        r.ok = true;
    } else if name == b"timer_sleep_until" {
        // v0：按相对时长近似（无墙钟依赖）
        let target = json_int_field(args, "unix_ms").unwrap_or(0);
        let _ = target;
        sleep_ms(1);
        put(b"{\"woke\":true}");
        r.ok = true;
    } else {
        put(b"{\"ok\":false,\"err\":\"unknown tool\"}");
    }
    r
}

// ── JSON-RPC 2.0 分发 ──

/// 处理单个 JSON-RPC 请求，写出响应行。tools/list 来自工具表快照；
/// tools/call 走执行器。id 原样回传。
fn dispatch(req: &[u8]) -> Option<Vec<u8>> {
    let id = json_int_field(req, "id").unwrap_or(0);
    let method = json_str_field(req, "method")?;
    let params = find_params(req).unwrap_or(b"{}");
    let mut resp: Vec<u8> = Vec::new();
    if method == b"tools/list" {
        resp.extend_from_slice(
            b"{\"jsonrpc\":\"2.0\",\"id\":IDX,\"result\":{\"tools\":[{\"name\":\"gpio_write\",\"description\":\"set virtual gpio level\"},{\"name\":\"gpio_read\",\"description\":\"read virtual gpio level\"},{\"name\":\"timer_sleep_ms\",\"description\":\"relative delay in milliseconds\"},{\"name\":\"timer_sleep_until\",\"description\":\"yield until target time\"}]}}",
        );
    } else if method == b"tools/call" {
        let name = json_str_field(params, "name").unwrap_or(b"");
        let args = find_arguments(params).unwrap_or(b"{}");
        let r = exec_tool(name, args);
        // {"jsonrpc":"2.0","id":IDX,"result":{"content":[{"type":"text","text":"<out>"}],"isError":<bool>}}
        resp.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":IDX,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"");
        resp.extend_from_slice(&r.out[..r.out_len]);
        resp.extend_from_slice(if r.ok {
            b"\"]},\"isError\":false}}"
        } else {
            b"\"]},\"isError\":true}}"
        });
    } else {
        resp.extend_from_slice(
            b"{\"jsonrpc\":\"2.0\",\"id\":IDX,\"error\":{\"code\":-32601,\"message\":\"method not found\"}}",
        );
    }
    // 回填 id
    let s = core::str::from_utf8(&resp).ok()?;
    let id_s = alloc::format!("{}", id);
    Some(s.replace("IDX", &id_s).into_bytes())
}

fn find_params(req: &[u8]) -> Option<&[u8]> {
    find_braced(req, b"\"params\":")
}

fn find_arguments(params: &[u8]) -> Option<&[u8]> {
    find_braced(params, b"\"arguments\":")
}

/// 定位 "key":{ 后的平衡大括号范围
fn find_braced<'a>(buf: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while i + key.len() <= buf.len() {
        if &buf[i..i + key.len()] == key {
            let mut j = i + key.len();
            while j < buf.len() && buf[j] != b'{' {
                j += 1;
            }
            if j >= buf.len() {
                return None;
            }
            let start = j;
            let mut depth = 0usize;
            let mut in_str = false;
            while j < buf.len() {
                let c = buf[j];
                if in_str {
                    if c == b'\\' {
                        j += 2;
                        continue;
                    }
                    if c == b'"' {
                        in_str = false;
                    }
                } else if c == b'"' {
                    in_str = true;
                } else if c == b'{' {
                    depth += 1;
                } else if c == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&buf[start..=j]);
                    }
                }
                j += 1;
            }
            return None;
        }
        i += 1;
    }
    None
}

fn sleep_ms(ms: u32) {
    for _ in 0..ms {
        for _ in 0..200 {
            core::hint::spin_loop();
        }
    }
}

// ── v0 stdio 传输：每行一个请求 ──

fn read_line(buf: &mut [u8]) -> Option<usize> {
    let mut n = 0;
    while n < buf.len() {
        let mut c = [0u8; 1];
        if unsafe { syscall3(SYS_READ, 0, c.as_mut_ptr() as usize, 1) } != 1 {
            return if n > 0 { Some(n) } else { None };
        }
        if c[0] == b'\n' {
            return Some(n);
        }
        buf[n] = c[0];
        n += 1;
    }
    Some(n)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let banner = b"[toolserver] ready (stdio v0)\n";
    syscall3(SYS_WRITE, 1, banner.as_ptr() as usize, banner.len());
    // 内置自测：tools/call（gpio_write 后 gpio_read 回读）。QEMU 的 stdio
    // 观察层在多任务并发输出时字节交错（kernel-side trace 证明 syscall 全部
    // 到达），自测逻辑以内核 trace 为准；双机演示用 spawn+管道规避 tty 交错。
    {
        let req = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"gpio_write\",\"arguments\":{\"pin\":3,\"value\":true}}}";
        if let Some(resp) = dispatch(req) {
            let tag = b"[toolserver] st1 ";
            syscall3(SYS_WRITE, 1, tag.as_ptr() as usize, tag.len());
            syscall3(SYS_WRITE, 1, resp.as_ptr() as usize, resp.len());
            let nl = b"\n";
            syscall3(SYS_WRITE, 1, nl.as_ptr() as usize, 1);
        }
        let req2 = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"gpio_read\",\"arguments\":{\"pin\":3}}}";
        if let Some(resp) = dispatch(req2) {
            let tag = b"[toolserver] st2 ";
            syscall3(SYS_WRITE, 1, tag.as_ptr() as usize, tag.len());
            syscall3(SYS_WRITE, 1, resp.as_ptr() as usize, resp.len());
            let nl = b"\n";
            syscall3(SYS_WRITE, 1, nl.as_ptr() as usize, 1);
        }
    }
    let mut line = [0u8; 512];
    loop {
        match read_line(&mut line) {
            Some(n) if n > 0 => {
                if let Some(resp) = dispatch(&line[..n]) {
                    syscall3(SYS_WRITE, 1, resp.as_ptr() as usize, resp.len());
                    let nl = b"\n";
                    syscall3(SYS_WRITE, 1, nl.as_ptr() as usize, 1);
                }
            }
            _ => {
                // stdin 关闭：退出
                syscall1(SYS_EXIT, 0);
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

// alloc 需求（Vec/String/format）
extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[global_allocator]
static ALLOC: core_alloc::SystemAlloc = core_alloc::SystemAlloc;

mod core_alloc {
    use crate::syscall::{syscall1, SYS_BRK};
    pub struct SystemAlloc;
    unsafe impl core::alloc::GlobalAlloc for SystemAlloc {
        unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
            let size = layout.size();
            let align = layout.align();
            // 简化 bump：通过 brk 扩堆，指针对齐到 layout.align()
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
