//! tui_test.rs — Simple TUI test with ANSI escape sequences
//! Tests: cursor movement, color output, screen clear, box drawing

#![no_std]
#![no_main]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

const SYS_IOCTL: usize = 80;
const TCSETS: usize = 0x5402;
const TERM_RAW: usize = 1;
const TERM_ECHO_OFF: usize = 3;
const TIOCGWINSZ: usize = 0x5413;

fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as usize, s.len());
    }
}

fn write_bytes(b: &[u8]) {
    unsafe {
        syscall3(SYS_WRITE, 1, b.as_ptr() as usize, b.len());
    }
}

fn sleep_ms(ms: u64) {
    // nanosleep via syscall 35
    let ts: [u64; 2] = [0, ms * 1_000_000];
    unsafe {
        syscall2(35, ts.as_ptr() as usize, 0);
    }
}

// ANSI escape helpers
fn clear_screen() { write_str("\x1b[2J"); }
fn cursor_home() { write_str("\x1b[H"); }
fn set_fg(n: u8) {
    write_bytes(b"\x1b[38;5;");
    // write number as decimal
    let mut buf = [0u8; 4];
    let mut n = n as usize;
    let mut i = 0;
    if n == 0 { buf[0] = b'0'; i = 1; }
    else {
        let mut digits = [0u8; 4];
        let mut d = 0;
        while n > 0 { digits[d] = b'0' + (n % 10) as u8; n /= 10; d += 1; }
        while d > 0 { d -= 1; buf[i] = digits[d]; i += 1; }
    }
    write_bytes(&buf[..i]);
    write_str("m");
}
fn reset_style() { write_str("\x1b[0m"); }
fn bold() { write_str("\x1b[1m"); }
fn move_cursor(row: u8, col: u8) {
    write_str("\x1b[");
    write_byte(row);
    write_str(";");
    write_byte(col);
    write_str("H");
}
fn write_byte(b: u8) {
    let mut buf = [0u8; 4];
    let mut n = b as usize;
    let mut i = 0;
    if n == 0 { buf[0] = b'0'; i = 1; }
    else {
        let mut digits = [0u8; 4];
        let mut d = 0;
        while n > 0 { digits[d] = b'0' + (n % 10) as u8; n /= 10; d += 1; }
        while d > 0 { d -= 1; buf[i] = digits[d]; i += 1; }
    }
    write_bytes(&buf[..i]);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // Step 1: Basic text output
    write_str("=== TUI Test ===\n\r");
    write_str("Testing ANSI escapes...\n\r");
    sleep_ms(100);

    // Step 2: Color output
    set_fg(196); // bright red
    bold();
    write_str("RED BOLD TEXT");
    reset_style();
    write_str("\n\r");

    set_fg(46); // bright green
    write_str("GREEN TEXT");
    reset_style();
    write_str("\n\r");

    set_fg(39); // bright blue
    write_str("BLUE TEXT");
    reset_style();
    write_str("\n\r");

    write_str("Colors OK!\n\r");
    sleep_ms(100);

    // Step 3: Clear screen and draw a box
    clear_screen();
    cursor_home();
    sleep_ms(50);

    // Draw border box
    let top = 3u8;
    let left = 5u8;
    let width = 30u8;
    let height = 8u8;

    // Top border
    move_cursor(top, left);
    write_str("\u{250c}"); // ┌
    for _ in 0..(width-2) { write_str("\u{2500}"); } // ─
    write_str("\u{2510}"); // ┐

    // Side borders
    for row in (top+1)..(top+height-1) {
        move_cursor(row, left);
        write_str("\u{2502}"); // │
        move_cursor(row, left + width - 1);
        write_str("\u{2502}"); // │
    }

    // Bottom border
    move_cursor(top + height - 1, left);
    write_str("\u{2514}"); // └
    for _ in 0..(width-2) { write_str("\u{2500}"); } // ─
    write_str("\u{2518}"); // ┘

    // Title inside box
    move_cursor(top + 1, left + 2);
    set_fg(226); // yellow
    bold();
    write_str("KarteOS TUI Test");
    reset_style();

    // Content
    move_cursor(top + 3, left + 2);
    write_str("ANSI colors:   ");
    set_fg(196); write_str("R"); set_fg(46); write_str("A"); set_fg(39); write_str("I"); set_fg(226); write_str("N"); set_fg(201); write_str("B"); set_fg(51); write_str("O"); set_fg(214); write_str("W");
    reset_style();

    move_cursor(top + 4, left + 2);
    write_str("Box drawing:   OK");

    move_cursor(top + 5, left + 2);
    write_str("Cursor move:   OK");

    // Progress bar animation
    move_cursor(top + 7, left + 2);
    write_str("Progress: [");
    for i in 0..12u8 {
        sleep_ms(80);
        set_fg(46);
        write_str("\u{2588}"); // █ full block
        reset_style();
    }
    write_str("] DONE");

    // Move cursor below box and exit
    move_cursor(top + height + 2, 1);
    write_str("TUI test complete!\n\r");
    sleep_ms(200);

    syscall1(SYS_EXIT, 0);
    loop {}
}
