//! Framebuffer text console with ANSI colors, CJK, 4K scaling.
//! 16-color VGA palette, SGR escape code parsing, boot splash.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(crate) static FB_ADDR: AtomicU64 = AtomicU64::new(0);
static FB_PITCH: AtomicU64 = AtomicU64::new(0);
static FB_WIDTH: AtomicU64 = AtomicU64::new(0);
static FB_HEIGHT: AtomicU64 = AtomicU64::new(0);
static FB_BPP: AtomicU64 = AtomicU64::new(0);
static FB_READY: AtomicBool = AtomicBool::new(false);

static CURSOR_X: AtomicU64 = AtomicU64::new(0);
static CURSOR_Y: AtomicU64 = AtomicU64::new(0);

const CHAR_W: usize = 8;
const CHAR_WIDE_W: usize = 16;
const CHAR_H: usize = 16;
static SCALE: AtomicU64 = AtomicU64::new(1);

// ─── ANSI color state ──────────────────────────────────────────
static FG_COLOR: AtomicU64 = AtomicU64::new(7); // default: white
static BG_COLOR: AtomicU64 = AtomicU64::new(0); // default: black
static IN_ESC: AtomicBool = AtomicBool::new(false);
static CSI_BUF: [AtomicU64; 8] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
static CSI_COUNT: AtomicU64 = AtomicU64::new(0);
static CSI_ACCUM: AtomicU64 = AtomicU64::new(0);
static CSI_Q: AtomicBool = AtomicBool::new(false);

/// 16-color VGA palette (RGB 8-bit per channel)
const PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00],
    [0xAA, 0x00, 0x00],
    [0x00, 0xAA, 0x00],
    [0xAA, 0x55, 0x00],
    [0x00, 0x00, 0xAA],
    [0xAA, 0x00, 0xAA],
    [0x00, 0xAA, 0xAA],
    [0xAA, 0xAA, 0xAA],
    [0x55, 0x55, 0x55],
    [0xFF, 0x55, 0x55],
    [0x55, 0xFF, 0x55],
    [0xFF, 0xFF, 0x55],
    [0x55, 0x55, 0xFF],
    [0xFF, 0x55, 0xFF],
    [0x55, 0xFF, 0xFF],
    [0xFF, 0xFF, 0xFF],
];

fn palette_rgb(idx: u8) -> u32 {
    let p = &PALETTE[(idx & 0x0F) as usize];
    ((p[2] as u32) << 16) | ((p[1] as u32) << 8) | (p[0] as u32)
}

fn fg_rgb() -> u32 {
    palette_rgb(FG_COLOR.load(Ordering::Relaxed) as u8)
}
fn bg_rgb() -> u32 {
    palette_rgb(BG_COLOR.load(Ordering::Relaxed) as u8)
}

// ─── Fonts ─────────────────────────────────────────────────────
static FONT: &[u8; 2048] = include_bytes!("font8x16.bin");
static CJK_FONT: &[u8] = include_bytes!("font16x16_cjk.bin");
const CJK_START: u32 = 0x4E00;
const CJK_COUNT: u32 = 8192;

static UTF8_STATE: AtomicU64 = AtomicU64::new(0);

#[inline]
fn scale() -> usize {
    SCALE.load(Ordering::Relaxed) as usize
}

#[inline]
fn is_wide(cp: u32) -> bool {
    cp >= 0x1100 && cp <= 0x115F
        || cp >= 0x2E80 && cp <= 0xA4CF
        || cp >= 0xAC00 && cp <= 0xD7A3
        || cp >= 0xF900 && cp <= 0xFAFF
        || cp >= 0xFE10 && cp <= 0xFE6F
        || cp >= 0xFF01 && cp <= 0xFF60
        || cp >= 0xFFE0 && cp <= 0xFFE6
}

fn utf8_decode(byte: u8) -> Option<u32> {
    let state = UTF8_STATE.load(Ordering::Relaxed);
    let remaining = (state >> 32) as u32;
    let codepoint = (state & 0xFFFF_FFFF) as u32;
    if byte & 0x80 == 0 {
        UTF8_STATE.store(0, Ordering::Relaxed);
        return Some(byte as u32);
    }
    if byte & 0xC0 == 0x80 {
        if remaining == 0 {
            return None;
        }
        let new_cp = (codepoint << 6) | (byte as u32 & 0x3F);
        if remaining == 1 {
            UTF8_STATE.store(0, Ordering::Relaxed);
            return Some(new_cp);
        }
        UTF8_STATE.store(
            ((remaining as u64 - 1) << 32) | new_cp as u64,
            Ordering::Relaxed,
        );
        return None;
    }
    let (rem, cp) = if byte & 0xE0 == 0xC0 {
        (1u32, (byte as u32 & 0x1F))
    } else if byte & 0xF0 == 0xE0 {
        (2u32, (byte as u32 & 0x0F))
    } else if byte & 0xF8 == 0xF0 {
        (3u32, (byte as u32 & 0x07))
    } else {
        return None;
    };
    UTF8_STATE.store(((rem as u64 - 1) << 32) | cp as u64, Ordering::Relaxed);
    None
}

// ─── Public API ────────────────────────────────────────────────
pub fn init(addr: usize, pitch: u32, width: u32, height: u32, bpp: u8) {
    FB_ADDR.store(addr as u64, Ordering::Relaxed);
    FB_PITCH.store(pitch as u64, Ordering::Relaxed);
    FB_WIDTH.store(width as u64, Ordering::Relaxed);
    FB_HEIGHT.store(height as u64, Ordering::Relaxed);
    FB_BPP.store(bpp as u64, Ordering::Relaxed);
    // Auto-detect scale factor based on screen width so text is readable
    // on high-DPI displays without manual configuration:
    //   width >= 3840 (4K)     → scale 3  (24×48 px per char, ~160 cols)
    //   width >= 2560 (1440p)  → scale 2  (16×32 px per char)
    //   otherwise              → scale 1  ( 8×16 px per char)
    let s: u64 = if width >= 3840 {
        3
    } else if width >= 2560 {
        2
    } else {
        1
    };
    SCALE.store(s, Ordering::Relaxed);
    FG_COLOR.store(7, Ordering::Relaxed);
    BG_COLOR.store(0, Ordering::Relaxed);
    FB_READY.store(true, Ordering::Relaxed);
}

pub fn framebuffer_region() -> Option<(usize, usize)> {
    if !FB_READY.load(Ordering::Relaxed) {
        return None;
    }
    let a = FB_ADDR.load(Ordering::Relaxed) as usize;
    let p = FB_PITCH.load(Ordering::Relaxed) as usize;
    let h = FB_HEIGHT.load(Ordering::Relaxed) as usize;
    let b = (FB_BPP.load(Ordering::Relaxed) as usize) / 8;
    if a == 0 || p == 0 || h == 0 || b == 0 {
        return None;
    }
    Some((a, p * h))
}

pub fn cols() -> usize {
    (FB_WIDTH.load(Ordering::Relaxed) as usize) / (CHAR_W * scale())
}
pub fn rows() -> usize {
    (FB_HEIGHT.load(Ordering::Relaxed) as usize) / (CHAR_H * scale())
}
pub fn cur_row() -> usize {
    CURSOR_Y.load(Ordering::Relaxed) as usize
}

/// Clear screen with current background color.
pub fn clear_screen() {
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let total =
        FB_PITCH.load(Ordering::Relaxed) as usize * FB_HEIGHT.load(Ordering::Relaxed) as usize;
    let bpp = FB_BPP.load(Ordering::Relaxed) as usize / 8;
    let bg = bg_rgb();
    // Fill all pixels with background color
    for y in 0..FB_HEIGHT.load(Ordering::Relaxed) as usize {
        let row_start = addr + y * FB_PITCH.load(Ordering::Relaxed) as usize;
        for x in 0..FB_WIDTH.load(Ordering::Relaxed) as usize {
            unsafe {
                core::ptr::write_volatile((row_start + x * bpp) as *mut u32, bg);
            }
        }
    }
    CURSOR_X.store(0, Ordering::Relaxed);
    CURSOR_Y.store(0, Ordering::Relaxed);
}

// ─── Rendering ─────────────────────────────────────────────────
fn clear_text_row(row: usize) {
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let pitch = FB_PITCH.load(Ordering::Relaxed) as usize;
    let width = FB_WIDTH.load(Ordering::Relaxed) as usize;
    let height = FB_HEIGHT.load(Ordering::Relaxed) as usize;
    let bpp = (FB_BPP.load(Ordering::Relaxed) as usize) / 8;
    let s = scale();
    let row_start_px = row * CHAR_H * s;
    let row_height = CHAR_H * s;
    if addr == 0 || pitch == 0 || width == 0 || height == 0 || bpp == 0 || row_start_px >= height {
        return;
    }
    let bg = bg_rgb();
    let end_y = (row_start_px + row_height).min(height);
    for y in row_start_px..end_y {
        let line = addr + y * pitch;
        for x in 0..width {
            unsafe {
                core::ptr::write_volatile((line + x * bpp) as *mut u32, bg);
            }
        }
    }
}

fn scroll_up() {
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let pitch = FB_PITCH.load(Ordering::Relaxed) as usize;
    let height = FB_HEIGHT.load(Ordering::Relaxed) as usize;
    let bpp = (FB_BPP.load(Ordering::Relaxed) as usize) / 8;
    if addr == 0 || pitch == 0 || height == 0 || bpp == 0 {
        return;
    }

    // pitch is already bytes per scanline. The old code multiplied by bpp
    // again, copying 4x past the framebuffer on 32bpp GOP modes.
    let total = height * pitch;
    let row_bytes = pitch * CHAR_H * scale();
    if row_bytes == 0 || row_bytes >= total {
        return;
    }
    unsafe {
        let dst = addr as *mut u8;
        let src = (addr + row_bytes) as *const u8;
        core::ptr::copy(src, dst, total - row_bytes);
    }
    clear_text_row(rows().saturating_sub(1));
}

fn put_pixel_s(px: usize, py: usize, color: u32) {
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let pitch = FB_PITCH.load(Ordering::Relaxed) as usize;
    let bpp = (FB_BPP.load(Ordering::Relaxed) / 8) as usize;
    let s = scale();
    let pyb = py * s;
    let pxb = px * s;
    for dy in 0..s {
        for dx in 0..s {
            let offset = (pyb + dy) * pitch + (pxb + dx) * bpp;
            unsafe {
                core::ptr::write_volatile((addr + offset) as *mut u32, color);
            }
        }
    }
}

fn draw_glyph_16(glyph_idx: usize, px: usize, py: usize, fg: u32, bg: u32) {
    if glyph_idx >= CJK_COUNT as usize {
        return;
    }
    let glyph = &CJK_FONT[glyph_idx * 32..][..32];
    for y in 0..CHAR_H {
        for x in 0..CHAR_WIDE_W {
            let byte_idx = y * 2 + x / 8;
            let bit_idx = 7 - (x % 8);
            let pixel = (glyph[byte_idx] >> bit_idx) & 1;
            put_pixel_s(px + x, py + y, if pixel != 0 { fg } else { bg });
        }
    }
}

fn draw_glyph_8(glyph_idx: usize, px: usize, py: usize, fg: u32, bg: u32) {
    let glyph = &FONT[glyph_idx * CHAR_H..][..CHAR_H];
    for y in 0..CHAR_H {
        let line = glyph[y];
        for x in 0..CHAR_W {
            let bit = (line >> (7 - x)) & 1;
            put_pixel_s(px + x, py + y, if bit != 0 { fg } else { bg });
        }
    }
}

fn draw_char_cp(cp: u32, col: usize, row: usize, fg: u32, bg: u32) {
    let s = scale();
    let px = col * CHAR_W * s;
    let py = row * CHAR_H * s;
    if is_wide(cp) {
        if cp >= CJK_START && cp < CJK_START + CJK_COUNT {
            draw_glyph_16((cp - CJK_START) as usize, px, py, fg, bg);
        } else {
            draw_glyph_8(b'?' as usize & 0x7F, px, py, fg, bg);
        }
    } else {
        draw_glyph_8((cp as usize) & 0x7F, px, py, fg, bg);
    }
}

fn advance_cursor(adv: usize) {
    let max_cols = cols();
    let max_rows = rows();
    let mut x = CURSOR_X.load(Ordering::Relaxed) as usize;
    let mut y = CURSOR_Y.load(Ordering::Relaxed) as usize;
    x += adv;
    if x >= max_cols {
        x = 0;
        y += 1;
    }
    if y >= max_rows {
        scroll_up();
        y = max_rows.saturating_sub(1);
    } else if x == 0 && adv > 0 {
        clear_text_row(y);
    }
    CURSOR_X.store(x as u64, Ordering::Relaxed);
    CURSOR_Y.store(y as u64, Ordering::Relaxed);
}

// ─── ANSI CSI parser ───────────────────────────────────────────
fn csi_dispatch(c: u8) {
    match c {
        b'm' => {
            let count = CSI_COUNT.load(Ordering::Relaxed) as usize;
            for i in 0..count {
                let v = CSI_BUF[i].load(Ordering::Relaxed) as u8;
                match v {
                    0 => {
                        FG_COLOR.store(7, Ordering::Relaxed);
                        BG_COLOR.store(0, Ordering::Relaxed);
                    }
                    1 => { /* bold = bright */ }
                    30..=37 => FG_COLOR.store((v - 30) as u64, Ordering::Relaxed),
                    40..=47 => BG_COLOR.store((v - 40) as u64, Ordering::Relaxed),
                    90..=97 => FG_COLOR.store((v - 90 + 8) as u64, Ordering::Relaxed),
                    100..=107 => BG_COLOR.store((v - 100 + 8) as u64, Ordering::Relaxed),
                    _ => {}
                }
            }
        }
        b'J' => {
            // CSI 2J = clear screen
            let v = CSI_BUF[0].load(Ordering::Relaxed);
            if v == 2 {
                clear_screen();
            }
        }
        b'H' => {
            // CSI row;col H = cursor position
            let r = CSI_BUF[0].load(Ordering::Relaxed) as usize;
            let c = CSI_BUF[1].load(Ordering::Relaxed) as usize;
            CURSOR_X.store((if c > 0 { c - 1 } else { 0 }) as u64, Ordering::Relaxed);
            CURSOR_Y.store((if r > 0 { r - 1 } else { 0 }) as u64, Ordering::Relaxed);
        }
        _ => {}
    }
    CSI_COUNT.store(0, Ordering::Relaxed);
    CSI_ACCUM.store(0, Ordering::Relaxed);
    IN_ESC.store(false, Ordering::Relaxed);
}

/// Process one byte through ANSI escape code parser. Returns true if byte was consumed.
fn ansi_feed(byte: u8) -> bool {
    if !IN_ESC.load(Ordering::Relaxed) && byte != 0x1B {
        return false;
    }
    // Start ESC sequence
    if byte == 0x1B {
        IN_ESC.store(true, Ordering::Relaxed);
        CSI_COUNT.store(0, Ordering::Relaxed);
        CSI_ACCUM.store(0, Ordering::Relaxed);
        CSI_Q.store(false, Ordering::Relaxed);
        return true;
    }
    if !IN_ESC.load(Ordering::Relaxed) {
        return false;
    }

    if byte == b'[' {
        return true;
    }

    if (b'0'..=b'9').contains(&byte) {
        let v = CSI_ACCUM.load(Ordering::Relaxed) * 10 + (byte - b'0') as u64;
        CSI_ACCUM.store(v, Ordering::Relaxed);
        return true;
    }

    if byte == b';' {
        let idx = CSI_COUNT.fetch_add(1, Ordering::Relaxed) as usize;
        if idx < 8 {
            CSI_BUF[idx].store(CSI_ACCUM.swap(0, Ordering::Relaxed), Ordering::Relaxed);
        }
        return true;
    }

    if byte == b'?' {
        CSI_Q.store(true, Ordering::Relaxed);
        return true;
    }

    // Final byte: dispatch
    let idx = CSI_COUNT.load(Ordering::Relaxed) as usize;
    if idx < 8 {
        CSI_BUF[idx].store(CSI_ACCUM.swap(0, Ordering::Relaxed), Ordering::Relaxed);
    }
    CSI_COUNT.fetch_add(1, Ordering::Relaxed);
    csi_dispatch(byte);
    true
}

// ─── putchar ───────────────────────────────────────────────────
pub(crate) fn putchar(byte: u8) {
    if !FB_READY.load(Ordering::Relaxed) {
        return;
    }

    // ANSI escape parsing
    if ansi_feed(byte) {
        return;
    }

    let max_rows = rows();
    match byte {
        b'\n' => {
            let _ = utf8_decode(b'\n');
            let y = CURSOR_Y.load(Ordering::Relaxed) as usize;
            if y + 1 >= max_rows {
                scroll_up();
                CURSOR_Y.store(max_rows.saturating_sub(1) as u64, Ordering::Relaxed);
            } else {
                let next = y + 1;
                clear_text_row(next);
                CURSOR_Y.store(next as u64, Ordering::Relaxed);
            }
            CURSOR_X.store(0, Ordering::Relaxed);
        }
        b'\r' => {
            CURSOR_X.store(0, Ordering::Relaxed);
        }
        _ => {
            if let Some(cp) = utf8_decode(byte) {
                let x = CURSOR_X.load(Ordering::Relaxed) as usize;
                let y = CURSOR_Y.load(Ordering::Relaxed) as usize;
                let fg = fg_rgb();
                let bg = bg_rgb();
                draw_char_cp(cp, x, y, fg, bg);
                advance_cursor(if is_wide(cp) { 2 } else { 1 });
            }
        }
    }
}

pub fn write_str(s: &str) {
    for &b in s.as_bytes() {
        putchar(b);
    }
}

/// Print a string centered horizontally on the current row.
pub fn centered(s: &str) {
    let w = s.len(); // rough (ASCII only)
    let col = cols();
    let pad = if col > w { (col - w) / 2 } else { 0 };
    for _ in 0..pad {
        putchar(b' ');
    }
    write_str(s);
    putchar(b'\n');
}

/// Draw a horizontal line of `ch` characters at the current row.
pub fn hline(ch: u8, w: usize) {
    let total = w.min(cols());
    for _ in 0..total {
        putchar(ch);
    }
    putchar(b'\n');
}

// ─── Boot splash ───────────────────────────────────────────────
pub fn boot_splash() {
    if !FB_READY.load(Ordering::Relaxed) {
        return;
    }
    let w = cols();
    clear_screen();

    // ── Top decorative bar ──
    FG_COLOR.store(0, Ordering::Relaxed); // text: black
    BG_COLOR.store(7, Ordering::Relaxed); // bg: bright white
    for _ in 0..w {
        putchar(b' ');
    } // fill row
    CURSOR_X.store(0, Ordering::Relaxed);

    // ── Brand line ──
    BG_COLOR.store(0, Ordering::Relaxed); // bg: black
    FG_COLOR.store(15, Ordering::Relaxed); // bright white
    CURSOR_Y.store(2, Ordering::Relaxed);
    centered("╔══════════════════════════════════════╗");
    centered("║       K a r t e O S    v 0 . 6      ║");
    centered("║    modern microkernel · Rust 2024    ║");
    centered("╚══════════════════════════════════════╝");

    // ── Decorative separator ──
    CURSOR_Y.store(
        (CURSOR_Y.load(Ordering::Relaxed) + 1) as u64,
        Ordering::Relaxed,
    );
    FG_COLOR.store(8, Ordering::Relaxed); // dark gray
    hline(b'-', w);

    // ── Reset to default white on black ──
    FG_COLOR.store(7, Ordering::Relaxed);
    BG_COLOR.store(0, Ordering::Relaxed);
    putchar(b'\n');
}

/// Print a boot log line: colored prefix + message.
pub fn boot_log(prefix: &str, prefix_color: u8, msg: &str) {
    FG_COLOR.store(prefix_color as u64, Ordering::Relaxed);
    write_str(prefix);
    FG_COLOR.store(7, Ordering::Relaxed); // white
    write_str(" ");
    write_str(msg);
    putchar(b'\n');
}

// ─── Debug square ──────────────────────────────────────────────
/// Writes a colored square to the GOP framebuffer for visual diagnostics.
/// Disabled in release builds to avoid QEMU iothread assertion during
/// context switches (VGA MMIO write in __switch can trigger
/// qemu_mutex_lock_iothread_impl assertion in QEMU 8.2.x).
#[unsafe(no_mangle)]
pub extern "C" fn fb_debug_square(_slot: usize, _color: u32) {
    // no-op in release builds
}
