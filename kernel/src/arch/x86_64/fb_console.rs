//! Framebuffer text console using GOP framebuffer.
//! 8x16 ASCII font + 16x16 CJK font, UTF-8, double-width, 4K auto-scaling.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(crate) static FB_ADDR:   AtomicU64 = AtomicU64::new(0);
static FB_PITCH:  AtomicU64 = AtomicU64::new(0);
static FB_WIDTH:  AtomicU64 = AtomicU64::new(0);
static FB_HEIGHT: AtomicU64 = AtomicU64::new(0);
static FB_BPP:    AtomicU64 = AtomicU64::new(0);
static FB_READY:  AtomicBool = AtomicBool::new(false);

static CURSOR_X: AtomicU64 = AtomicU64::new(0);
static CURSOR_Y: AtomicU64 = AtomicU64::new(0);

const CHAR_W: usize = 8;
const CHAR_WIDE_W: usize = 16;
const CHAR_H: usize = 16;
static SCALE: AtomicU64 = AtomicU64::new(1);

static FONT: &[u8; 2048] = include_bytes!("font8x16.bin");
static CJK_FONT: &[u8] = include_bytes!("font16x16_cjk.bin");
const CJK_START: u32 = 0x4E00;
const CJK_COUNT: u32 = 8192;

static UTF8_STATE: AtomicU64 = AtomicU64::new(0);

#[inline] fn scale() -> usize { SCALE.load(Ordering::Relaxed) as usize }

#[inline]
fn is_wide(cp: u32) -> bool {
    cp >= 0x1100 && cp <= 0x115F
    || cp >= 0x2E80 && cp <= 0xA4CF
    || cp >= 0xAC00 && cp <= 0xD7A3
    || cp >= 0xF900 && cp <= 0xFAFF
    || cp >= 0xFE10 && cp <= 0xFE1F
    || cp >= 0xFE30 && cp <= 0xFE6F
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
        if remaining == 0 { return None; }
        let new_cp = (codepoint << 6) | (byte as u32 & 0x3F);
        if remaining == 1 {
            UTF8_STATE.store(0, Ordering::Relaxed);
            return Some(new_cp);
        }
        UTF8_STATE.store(((remaining as u64 - 1) << 32) | new_cp as u64, Ordering::Relaxed);
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

pub fn init(addr: usize, pitch: u32, width: u32, height: u32, bpp: u8) {
    FB_ADDR.store(addr as u64, Ordering::Relaxed);
    FB_PITCH.store(pitch as u64, Ordering::Relaxed);
    FB_WIDTH.store(width as u64, Ordering::Relaxed);
    FB_HEIGHT.store(height as u64, Ordering::Relaxed);
    FB_BPP.store(bpp as u64, Ordering::Relaxed);
    let s: u64 = if width >= 3840 || height >= 2160 { 2 } else { 1 };
    SCALE.store(s, Ordering::Relaxed);
    FB_READY.store(true, Ordering::Relaxed);
}

pub fn framebuffer_region() -> Option<(usize, usize)> {
    if !FB_READY.load(Ordering::Relaxed) { return None; }
    let a = FB_ADDR.load(Ordering::Relaxed) as usize;
    let p = FB_PITCH.load(Ordering::Relaxed) as usize;
    let h = FB_HEIGHT.load(Ordering::Relaxed) as usize;
    let b = (FB_BPP.load(Ordering::Relaxed) as usize) / 8;
    if a == 0 || p == 0 || h == 0 || b == 0 { return None; }
    Some((a, p * h))
}

pub fn cols() -> usize { (FB_WIDTH.load(Ordering::Relaxed) as usize) / (CHAR_W * scale()) }
pub fn rows() -> usize { (FB_HEIGHT.load(Ordering::Relaxed) as usize) / (CHAR_H * scale()) }

fn scroll_up() {
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let pitch = FB_PITCH.load(Ordering::Relaxed) as usize;
    let height = FB_HEIGHT.load(Ordering::Relaxed) as usize;
    let bpp = FB_BPP.load(Ordering::Relaxed) as usize / 8;
    let total = height * pitch * bpp;
    let row_bytes = pitch * CHAR_H * scale();
    unsafe {
        let dst = addr as *mut u8;
        let src = (addr + row_bytes) as *const u8;
        core::ptr::copy(src, dst, total - row_bytes);
        core::ptr::write_bytes((addr + total - row_bytes) as *mut u8, 0, row_bytes);
    }
}

fn put_pixel_s(px: usize, py: usize, color: u32) {
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let pitch = FB_PITCH.load(Ordering::Relaxed) as usize;
    let bpp = (FB_BPP.load(Ordering::Relaxed) / 8) as usize;
    let s = scale();
    let py_base = py * s;
    let px_base = px * s;
    for dy in 0..s {
        for dx in 0..s {
            let offset = (py_base + dy) * pitch + (px_base + dx) * bpp;
            unsafe { core::ptr::write_volatile((addr + offset) as *mut u32, color); }
        }
    }
}

fn draw_glyph_16(glyph_idx: usize, px: usize, py: usize) {
    if glyph_idx >= CJK_COUNT as usize { return; }
    let glyph = &CJK_FONT[glyph_idx * 32..][..32];
    for y in 0..CHAR_H {
        for x in 0..CHAR_WIDE_W {
            let byte_idx = y * 2 + x / 8;
            let bit_idx = 7 - (x % 8);
            let pixel = (glyph[byte_idx] >> bit_idx) & 1;
            let color: u32 = if pixel != 0 { 0xFFFFFFFF } else { 0x00000000 };
            put_pixel_s(px + x, py + y, color);
        }
    }
}

fn draw_glyph_8(glyph_idx: usize, px: usize, py: usize) {
    let glyph = &FONT[glyph_idx * CHAR_H..][..CHAR_H];
    for y in 0..CHAR_H {
        let line = glyph[y];
        for x in 0..CHAR_W {
            let bit = (line >> (7 - x)) & 1;
            put_pixel_s(px + x, py + y, if bit != 0 { 0xFFFFFFFF } else { 0x00000000 });
        }
    }
}

fn draw_char_cp(cp: u32, col: usize, row: usize) {
    let s = scale();
    let px = col * CHAR_W * s;
    let py = row * CHAR_H * s;
    if is_wide(cp) {
        if cp >= CJK_START && cp < CJK_START + CJK_COUNT {
            draw_glyph_16((cp - CJK_START) as usize, px, py);
        } else {
            draw_glyph_8(b'?' as usize & 0x7F, px, py);
        }
    } else {
        draw_glyph_8((cp as usize) & 0x7F, px, py);
    }
}

fn advance_cursor(adv: usize) {
    let max_cols = cols();
    let max_rows = rows();
    let mut x = CURSOR_X.load(Ordering::Relaxed) as usize;
    let mut y = CURSOR_Y.load(Ordering::Relaxed) as usize;
    x += adv;
    if x >= max_cols { x = 0; y += 1; }
    if y >= max_rows { scroll_up(); y = max_rows - 1; }
    CURSOR_X.store(x as u64, Ordering::Relaxed);
    CURSOR_Y.store(y as u64, Ordering::Relaxed);
}

pub fn putchar(byte: u8) {
    if !FB_READY.load(Ordering::Relaxed) { return; }
    let max_rows = rows();
    match byte {
        b'\n' => {
            let _ = utf8_decode(b'\n');
            let y = CURSOR_Y.load(Ordering::Relaxed) as usize;
            if y + 1 >= max_rows { scroll_up(); }
            else { CURSOR_Y.store((y + 1) as u64, Ordering::Relaxed); }
            CURSOR_X.store(0, Ordering::Relaxed);
        }
        b'\r' => { CURSOR_X.store(0, Ordering::Relaxed); }
        _ => {
            if let Some(cp) = utf8_decode(byte) {
                let x = CURSOR_X.load(Ordering::Relaxed) as usize;
                let y = CURSOR_Y.load(Ordering::Relaxed) as usize;
                draw_char_cp(cp, x, y);
                advance_cursor(if is_wide(cp) { 2 } else { 1 });
            }
        }
    }
}

pub fn write_str(s: &str) { for &b in s.as_bytes() { putchar(b); } }

#[unsafe(no_mangle)]
pub extern "C" fn fb_debug_square(slot: usize, color: u32) {
    if !FB_READY.load(Ordering::Relaxed) { return; }
    let addr = FB_ADDR.load(Ordering::Relaxed) as usize;
    let pitch = FB_PITCH.load(Ordering::Relaxed) as usize;
    let bpp = (FB_BPP.load(Ordering::Relaxed) / 8) as usize;
    let width = FB_WIDTH.load(Ordering::Relaxed) as usize;
    if addr == 0 || pitch == 0 || bpp < 4 || width == 0 { return; }
    let size = 32usize;
    let gap = 6usize;
    let x0 = 16 + slot * (size + gap);
    if x0 + size >= width { return; }
    for y in 0..size {
        for x in 0..size {
            let offset = y * pitch + (x0 + x) * bpp;
            unsafe { core::ptr::write_volatile((addr + offset) as *mut u32, color); }
        }
    }
}
