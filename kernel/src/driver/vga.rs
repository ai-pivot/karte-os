//! VGA text mode driver for x86_64.
//!
//! Provides 80×25 character display via the VGA text buffer at physical address 0xB8000.
//! Supports cursor positioning, scrolling, and color attributes.
//! Only compiled for x86_64 targets.

#[cfg(target_arch = "x86_64")]
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

/// VGA text buffer physical (and virtual, via identity map) address.
const VGA_BUFFER: usize = 0xB8000;

/// Screen dimensions.
const COLS: usize = 80;
const ROWS: usize = 25;

/// VGA color attribute: light gray on black.
const DEFAULT_ATTR: u8 = 0x07;

/// VGA I/O port addresses for cursor control.
const CRT_CTRL: u16 = 0x3D4;
const CRT_DATA: u16 = 0x3D5;

/// Cursor position registers.
const CURSOR_HIGH: u8 = 0x0E;
const CURSOR_LOW: u8 = 0x0F;

/// Global VGA state.
static VGA_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Current cursor position (column, row).
static mut CURSOR_COL: usize = 0;
static mut CURSOR_ROW: usize = 0;

/// Write a byte to an I/O port.
#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, preserves_flags));
}

/// Update the hardware cursor position to match (CURSOR_COL, CURSOR_ROW).
unsafe fn update_hardware_cursor() {
    let pos = (CURSOR_ROW * COLS + CURSOR_COL) as u16;
    outb(CRT_CTRL, CURSOR_HIGH);
    outb(CRT_DATA, (pos >> 8) as u8);
    outb(CRT_CTRL, CURSOR_LOW);
    outb(CRT_DATA, (pos & 0xFF) as u8);
}

/// Write a character with attribute at (col, row).
#[inline(always)]
unsafe fn write_char(col: usize, row: usize, ch: u8, attr: u8) {
    let offset = (row * COLS + col) * 2;
    ptr::write_volatile((VGA_BUFFER + offset) as *mut u8, ch);
    ptr::write_volatile((VGA_BUFFER + offset + 1) as *mut u8, attr);
}

/// Scroll the screen up by one line. The bottom line is cleared.
unsafe fn scroll_up() {
    // Copy rows 1..ROWS-1 → rows 0..ROWS-2 (2 bytes per char)
    let src = VGA_BUFFER + COLS * 2; // row 1
    let dst = VGA_BUFFER; // row 0
    let len = COLS * (ROWS - 1) * 2;
    ptr::copy(src as *const u8, dst as *mut u8, len);

    // Clear the last row
    let last_row = VGA_BUFFER + COLS * (ROWS - 1) * 2;
    for col in 0..COLS {
        ptr::write_volatile((last_row + col * 2) as *mut u8, b' ');
        ptr::write_volatile((last_row + col * 2 + 1) as *mut u8, DEFAULT_ATTR);
    }
}

/// Clear the entire screen.
pub fn clear_screen() {
    unsafe {
        for row in 0..ROWS {
            for col in 0..COLS {
                write_char(col, row, b' ', DEFAULT_ATTR);
            }
        }
        CURSOR_COL = 0;
        CURSOR_ROW = 0;
        update_hardware_cursor();
    }
}

/// Write a single character to the VGA text console.
///
/// Handles:
/// - `\n` → newline (scroll if at bottom)
/// - `\r` → carriage return
/// - `\t` → tab (8-column aligned)
/// - `\x08` → backspace (move cursor left)
/// - Printable ASCII → display character
pub fn putchar(c: u8) {
    if !VGA_INITIALIZED.load(Ordering::Relaxed) {
        return;
    }

    unsafe {
        match c {
            b'\n' => {
                CURSOR_COL = 0;
                CURSOR_ROW += 1;
                if CURSOR_ROW >= ROWS {
                    scroll_up();
                    CURSOR_ROW = ROWS - 1;
                }
            }
            b'\r' => {
                CURSOR_COL = 0;
            }
            b'\t' => {
                // Align to next 8-column boundary
                let next_tab = (CURSOR_COL + 8) & !7;
                let spaces = if next_tab < COLS {
                    next_tab - CURSOR_COL
                } else {
                    COLS - CURSOR_COL
                };
                for _ in 0..spaces {
                    write_char(CURSOR_COL, CURSOR_ROW, b' ', DEFAULT_ATTR);
                    CURSOR_COL += 1;
                    if CURSOR_COL >= COLS {
                        CURSOR_COL = 0;
                        CURSOR_ROW += 1;
                        if CURSOR_ROW >= ROWS {
                            scroll_up();
                            CURSOR_ROW = ROWS - 1;
                        }
                        break;
                    }
                }
            }
            b'\x08' => {
                // Backspace: move cursor back one position
                if CURSOR_COL > 0 {
                    CURSOR_COL -= 1;
                    write_char(CURSOR_COL, CURSOR_ROW, b' ', DEFAULT_ATTR);
                }
            }
            _ => {
                if c >= 0x20 {
                    // Printable character
                    write_char(CURSOR_COL, CURSOR_ROW, c, DEFAULT_ATTR);
                    CURSOR_COL += 1;
                    if CURSOR_COL >= COLS {
                        CURSOR_COL = 0;
                        CURSOR_ROW += 1;
                        if CURSOR_ROW >= ROWS {
                            scroll_up();
                            CURSOR_ROW = ROWS - 1;
                        }
                    }
                }
            }
        }
        update_hardware_cursor();
    }
}

/// Initialize the VGA text mode driver.
///
/// Clears the screen and enables the hardware cursor.
/// Should be called early in x86_64 boot before console output begins.
pub fn init() {
    unsafe {
        // Enable cursor: set scan line start=0, end=15
        outb(CRT_CTRL, 0x0A); // Cursor Start
        outb(CRT_DATA, 0x00); // Start at scan line 0 (visible)
        outb(CRT_CTRL, 0x0B); // Cursor End
        outb(CRT_DATA, 0x0F); // End at scan line 15
    }
    clear_screen();
    VGA_INITIALIZED.store(true, Ordering::Relaxed);
    crate::console_println!("[vga] Text mode 80x25 initialized");
}
