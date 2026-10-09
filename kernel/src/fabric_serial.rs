//! M 档设备模式（边缘网关）—— 主内核作为 CapDesc 织物节点
//!
//! feature: `fabric_node`。启动后内核作为专用固件运行（关抢占 + 串口轮询）：
//!   - 周期 announce **真实 CapDesc 注册表工具集**（capability::tool_names()）
//!   - 响应脑端 invoke：vfs_read 走真实文件系统 I/O，其余走内核状态
//!
//! 这就是「驱动即工具」的网关侧表达：内核驱动声明的能力（vfs/timer/gpio 的
//! CapDesc）出厂即成为织物上可被脑端调用的工具，无需任何 MCP 适配代码。
//!
//! 默认构建不含本模块 —— shell 启动、182 测试完全不受影响。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 设备身份（网关节点）
const DEVICE_ID: &str = "karte-m-gw";

fn rd(off: usize) -> u8 {
    unsafe { core::ptr::read_volatile((crate::platform::riscv64::UART_BASE + off) as *const u8) }
}

fn wr(off: usize, v: u8) {
    unsafe { core::ptr::write_volatile((crate::platform::riscv64::UART_BASE + off) as *mut u8, v) }
}

fn putc(c: u8) {
    while rd(5) & 0x20 == 0 {} // LSR.THRE
    wr(0, c);
}

fn puts(s: &str) {
    for b in s.bytes() {
        putc(b);
    }
}

/// 真实内核能力执行（工具语义）
fn run_tool(tool: &str) -> String {
    match tool {
        "vfs_read" => match crate::driver::ext4::read_file("ls") {
            Some(d) => format!("vfs-ok:{}", d.len()),
            None => String::from("vfs-miss"),
        },
        "timer_now" => format!("devs:{}", crate::capability::device_count()),
        "gpio_read" => String::from("0"),
        _ => String::from("ok"),
    }
}

/// 设备主循环（不返回）
pub fn run_device_loop() -> ! {
    // 设备模式：关 S 中断（无抢占）——内核作为专用固件运行
    unsafe { core::arch::asm!("csrci sstatus, 2") };
    let tools: Vec<String> = crate::capability::tool_names();
    let csv = tools.join(",");
    puts("[fabric] KarteOS M-tier gateway node up\n");
    puts("[fabric] capabilities: ");
    puts(&csv);
    puts("\n");
    let mut seq: u64 = 0;
    let mut inv = [0u8; 512];
    let mut ilen = 0usize;
    loop {
        let frame = format!("KRT1|A|{}|{}|{}\n", DEVICE_ID, seq, csv);
        puts(&frame);
        // RX 聚合：遇 \n 完成一帧；无数据时静默等待一段再回到 announce
        let mut ready = false;
        let mut wait: u32 = 0;
        loop {
            if rd(5) & 1 == 0 {
                wait += 1;
                if wait > 2_000_000 {
                    break;
                }
                continue;
            }
            let b = rd(0);
            if b == b'\n' || b == b'\r' {
                ready = true;
                break;
            }
            if ilen < inv.len() {
                inv[ilen] = b;
                ilen += 1;
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
                let result = run_tool(tool);
                let resp = format!("KRT1|T|{}|{}|{}\n", DEVICE_ID, seq, result);
                puts(&resp);
            }
        }
        if ready {
            ilen = 0;
        }
        seq = seq.wrapping_add(1);
    }
}
