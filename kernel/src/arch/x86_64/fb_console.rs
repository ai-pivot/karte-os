//! Framebuffer text console using GOP framebuffer from multiboot2.
//! Fallback when legacy VGA (0xB8000) is not available (UEFI without CSM).
//! Uses a built-in 8x16 bitmap font for character rendering.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Framebuffer state — initialized once from multiboot2
static FB_ADDR:   AtomicU64 = AtomicU64::new(0);
static FB_PITCH:  AtomicU64 = AtomicU64::new(0);
static FB_WIDTH:  AtomicU64 = AtomicU64::new(0);
static FB_HEIGHT: AtomicU64 = AtomicU64::new(0);
static FB_BPP:    AtomicU64 = AtomicU64::new(0);
static FB_READY:  AtomicBool = AtomicBool::new(false);

// Console cursor
static CURSOR_X: AtomicU64 = AtomicU64::new(0);
static CURSOR_Y: AtomicU64 = AtomicU64::new(0);

const CHAR_W: usize = 8;
const CHAR_H: usize = 16;

// ─── Builtin 8x16 font (first 128 ASCII characters) ────────────────────
// Simplified font — just enough glyphs for readable text
static FONT: &[u8; 2048] = include_bytes!("font8x16.bin");

/// Initialize framebuffer console from multiboot2 framebuffer info
pub fn init(addr: usize, pitch: u32, width: u32, height: u32, bpp: u8) {
    FB_ADDR.store(addr as u64, Ordering::Relaxed);
    FB_PITCH.store(pitch as u64, Ordering::Relaxed);
    FB_WIDTH.store(width as u64, Ordering::Relaxed);
    FB_HEIGHT.store(height as u64, Ordering::Relaxed);
    FB_BPP.store(bpp as u64, Ordering::Relaxed);
    FB_READY.store(true, Ordering::Relaxed);
    CURSOR_X.store(0, Ordering::Relaxed);
    CURSOR_Y.store(0, Ordering::Relaxed);
}

pub fn is_ready() -> bool {
    FB_READY.load(Ordering::Relaxed)
}

pub fn cols() -> usize {
    (FB_WIDTH.load(Ordering::Relaxed) as usize) / CHAR_W
}

pub fn rows() -> usize {
    (FB_HEIGHT.load(Ordering::Relaxed) as usize) / CHAR_H
}

/// Scroll the screen up by one line
fn scroll_up() {
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let pitch = FB_PITCH.load(Ordering::Relaxed) as usize;
    let height = FB_HEIGHT.load(Ordering::Relaxed) as usize;
    let bpp = FB_BPP.load(Ordering::Relaxed) as usize;
    let row_bytes = pitch * CHAR_H;
    let skip = row_bytes;
    let total = height * pitch * (bpp / 8);
    let move_size = total - skip;
    // Move all lines up by one character row
    unsafe {
        let dst = addr as *mut u8;
        let src = (addr + row_bytes) as *const u8;
        core::ptr::copy(src, dst, move_size);
        // Clear last line
        let last_line_start = addr + total - row_bytes;
        core::ptr::write_bytes(last_line_start as *mut u8, 0, row_bytes);
    }
}

/// Draw a single character at (col, row) using the built-in font
fn draw_char(c: u8, col: usize, row: usize) {
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let pitch = FB_PITCH.load(Ordering::Relaxed) as usize;
    let bpp = (FB_BPP.load(Ordering::Relaxed) / 8) as usize;

    let glyph = &FONT[(c as usize) * CHAR_H..(c as usize + 1) * CHAR_H];
    let px = col * CHAR_W;
    let py = row * CHAR_H;

    for y in 0..CHAR_H {
        let line = glyph[y];
        for x in 0..CHAR_W {
            let pixel = (line >> (7 - x)) & 1;
            let offset = (py + y) * pitch + (px + x) * bpp;
            if pixel != 0 {
                // White pixel
                unsafe {
                    let p = (addr + offset) as *mut u32;
                    core::ptr::write_volatile(p, 0xFFFFFFFF);
                }
            } else {
                // Black pixel
                unsafe {
                    let p = (addr + offset) as *mut u32;
                    core::ptr::write_volatile(p, 0x00000000);
                }
            }
        }
    }
}

/// Put a single character on the framebuffer console
pub fn putchar(c: u8) {
    if !FB_READY.load(Ordering::Relaxed) {
        return;
    }

    let max_cols = cols();
    let max_rows = rows();

    match c {
        b'\n' => {
            let y = CURSOR_Y.load(Ordering::Relaxed) as usize;
            if y + 1 >= max_rows {
                scroll_up();
            } else {
                CURSOR_Y.store((y + 1) as u64, Ordering::Relaxed);
            }
            CURSOR_X.store(0, Ordering::Relaxed);
        }
        b'\r' => {
            CURSOR_X.store(0, Ordering::Relaxed);
        }
        _ => {
            let mut x = CURSOR_X.load(Ordering::Relaxed) as usize;
            let y = CURSOR_Y.load(Ordering::Relaxed) as usize;

            draw_char(c, x, y);

            x += 1;
            if x >= max_cols {
                x = 0;
                if y + 1 >= max_rows {
                    scroll_up();
                } else {
                    CURSOR_Y.store((y + 1) as u64, Ordering::Relaxed);
                }
            }
            CURSOR_X.store(x as u64, Ordering::Relaxed);
        }
    }
}

/// Write a string to the framebuffer console
pub fn write_str(s: &str) {
    for &b in s.as_bytes() {
        putchar(b);
    }
}
