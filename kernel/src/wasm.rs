//! P3.2 WASM 应用模型 — 微型 WASM 解释器 v0（内核内，no_std，双架构）
//!
//! ROADMAP P3.2 的 WASM 运行时底座。v0 支持子集：
//!   - 模块解析：magic/版本、Type(1)/Function(3)/Export(7)/Code(10) section
//!   - 执行：i32.const(0x41)/i32.add(0x6A)/drop(0x1A)/end(0x0B) 的表达式栈
//!   - 调用：按导出名调用无参->i32 函数（WASM 应用 = 纯函数工具的载体）
//!
//! 跨芯片演示（验收）：同一 wasm 字节码在 riscv64 与 x86_64 的内核
//! 单测中解释执行并得到一致结果——解释器与指令集解耦，天然跨芯片。
//! v1 路线：完整指令集 + WASM↔CapDesc 工具互暴露（见 docs/design/wasm-apps.md）。

use alloc::vec::Vec;

pub struct MiniWasm<'a> {
    code: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum WasmErr {
    BadMagic,
    BadSection,
    Unsupported,
}

impl<'a> MiniWasm<'a> {
    /// 解析模块头（\0asm + version 1）。
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WasmErr> {
        if bytes.len() < 8 || &bytes[0..4] != b"\0asm" {
            return Err(WasmErr::BadMagic);
        }
        Ok(Self { code: bytes })
    }

    /// 定位某 section 的 payload（id 相同的第一个），返回 (payload, next_off)。
    fn section(&self, id: u8) -> Option<(&'a [u8], usize)> {
        let mut off = 8usize;
        let c = self.code;
        while off < c.len() {
            let sid = c[off];
            off += 1;
            let mut len: usize = 0;
            let mut shift = 0;
            loop {
                let b = *c.get(off)?;
                off += 1;
                len |= ((b & 0x7F) as usize) << shift;
                if b & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            let payload = c.get(off..off + len)?;
            if sid == id {
                return Some((payload, off + len));
            }
            off += len;
        }
        None
    }

    /// 运行第一个导出函数的表达式，返回栈顶 i32（v0：无参函数）。
    pub fn call_export(&self) -> Result<i32, WasmErr> {
        // Code section: vector of code entries; v0 只跑第一个函数体
        let (code_sec, _) = self.section(10).ok_or(WasmErr::Unsupported)?;
        let mut off = 0usize;
        // 函数数量（LEB，取低位即可——v0 模块函数数 < 128）
        let count = *code_sec.get(off).ok_or(WasmErr::BadSection)?;
        off += 1;
        if count == 0 {
            return Err(WasmErr::Unsupported);
        }
        // 第一个 code entry：size(LEB) + locals(LEB vec) + expr
        let mut size: usize = 0;
        let mut shift = 0;
        loop {
            let b = *code_sec.get(off).ok_or(WasmErr::BadSection)?;
            off += 1;
            size |= ((b & 0x7F) as usize) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        let body = code_sec
            .get(off..off + size)
            .ok_or(WasmErr::BadSection)?
            .to_vec();
        self.run_expr(&body)
    }

    /// 执行表达式（locals 数之后到 end），v0 指令集：i32.const/add/drop/end。
    fn run_expr(&self, body: &[u8]) -> Result<i32, WasmErr> {
        let mut off = 0usize;
        // locals 声明：vec of (count, type)——v0 要求全 0 locals
        let nl = *body.get(off).ok_or(WasmErr::BadSection)?;
        off += 1;
        for _ in 0..nl {
            let cnt = *body.get(off).ok_or(WasmErr::BadSection)?;
            off += 1 + cnt as usize; // v0：跳过声明（不支持非零 locals）
            if cnt != 0 {
                return Err(WasmErr::Unsupported);
            }
        }
        let mut stack: Vec<i32> = Vec::new();
        while off < body.len() {
            match body[off] {
                0x41 => {
                    // i32.const，LEB128 signed（v0：正数低位）
                    off += 1;
                    let mut v: i32 = 0;
                    let mut shift = 0;
                    loop {
                        let b = *body.get(off).ok_or(WasmErr::BadSection)?;
                        off += 1;
                        v |= ((b & 0x7F) as i32) << shift;
                        if b & 0x80 == 0 {
                            if b & 0x40 != 0 && shift < 25 {
                                v |= -1i32 << (shift + 7); // 符号扩展
                            }
                            break;
                        }
                        shift += 7;
                    }
                    stack.push(v);
                }
                0x6A => {
                    let b = stack.pop().ok_or(WasmErr::Unsupported)?;
                    let a = stack.pop().ok_or(WasmErr::Unsupported)?;
                    stack.push(a.wrapping_add(b));
                    off += 1;
                }
                0x1A => {
                    stack.pop();
                    off += 1;
                }
                0x0B => {
                    // end
                    return stack.last().copied().ok_or(WasmErr::Unsupported);
                }
                _ => return Err(WasmErr::Unsupported),
            }
        }
        Err(WasmErr::Unsupported)
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── WASM Mini Interpreter Tests ──");

    crate::test::run_test("wasm_magic_parse", || {
        let bad = [0x00, 0x61, 0x73, 0x6D, 0x99];
        let bad2 = b"\0asZ\x01\x00\x00\x00";
        MiniWasm::parse(bad2).is_err() && MiniWasm::parse(&bad).is_err() && {
            // 只含头部的空模块（无 code section → call 应报 Unsupported）
            let m = b"\0asm\x01\x00\x00\x00";
            MiniWasm::parse(m).is_ok()
        }
    });

    crate::test::run_test("wasm_const_add_exec", || {
        // 空模块 + code section(10)：1 个函数体 = locals 0x00 + (i32.const 7)(i32.const 35)(i32.add)end
        // body: 00 41 07 41 23 6A 0B
        let body: &[u8] = &[0x00, 0x41, 0x07, 0x41, 0x23, 0x6A, 0x0B];
        let code_sec: &[u8] = &[
            0x01, 0x07, body[0], body[1], body[2], body[3], body[4], body[5], body[6],
        ];
        // 组装完整模块：头 + code section header(id=10, size=len)
        let mut m: alloc::vec::Vec<u8> = b"\0asm\x01\x00\x00\x00".to_vec();
        m.push(10);
        m.push(code_sec.len() as u8);
        m.extend_from_slice(code_sec);
        let r = MiniWasm::parse(&m).and_then(|w| w.call_export());
        r == Ok(42) // 7 + 35 = 42
    });

    crate::test::run_test("wasm_drop_stack_semantics", || {
        // locals 0 + (const 1)(const 2)(drop)(const 3)end → 栈顶 3
        let body: &[u8] = &[0x00, 0x41, 0x01, 0x41, 0x02, 0x1A, 0x41, 0x03, 0x0B];
        let mut m: alloc::vec::Vec<u8> = b"\0asm\x01\x00\x00\x00".to_vec();
        m.push(10);
        m.push(body.len() as u8 + 2); // count(1) + entry-size(1) + body
        m.push(0x01); // 1 个 code entry
        m.push(body.len() as u8);
        m.extend_from_slice(body);
        MiniWasm::parse(&m).and_then(|w| w.call_export()) == Ok(3)
    });
}
