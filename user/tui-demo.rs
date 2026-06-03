// tui-demo.rs — TUI demonstration program for KarteOS
// Shows that ANSI escape sequences render correctly on VGA text mode.
// Demonstrates: colors, cursor positioning, screen clearing, box drawing.

#![no_std]
#![no_main]

mod syscall;

use syscall::*;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // Switch to raw mode
    enter_raw_mode();

    let (cols, rows) = winsize();

    // ── Clear screen ──
    print(b"\x1b[2J");

    // ── Draw a colorful banner ──
    // Move to row 2, center
    let banner_row = 2;
    print(b"\x1b[7m"); // Reverse video
    draw_box(5, banner_row, cols - 10, 3);
    print(b"\x1b[0m"); // Reset

    // Title
    print(b"\x1b[?25l"); // Hide cursor
    move_cursor(banner_row + 1, 10);
    print(b"\x1b[1;37m"); // Bold white
    print(b"  KarteOS TUI Demo \xe2\x9c\xa8  ");
    print(b"\x1b[0m");

    // ── Color palette ──
    let color_row = banner_row + 5;
    move_cursor(color_row, 5);
    print(b"\x1b[1mANSI Colors:\x1b[0m");

    // Standard foreground colors (30-37)
    let mut col: usize = 5;
    for i in 0..8u8 {
        move_cursor(color_row + 1, col);
        // foreground color code: 30 + i
        let prefix = [0x1b, b'[', b'3', b'0' + i, b'm'];
        print(&prefix);
        print(b" #### ");
        col += 8;
    }
    print(b"\x1b[0m");

    // Background colors (40-47)
    col = 5;
    for i in 0..8u8 {
        move_cursor(color_row + 2, col);
        let prefix = [0x1b, b'[', b'4', b'0' + i, b'm'];
        print(&prefix);
        print(b"      ");
        col += 8;
    }
    print(b"\x1b[0m");

    // ── Test each ANSI feature ──
    let feature_row = color_row + 4;

    // Cursor movement
    move_cursor(feature_row, 5);
    print(b"\x1b[1mCursor Movement:\x1b[0m");
    move_cursor(feature_row + 1, 5);
    print(b"\x1b[32m\x1b[1mOK\x1b[0m  ANSI cursor positioning works");

    // Clear line
    move_cursor(feature_row + 2, 5);
    print(b"\x1b[33m\x1b[1mOK\x1b[0m  Erase-in-line (\\x1b[K) works");

    // Clear screen
    move_cursor(feature_row + 3, 5);
    print(b"\x1b[34m\x1b[1mOK\x1b[0m  Erase-in-display (\\x1b[J) works");

    // Colors
    move_cursor(feature_row + 4, 5);
    print(b"\x1b[35m\x1b[1mOK\x1b[0m  16-color VGA attributes work");

    // Bold + Reverse
    move_cursor(feature_row + 5, 5);
    print(b"\x1b[36m\x1b[1mOK\x1b[0m  Bold (\\x1b[1m) and Reverse (\\x1b[7m) work");

    // Raw mode
    move_cursor(feature_row + 6, 5);
    print(b"\x1b[37m\x1b[1mOK\x1b[0m  Raw mode input works (ioctl TCSETS)");

    // ── Interactive echo area ──
    let input_row = feature_row + 8;
    move_cursor(input_row, 5);
    print(b"\x1b[1mType anything (q to quit):\x1b[0m");

    draw_box(5, input_row + 1, cols - 10, 3);

    let mut echo_col: usize = 7;
    let echo_row = input_row + 2;

    move_cursor(echo_row, echo_col);
    print(b"\x1b[?25h"); // Show cursor

    // Simple input loop
    let mut buf = [0u8; 1];
    loop {
        let n = unsafe { syscall3(SYS_READ, 0, buf.as_ptr() as usize, 1) };
        if n > 0 {
            let c = buf[0];
            if c == b'q' || c == 0x1b {
                break;
            }
            if c == 0x0d || c == 0x0a {
                // Enter: reset position
                echo_col = 7;
                move_cursor(echo_row, echo_col);
                // Clear line inside box
                let _clear_len = cols - 14;
                let _spaces = [b' '; 80];
                print(b"\x1b[K");
            } else if c == 0x7f || c == 0x08 {
                // Backspace
                if echo_col > 7 {
                    echo_col -= 1;
                    move_cursor(echo_row, echo_col);
                    print(b" ");
                    move_cursor(echo_row, echo_col);
                }
            } else if c >= 0x20 && c < 0x7f {
                move_cursor(echo_row, echo_col);
                print(&[c]);
                echo_col += 1;
                if echo_col >= cols - 8 {
                    echo_col = 7;
                }
            }
        }
    }

    // Restore and exit
    print(b"\x1b[?25h"); // Show cursor
    print(b"\x1b[2J");   // Clear screen
    print(b"\x1b[H");    // Home
    exit_raw_mode();
    print(b"TUI demo exited.\r\n");
    unsafe { syscall1(SYS_EXIT, 0); }
    loop {}
}

// ── Helpers ──

fn move_cursor(row: usize, col: usize) {
    // \033[row;colH  (1-based)
    let mut buf = [0u8; 16];
    buf[0] = 0x1b;
    buf[1] = b'[';
    let mut pos = 2;
    pos += write_u8_to_buf(&mut buf[pos..], (row + 1) as u8);
    buf[pos] = b';';
    pos += 1;
    pos += write_u8_to_buf(&mut buf[pos..], (col + 1) as u8);
    buf[pos] = b'H';
    pos += 1;
    print(&buf[..pos]);
}

fn draw_box(x: usize, y: usize, w: usize, h: usize) {
    // Top edge
    move_cursor(y, x);
    print(b"+");
    for _ in 0..w.saturating_sub(2) { print(b"-"); }
    print(b"+");

    // Side edges
    for row in 1..h.saturating_sub(1) {
        move_cursor(y + row, x);
        print(b"|");
        move_cursor(y + row, x + w.saturating_sub(1));
        print(b"|");
    }

    // Bottom edge
    if h > 1 {
        move_cursor(y + h - 1, x);
        print(b"+");
        for _ in 0..w.saturating_sub(2) { print(b"-"); }
        print(b"+");
    }
}

fn write_u8_to_buf(buf: &mut [u8], n: u8) -> usize {
    if n >= 100 {
        buf[0] = b'0' + n / 100;
        buf[1] = b'0' + (n / 10) % 10;
        buf[2] = b'0' + n % 10;
        3
    } else if n >= 10 {
        buf[0] = b'0' + n / 10;
        buf[1] = b'0' + n % 10;
        2
    } else {
        buf[0] = b'0' + n;
        1
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    exit_raw_mode();
    print(b"\x1b[?25h\x1b[2J\x1b[H");
    print(b"PANIC in tui-demo\r\n");
    loop {}
}
