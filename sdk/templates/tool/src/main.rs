// karte-sdk tool template — CapDesc 工具脚手架
//
// 三处 TODO 填完即是一个可被脑发现与调用的工具：
//   1. device_id / tools 列表
//   2. 工具调用处理（switch 分发）
//   3. 工具的执行体
//
// 构建部署：cd user && make ARCH=riscv64 <name>.elf && ../tools/mkdisk.sh put

#![no_std]
#![no_main]

use core::arch::asm;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // 1. 填写设备自述
    let device_id = "mytool-1"; // TODO(1): 设备唯一 ID
    let tools = ["mytool.do_thing"]; // TODO(1): 暴露的工具名列表

    // —— CapDesc announce（KRT1 wire，与 kernel/src/drt.rs 协议一致）——
    // make_wire(b'A', seq, ...) 的用户态等价：直接用 sys_sendto 发往
    // DRT_PORT=43110；此处最小化用 debug_print 提示 + 死循环处理。
    print(b"[mytool] announcing device_id=");
    print(device_id.as_bytes());
    print(b"\n");

    loop {
        // 2. 工具调用分发（收到脑端 invoke 后在此 switch）——
        //    TODO(2): 读取 stdin/socket 请求，解析 tool 名
        // 3. 工具执行体 —— TODO(3): do_thing 的实现 + 返回
        unsafe {
            asm!("wfi");
        }
    }
}

fn print(s: &[u8]) {
    // sys_write(fd=1, buf, len) via ecall
    unsafe {
        core::arch::asm!(
            "li a7, 2", "li a0, 1",
            in("a1") s.as_ptr(), in("a2") s.len(),
            "ecall"
        );
    }
}

#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}
