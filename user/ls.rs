// user/ls.rs — list filesystem contents
#![no_std]
#![no_main]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let mut buf = [0u8; 4096];
    // Read CMD_ARGS — first word is the directory path (or empty for CWD)
    let mut path_buf = [0u8; 256];
    let path_len = match getenv(b"CMD_ARGS") {
        Some(b) => {
            let end = b.iter().position(|&x| x == 0).unwrap_or(b.len());
            let l = if end > 0 && b[end - 1] == b'\n' { end - 1 } else { end };
            let l = l.min(255);
            for i in 0..l { path_buf[i] = b[i]; }
            l
        }
        None => 0,
    };
    let path = if path_len > 0 { &path_buf[..path_len] } else { &[] };
    let n = ls_dir(path, &mut buf);
    if n > 0 {
        print(&buf[..n as usize]);
    }
    syscall1(SYS_EXIT, 0);
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
