//! VGA text mode driver for x86_64 with full ANSI escape sequence support.
//!
//! Provides 80×25 character display via the VGA text buffer at physical address 0xB8000.
//! Supports cursor positioning, scrolling, color attributes, and ANSI CSI sequences
//! for TUI application compatibility (vim-style full-screen programs).
//!
//! ANSI CSI sequences handled:
//!   \033[H          - Cursor home
//!   \033[row;colH   - Cursor move to (row,col)  (1-based)
//!   \033[A/B/C/D    - Cursor up/down/right/left
//!   \033[J          - Erase from cursor to end of screen
//!   \033[2J         - Clear entire screen
//!   \033[K          - Erase from cursor to end of line
//!   \033[1K         - Erase from start of line to cursor
//!   \033[2K         - Erase entire line
//!   \033[X          - Erase N characters at cursor
//!   \033[m          - Reset attributes
//!   \033[30-37m     - Set foreground color (ANSI 0-7)
//!   \033[90-97m     - Set foreground color (bright ANSI 8-15)
//!   \033[40-47m     - Set background color (ANSI 0-7)
//!   \033[100-107m   - Set background color (bright ANSI 8-15)
//!   \033[1m         - Bold (bright foreground)
//!   \033[7m         - Reverse video
//!   \033[?25h       - Show cursor
//!   \033[?25l       - Hide cursor
//!   \033[s          - Save cursor position
//!   \033[u          - Restore cursor position
//!   \033[6n         - Device Status Report (respond with \033[row;colR)

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

/// Raw mode flag: when true, kernel console output is suppressed from VGA.
/// This prevents kernel log messages from corrupting TUI programs.
/// Set by tty::set_mode(Raw), cleared by tty::set_mode(Canonical).
static VGA_RAW_MODE: AtomicBool = AtomicBool::new(false);

/// Check if VGA is in raw mode (TUI program owns the screen).
pub fn is_raw_mode() -> bool {
    VGA_RAW_MODE.load(Ordering::Relaxed)
}

/// Set VGA raw mode flag.
pub fn set_raw_mode(enabled: bool) {
    VGA_RAW_MODE.store(enabled, Ordering::Relaxed);
}

// ─── ANSI Escape Sequence Parser State ──────────────────────────────────

/// CSI parameter buffer max
const CSI_MAX_PARAMS: usize = 16;

#[derive(Clone, Copy, PartialEq)]
enum ParseState {
    Normal,
    Escape, // Received ESC (\x1b)
    Csi,    // Received ESC [ — collecting parameters
    Osc,    // Received ESC ] — collecting OSC string (skip until BEL/ST)
}

/// Global ANSI parser state (only accessed from putchar, single-threaded)
static mut PARSE_STATE: ParseState = ParseState::Normal;
static mut CSI_PARAMS: [u16; CSI_MAX_PARAMS] = [0; CSI_MAX_PARAMS];
static mut CSI_PARAM_COUNT: usize = 0;
static mut CSI_HAS_QUESTION: bool = false; // For \033[?... sequences

// ─── Cursor & Attribute State ───────────────────────────────────────────

static mut CURSOR_COL: usize = 0;
static mut CURSOR_ROW: usize = 0;
static mut CURSOR_VISIBLE: bool = true;

/// Current VGA attribute byte (foreground | background << 4)
static mut CURRENT_FG: u8 = 7; // Light gray
static mut CURRENT_BG: u8 = 0; // Black
static mut BOLD: bool = false;
static mut REVERSE: bool = false;
static mut BLINK: bool = false;
static mut FAINT: bool = false; // Dim (mapped to darker fg)
static mut CONCEALED: bool = false; // Hidden text

/// Cached attribute byte (recomputed only when SGR changes, not per-char)
static mut CACHED_ATTR: u8 = 0x07;

/// Scroll region (DECSTBM): lines outside [top, bottom] are not scrolled.
/// Default: full screen (0..ROWS-1).
static mut SCROLL_TOP: usize = 0;
static mut SCROLL_BOTTOM: usize = ROWS - 1;

/// Saved cursor position
static mut SAVED_COL: usize = 0;
static mut SAVED_ROW: usize = 0;

/// Alternate screen buffer (for \033[?1049h / \033[?47h)
static mut ALT_SCREEN_SAVED: bool = false;
static mut ALT_SCREEN_BUF: [u8; COLS * ROWS * 2] = [0; COLS * ROWS * 2];

// ─── I/O helpers ────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, preserves_flags));
}

unsafe fn update_hardware_cursor() {
    if !CURSOR_VISIBLE {
        // Hide cursor by moving it off-screen
        outb(CRT_CTRL, CURSOR_HIGH);
        outb(CRT_DATA, 0xFF);
        outb(CRT_CTRL, CURSOR_LOW);
        outb(CRT_DATA, 0xFF);
        return;
    }
    let pos = (CURSOR_ROW * COLS + CURSOR_COL) as u16;
    outb(CRT_CTRL, CURSOR_HIGH);
    outb(CRT_DATA, (pos >> 8) as u8);
    outb(CRT_CTRL, CURSOR_LOW);
    outb(CRT_DATA, (pos & 0xFF) as u8);
}

// ─── Attribute computation ──────────────────────────────────────────────

/// Compute the VGA attribute byte from current state.
unsafe fn current_attr() -> u8 {
    let mut fg = CURRENT_FG;
    let bg = CURRENT_BG;

    // Bold makes foreground bright (add 8)
    if BOLD && fg < 8 {
        fg += 8;
    }

    // Concealed text: fg = bg (invisible)
    if CONCEALED {
        return ((bg & 0x0F) << 4) | (bg & 0x0F);
    }

    let base = if REVERSE {
        // Swap foreground and background
        ((bg & 0x0F) << 4) | (fg & 0x0F)
    } else {
        ((bg & 0x0F) << 4) | (fg & 0x0F)
    };

    // Blink uses bit 7
    if BLINK { base | 0x80 } else { base }
}

/// Recompute and cache the VGA attribute byte. Call after any attribute change.
unsafe fn update_cached_attr() {
    CACHED_ATTR = current_attr();
}

// ─── Low-level screen operations ────────────────────────────────────────

#[inline(always)]
unsafe fn write_char(col: usize, row: usize, ch: u8, attr: u8) {
    let offset = (row * COLS + col) * 2;
    ptr::write_volatile((VGA_BUFFER + offset) as *mut u8, ch);
    ptr::write_volatile((VGA_BUFFER + offset + 1) as *mut u8, attr);
}

/// Read character attribute at (col, row).
#[inline(always)]
unsafe fn read_attr(col: usize, row: usize) -> u8 {
    let offset = (row * COLS + col) * 2 + 1;
    ptr::read_volatile((VGA_BUFFER + offset) as *const u8)
}

unsafe fn scroll_up() {
    // Respect scroll region: only scroll lines within [SCROLL_TOP, SCROLL_BOTTOM]
    let top = SCROLL_TOP;
    let bottom = SCROLL_BOTTOM;
    if bottom <= top {
        return;
    }
    let region_height = bottom - top + 1;
    if region_height >= ROWS {
        // Full-screen scroll (original behavior)
        let src = VGA_BUFFER + COLS * 2;
        let dst = VGA_BUFFER;
        let len = COLS * (ROWS - 1) * 2;
        ptr::copy(src as *const u8, dst as *mut u8, len);

        let last_row = VGA_BUFFER + COLS * (ROWS - 1) * 2;
        let attr = current_attr();
        for col in 0..COLS {
            ptr::write_volatile((last_row + col * 2) as *mut u8, b' ');
            ptr::write_volatile((last_row + col * 2 + 1) as *mut u8, attr);
        }
    } else {
        // Region-only scroll: shift lines [top+1..=bottom] up by 1
        let src_row = top + 1;
        let dst_row = top;
        let rows_to_copy = bottom - top;
        let len = COLS * rows_to_copy * 2;
        let src = VGA_BUFFER + src_row * COLS * 2;
        let dst = VGA_BUFFER + dst_row * COLS * 2;
        ptr::copy(src as *const u8, dst as *mut u8, len);

        // Clear bottom line of the region
        let clear_row = VGA_BUFFER + bottom * COLS * 2;
        let attr = current_attr();
        for col in 0..COLS {
            ptr::write_volatile((clear_row + col * 2) as *mut u8, b' ');
            ptr::write_volatile((clear_row + col * 2 + 1) as *mut u8, attr);
        }
    }
}

// ─── ANSI CSI parameter parsing ─────────────────────────────────────────

/// Get CSI parameter at index, or default value if missing/zero.
unsafe fn csi_param(idx: usize, default: u16) -> u16 {
    if idx < CSI_PARAM_COUNT && CSI_PARAMS[idx] != 0 {
        CSI_PARAMS[idx]
    } else {
        default
    }
}

/// VGA 16-color palette as RGB values for nearest-color matching.
/// Index matches VGA color number (0-15).
const VGA_PALETTE_RGB: [(u8, u8, u8); 16] = [
    (0, 0, 0),       // 0: Black
    (170, 0, 0),     // 1: Red (dark)
    (0, 170, 0),     // 2: Green (dark)
    (170, 85, 0),    // 3: Yellow/Brown (dark)
    (0, 0, 170),     // 4: Blue (dark)
    (170, 0, 170),   // 5: Magenta (dark)
    (0, 170, 170),   // 6: Cyan (dark)
    (170, 170, 170), // 7: White/Light gray
    (85, 85, 85),    // 8: Bright black/Dark gray
    (255, 85, 85),   // 9: Bright red
    (85, 255, 85),   // 10: Bright green
    (255, 255, 85),  // 11: Bright yellow
    (85, 85, 255),   // 12: Bright blue
    (255, 85, 255),  // 13: Bright magenta
    (85, 255, 255),  // 14: Bright cyan
    (255, 255, 255), // 15: Bright white
];

/// Standard 256-color palette (same as xterm).
/// Indices 0-15 match the 16 VGA colors above.
/// Indices 16-231 are a 6x6x6 color cube.
/// Indices 232-255 are a grayscale ramp.
fn color256_to_rgb(idx: u8) -> (u8, u8, u8) {
    if idx < 16 {
        VGA_PALETTE_RGB[idx as usize]
    } else if idx < 232 {
        // 6x6x6 color cube: idx = 16 + 36*r + 6*g + b
        let i = (idx - 16) as usize;
        let r = i / 36;
        let g = (i / 6) % 6;
        let b = i % 6;
        // Convert 0-5 to RGB values
        let component = |v: usize| -> u8 {
            if v == 0 {
                0
            } else {
                (55 + v * 40) as u8 // 0, 95, 135, 175, 215, 255
            }
        };
        (component(r), component(g), component(b))
    } else {
        // Grayscale ramp: 232-255
        let v = 8 + (idx - 232) * 10; // 8, 18, ..., 248
        (v, v, v)
    }
}

/// Find the nearest VGA 16-color index for an RGB value.
fn nearest_vga_color(r: u8, g: u8, b: u8) -> u8 {
    let mut best = 0u8;
    let mut best_dist = u32::MAX;
    for (i, &(pr, pg, pb)) in VGA_PALETTE_RGB.iter().enumerate() {
        let dr = r as i32 - pr as i32;
        let dg = g as i32 - pg as i32;
        let db = b as i32 - pb as i32;
        let dist = (dr * dr + dg * dg + db * db) as u32;
        if dist < best_dist {
            best_dist = dist;
            best = i as u8;
        }
    }
    best
}

/// Map a 256-color index to the nearest VGA 16-color.
fn color256_to_vga(idx: u8) -> u8 {
    let (r, g, b) = color256_to_rgb(idx);
    nearest_vga_color(r, g, b)
}

// ─── ANSI CSI dispatch ──────────────────────────────────────────────────

/// Process a complete CSI sequence. `final_byte` is the last character (e.g., 'H', 'J', 'm').
unsafe fn csi_dispatch(final_byte: u8) {
    match final_byte {
        // ── Cursor positioning ──
        b'H' | b'f' => {
            // CUP — Cursor Position: \033[row;colH (1-based, default 1;1)
            let row = (csi_param(0, 1) as usize).saturating_sub(1).min(ROWS - 1);
            let col = (csi_param(1, 1) as usize).saturating_sub(1).min(COLS - 1);
            CURSOR_ROW = row;
            CURSOR_COL = col;
        }
        b'A' => {
            // CUU — Cursor Up
            let n = csi_param(0, 1) as usize;
            CURSOR_ROW = CURSOR_ROW.saturating_sub(n);
        }
        b'B' => {
            // CUD — Cursor Down
            let n = csi_param(0, 1) as usize;
            CURSOR_ROW = (CURSOR_ROW + n).min(ROWS - 1);
        }
        b'C' => {
            // CUF — Cursor Forward
            let n = csi_param(0, 1) as usize;
            CURSOR_COL = (CURSOR_COL + n).min(COLS - 1);
        }
        b'D' => {
            // CUB — Cursor Back
            let n = csi_param(0, 1) as usize;
            CURSOR_COL = CURSOR_COL.saturating_sub(n);
        }
        b'E' => {
            // CNL — Cursor Next Line
            let n = csi_param(0, 1) as usize;
            CURSOR_ROW = (CURSOR_ROW + n).min(ROWS - 1);
            CURSOR_COL = 0;
        }
        b'F' => {
            // CPL — Cursor Previous Line
            let n = csi_param(0, 1) as usize;
            CURSOR_ROW = CURSOR_ROW.saturating_sub(n);
            CURSOR_COL = 0;
        }
        b'G' => {
            // CHA — Cursor Horizontal Absolute
            let col = (csi_param(0, 1) as usize).saturating_sub(1).min(COLS - 1);
            CURSOR_COL = col;
        }
        b'd' => {
            // VPA — Vertical Position Absolute
            let row = (csi_param(0, 1) as usize).saturating_sub(1).min(ROWS - 1);
            CURSOR_ROW = row;
        }

        // ── Erase ──
        b'J' => {
            // ED — Erase in Display
            let mode = csi_param(0, 0);
            let attr = current_attr();
            match mode {
                0 => {
                    // Erase from cursor to end of screen
                    // Clear rest of current line
                    for col in CURSOR_COL..COLS {
                        write_char(col, CURSOR_ROW, b' ', attr);
                    }
                    // Clear all lines below
                    for row in (CURSOR_ROW + 1)..ROWS {
                        for col in 0..COLS {
                            write_char(col, row, b' ', attr);
                        }
                    }
                }
                1 => {
                    // Erase from start of screen to cursor
                    for row in 0..CURSOR_ROW {
                        for col in 0..COLS {
                            write_char(col, row, b' ', attr);
                        }
                    }
                    for col in 0..=CURSOR_COL {
                        write_char(col, CURSOR_ROW, b' ', attr);
                    }
                }
                2 => {
                    // Clear entire screen (don't move cursor)
                    for row in 0..ROWS {
                        for col in 0..COLS {
                            write_char(col, row, b' ', attr);
                        }
                    }
                }
                3 => {
                    // Clear scrollback (no-op for VGA, just clear screen)
                    for row in 0..ROWS {
                        for col in 0..COLS {
                            write_char(col, row, b' ', attr);
                        }
                    }
                }
                _ => {}
            }
        }
        b'K' => {
            // EL — Erase in Line
            let mode = csi_param(0, 0);
            let attr = current_attr();
            match mode {
                0 => {
                    // Erase from cursor to end of line
                    for col in CURSOR_COL..COLS {
                        write_char(col, CURSOR_ROW, b' ', attr);
                    }
                }
                1 => {
                    // Erase from start of line to cursor
                    for col in 0..=CURSOR_COL {
                        write_char(col, CURSOR_ROW, b' ', attr);
                    }
                }
                2 => {
                    // Erase entire line
                    for col in 0..COLS {
                        write_char(col, CURSOR_ROW, b' ', attr);
                    }
                }
                _ => {}
            }
        }
        b'X' => {
            // ECH — Erase Characters
            let n = csi_param(0, 1) as usize;
            let attr = current_attr();
            for i in 0..n {
                let col = CURSOR_COL + i;
                if col >= COLS {
                    break;
                }
                write_char(col, CURSOR_ROW, b' ', attr);
            }
        }

        // ── SGR (Select Graphic Rendition) — Colors & Attributes ──
        b'm' => {
            if CSI_PARAM_COUNT == 0 {
                // \033[m — reset all attributes
                CSI_PARAMS[0] = 0;
                CSI_PARAM_COUNT = 1;
            }
            let mut i = 0;
            while i < CSI_PARAM_COUNT {
                let p = CSI_PARAMS[i];
                match p {
                    0 => {
                        // Reset all
                        CURRENT_FG = 7;
                        CURRENT_BG = 0;
                        BOLD = false;
                        REVERSE = false;
                        BLINK = false;
                        FAINT = false;
                        CONCEALED = false;
                    }
                    1 => {
                        // Bold / increased intensity
                        BOLD = true;
                    }
                    2 => {
                        // Dim / faint
                        FAINT = true;
                    }
                    3 => {
                        // Italic — no VGA support, ignore
                    }
                    4 => {
                        // Underline — no VGA support, ignore
                    }
                    5 => {
                        // Blink (VGA supports this via bit 7)
                        BLINK = true;
                    }
                    7 => {
                        // Reverse video
                        REVERSE = true;
                    }
                    8 => {
                        // Concealed (invisible)
                        CONCEALED = true;
                    }
                    9 => {
                        // Strikethrough — no VGA support, ignore
                    }
                    22 => {
                        // Normal intensity (not bold, not faint)
                        BOLD = false;
                        FAINT = false;
                    }
                    23 => {
                        // Not italic — ignore
                    }
                    24 => {
                        // Not underlined — ignore
                    }
                    25 => {
                        // Not blinking
                        BLINK = false;
                    }
                    27 => {
                        // Not reversed
                        REVERSE = false;
                    }
                    28 => {
                        // Not concealed
                        CONCEALED = false;
                    }
                    29 => {
                        // Not crossed out — ignore
                    }
                    30..=37 => {
                        // Set foreground color (ANSI 0-7)
                        CURRENT_FG = (p - 30) as u8;
                    }
                    38 => {
                        // Extended foreground color
                        if i + 1 < CSI_PARAM_COUNT {
                            let mode = CSI_PARAMS[i + 1];
                            if mode == 5 && i + 2 < CSI_PARAM_COUNT {
                                // 256-color: \033[38;5;Nm
                                CURRENT_FG = color256_to_vga(CSI_PARAMS[i + 2] as u8);
                                i += 2;
                            } else if mode == 2 && i + 4 < CSI_PARAM_COUNT {
                                // TrueColor: \033[38;2;R;G;Bm
                                let r = CSI_PARAMS[i + 2] as u8;
                                let g = CSI_PARAMS[i + 3] as u8;
                                let b = CSI_PARAMS[i + 4] as u8;
                                CURRENT_FG = nearest_vga_color(r, g, b);
                                i += 4;
                            }
                        }
                    }
                    39 => {
                        // Default foreground
                        CURRENT_FG = 7;
                    }
                    40..=47 => {
                        // Set background color (ANSI 0-7)
                        CURRENT_BG = (p - 40) as u8;
                    }
                    48 => {
                        // Extended background color
                        if i + 1 < CSI_PARAM_COUNT {
                            let mode = CSI_PARAMS[i + 1];
                            if mode == 5 && i + 2 < CSI_PARAM_COUNT {
                                // 256-color: \033[48;5;Nm
                                CURRENT_BG = color256_to_vga(CSI_PARAMS[i + 2] as u8);
                                i += 2;
                            } else if mode == 2 && i + 4 < CSI_PARAM_COUNT {
                                // TrueColor: \033[48;2;R;G;Bm
                                let r = CSI_PARAMS[i + 2] as u8;
                                let g = CSI_PARAMS[i + 3] as u8;
                                let b = CSI_PARAMS[i + 4] as u8;
                                CURRENT_BG = nearest_vga_color(r, g, b);
                                i += 4;
                            }
                        }
                    }
                    49 => {
                        // Default background
                        CURRENT_BG = 0;
                    }
                    90..=97 => {
                        // Bright foreground (ANSI 8-15)
                        CURRENT_FG = (p - 90 + 8) as u8;
                    }
                    100..=107 => {
                        // Bright background (ANSI 8-15)
                        CURRENT_BG = (p - 100 + 8) as u8;
                    }
                    _ => {
                        // Ignore unknown SGR parameters
                    }
                }
                i += 1;
            }
            // Update cached attribute after SGR changes
            update_cached_attr();
        }
        b'h' => {
            if CSI_HAS_QUESTION {
                let mode = csi_param(0, 0);
                match mode {
                    25 => {
                        CURSOR_VISIBLE = true;
                    }
                    7 => { /* Auto-wrap ON — VGA always wraps, no-op */ }
                    47 | 1047 | 1049 => {
                        // Switch to alternate screen buffer
                        if !ALT_SCREEN_SAVED {
                            // Save current screen
                            unsafe {
                                let buf_ptr = core::ptr::addr_of_mut!(ALT_SCREEN_BUF) as *mut u8;
                                ptr::copy(VGA_BUFFER as *const u8, buf_ptr, COLS * ROWS * 2);
                            }
                            ALT_SCREEN_SAVED = true;
                            if mode == 1049 {
                                // Also save cursor position
                                SAVED_COL = CURSOR_COL;
                                SAVED_ROW = CURSOR_ROW;
                            }
                            // Clear screen for alternate buffer
                            let attr = current_attr();
                            for row in 0..ROWS {
                                for col in 0..COLS {
                                    write_char(col, row, b' ', attr);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        b'l' => {
            if CSI_HAS_QUESTION {
                let mode = csi_param(0, 0);
                match mode {
                    25 => {
                        CURSOR_VISIBLE = false;
                    }
                    7 => { /* Auto-wrap OFF — not fully supported */ }
                    47 | 1047 | 1049 => {
                        // Switch back to main screen buffer
                        if ALT_SCREEN_SAVED {
                            unsafe {
                                let buf_ptr = core::ptr::addr_of!(ALT_SCREEN_BUF) as *const u8;
                                ptr::copy(buf_ptr, VGA_BUFFER as *mut u8, COLS * ROWS * 2);
                            }
                            ALT_SCREEN_SAVED = false;
                            if mode == 1049 {
                                CURSOR_COL = SAVED_COL;
                                CURSOR_ROW = SAVED_ROW;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // ── Save/Restore cursor ──
        b's' => {
            SAVED_COL = CURSOR_COL;
            SAVED_ROW = CURSOR_ROW;
        }
        b'u' => {
            CURSOR_COL = SAVED_COL;
            CURSOR_ROW = SAVED_ROW;
        }

        // ── Device Status Report ──
        b'n' => {
            let mode = csi_param(0, 0);
            if mode == 6 {
                // Report cursor position: \033[row;colR (1-based)
                let row = (CURSOR_ROW + 1) as u8;
                let col = (CURSOR_COL + 1) as u8;
                // Send response via UART/serial
                let resp = [
                    b'\x1b',
                    b'[',
                    b'0' + row / 10,
                    b'0' + row % 10,
                    b';',
                    b'0' + col / 10,
                    b'0' + col % 10,
                    b'R',
                ];
                for &b in &resp {
                    crate::arch::uart::putchar(b);
                }
                // Also inject into TTY input for programs reading stdin
                // The DSR response should go to the application's stdin
            }
            // mode == 5: "I'm OK" → respond \033[0n
            if mode == 5 {
                crate::arch::uart::putchar(b'\x1b');
                crate::arch::uart::putchar(b'[');
                crate::arch::uart::putchar(b'0');
                crate::arch::uart::putchar(b'n');
            }
        }

        // ── Scroll regions ──
        b'r' => {
            // DECSTBM — Set Scrolling Region: \033[top;bottomr (1-based)
            let top = (csi_param(0, 1) as usize).saturating_sub(1).min(ROWS - 1);
            let bottom = if CSI_PARAM_COUNT > 1 && CSI_PARAMS[1] != 0 {
                (CSI_PARAMS[1] as usize).min(ROWS)
            } else {
                ROWS
            };
            let bottom = bottom.saturating_sub(1).max(top);
            if bottom > top {
                SCROLL_TOP = top;
                SCROLL_BOTTOM = bottom;
                // Move cursor to home position within scroll region
                CURSOR_ROW = top;
                CURSOR_COL = 0;
            }
        }

        // ── Insert/Delete lines ──
        b'L' => {
            // IL — Insert Lines
            let n = csi_param(0, 1) as usize;
            let attr = current_attr();
            // Shift rows down from CURSOR_ROW
            for _ in 0..n {
                // Shift all rows below current row down by 1
                for row in (CURSOR_ROW + 1..ROWS).rev() {
                    for col in 0..COLS {
                        let ch_offset = ((row - 1) * COLS + col) * 2;
                        let ch = ptr::read_volatile((VGA_BUFFER + ch_offset) as *const u8);
                        let at = ptr::read_volatile((VGA_BUFFER + ch_offset + 1) as *const u8);
                        let dst_offset = (row * COLS + col) * 2;
                        ptr::write_volatile((VGA_BUFFER + dst_offset) as *mut u8, ch);
                        ptr::write_volatile((VGA_BUFFER + dst_offset + 1) as *mut u8, at);
                    }
                }
                // Clear current row
                for col in 0..COLS {
                    write_char(col, CURSOR_ROW, b' ', attr);
                }
            }
        }
        b'M' => {
            // DL — Delete Lines
            let n = csi_param(0, 1) as usize;
            let attr = current_attr();
            for _ in 0..n {
                // Shift rows up from CURSOR_ROW
                for row in CURSOR_ROW..(ROWS - 1) {
                    for col in 0..COLS {
                        let src_offset = ((row + 1) * COLS + col) * 2;
                        let ch = ptr::read_volatile((VGA_BUFFER + src_offset) as *const u8);
                        let at = ptr::read_volatile((VGA_BUFFER + src_offset + 1) as *const u8);
                        let dst_offset = (row * COLS + col) * 2;
                        ptr::write_volatile((VGA_BUFFER + dst_offset) as *mut u8, ch);
                        ptr::write_volatile((VGA_BUFFER + dst_offset + 1) as *mut u8, at);
                    }
                }
                // Clear last row
                for col in 0..COLS {
                    write_char(col, ROWS - 1, b' ', attr);
                }
            }
        }

        _ => {
            // Unknown CSI final byte — ignore
        }
    }

    update_hardware_cursor();
}

// ─── Public API ─────────────────────────────────────────────────────────

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

/// Write a single byte to the VGA console.
///
/// Handles:
/// - Control characters (\n, \r, \t, \x08, \x07)
/// - ANSI escape sequences (\033[...X)
/// - Printable ASCII → display character with current attributes
pub(crate) fn putchar(c: u8) {
    if !VGA_INITIALIZED.load(Ordering::Relaxed) {
        return;
    }

    unsafe {
        match PARSE_STATE {
            ParseState::Normal => {
                match c {
                    0x1B => {
                        // ESC — start escape sequence
                        PARSE_STATE = ParseState::Escape;
                    }
                    b'\n' => {
                        CURSOR_COL = 0;
                        CURSOR_ROW += 1;
                        if CURSOR_ROW >= ROWS {
                            scroll_up();
                            CURSOR_ROW = ROWS - 1;
                        }
                        update_hardware_cursor();
                    }
                    b'\r' => {
                        CURSOR_COL = 0;
                        update_hardware_cursor();
                    }
                    b'\t' => {
                        let next_tab = (CURSOR_COL + 8) & !7;
                        let spaces = if next_tab < COLS {
                            next_tab - CURSOR_COL
                        } else {
                            COLS - CURSOR_COL
                        };
                        let attr = current_attr();
                        for _ in 0..spaces {
                            write_char(CURSOR_COL, CURSOR_ROW, b' ', attr);
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
                        if CURSOR_COL > 0 {
                            CURSOR_COL -= 1;
                            // Don't erase — TUI programs use \b + write to update
                        }
                        update_hardware_cursor();
                    }
                    b'\x07' => {
                        // BEL — beep (no-op for now, could use PC speaker)
                        update_hardware_cursor();
                    }
                    _ => {
                        if c >= 0x20 {
                            // Fast path: printable character — use cached attribute
                            // (no current_attr() call per character — major perf win)
                            write_char(CURSOR_COL, CURSOR_ROW, c, CACHED_ATTR);
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
                // Note: update_hardware_cursor() removed from here for performance.
                // It's now called only for control characters and escape sequences.
            }
            ParseState::Escape => {
                match c {
                    b'[' => {
                        // CSI — Control Sequence Introducer
                        PARSE_STATE = ParseState::Csi;
                        CSI_PARAM_COUNT = 0;
                        CSI_HAS_QUESTION = false;
                        CSI_PARAMS = [0; CSI_MAX_PARAMS];
                    }
                    b'c' => {
                        // RIS — Reset
                        clear_screen();
                        CURRENT_FG = 7;
                        CURRENT_BG = 0;
                        BOLD = false;
                        REVERSE = false;
                        BLINK = false;
                        FAINT = false;
                        CONCEALED = false;
                        SCROLL_TOP = 0;
                        SCROLL_BOTTOM = ROWS - 1;
                        CURSOR_VISIBLE = true;
                        PARSE_STATE = ParseState::Normal;
                        update_cached_attr();
                    }
                    b'7' => {
                        // DECSC — Save cursor + attributes
                        SAVED_COL = CURSOR_COL;
                        SAVED_ROW = CURSOR_ROW;
                        PARSE_STATE = ParseState::Normal;
                    }
                    b'8' => {
                        // DECRC — Restore cursor + attributes
                        CURSOR_COL = SAVED_COL;
                        CURSOR_ROW = SAVED_ROW;
                        update_hardware_cursor();
                        PARSE_STATE = ParseState::Normal;
                    }
                    b'D' => {
                        // IND — Index (move down, scroll if at bottom)
                        CURSOR_ROW += 1;
                        if CURSOR_ROW >= ROWS {
                            scroll_up();
                            CURSOR_ROW = ROWS - 1;
                        }
                        update_hardware_cursor();
                        PARSE_STATE = ParseState::Normal;
                    }
                    b'M' => {
                        // RI — Reverse Index (move up, scroll if at top)
                        if CURSOR_ROW == 0 {
                            // Scroll down
                            let attr = current_attr();
                            let last_row = VGA_BUFFER + COLS * (ROWS - 1) * 2;
                            for row in (1..ROWS).rev() {
                                let src = VGA_BUFFER + (row - 1) * COLS * 2;
                                let dst = VGA_BUFFER + row * COLS * 2;
                                ptr::copy(src as *const u8, dst as *mut u8, COLS * 2);
                            }
                            for col in 0..COLS {
                                write_char(col, 0, b' ', attr);
                            }
                        } else {
                            CURSOR_ROW -= 1;
                        }
                        update_hardware_cursor();
                        PARSE_STATE = ParseState::Normal;
                    }
                    b'(' => {
                        // Designate G0 character set — skip next byte
                        // (just consume the next character and ignore)
                        // We'll handle this by staying in Escape state
                        // Actually, consume one more byte then return to normal
                        // Simplified: just go back to normal
                        PARSE_STATE = ParseState::Normal;
                    }
                    b')' => {
                        // Designate G1 — same as above
                        PARSE_STATE = ParseState::Normal;
                    }
                    b']' => {
                        // OSC — Operating System Command
                        // Skip until BEL (\x07) or ST (\033\\)
                        PARSE_STATE = ParseState::Osc;
                    }
                    b'_' => {
                        // APC — Application Program Command (skip until BEL/ST)
                        PARSE_STATE = ParseState::Osc;
                    }
                    b'P' => {
                        // DCS — Device Control String (skip until BEL/ST)
                        PARSE_STATE = ParseState::Osc;
                    }
                    _ => {
                        // Unknown escape sequence — return to normal
                        PARSE_STATE = ParseState::Normal;
                    }
                }
            }
            ParseState::Csi => {
                match c {
                    b'0'..=b'9' => {
                        // Digit — accumulate into current parameter
                        if CSI_PARAM_COUNT < CSI_MAX_PARAMS {
                            CSI_PARAMS[CSI_PARAM_COUNT] =
                                CSI_PARAMS[CSI_PARAM_COUNT] * 10 + (c - b'0') as u16;
                        }
                    }
                    b';' => {
                        // Parameter separator
                        if CSI_PARAM_COUNT < CSI_MAX_PARAMS - 1 {
                            CSI_PARAM_COUNT += 1;
                        }
                    }
                    b'?' => {
                        // Private mode marker
                        CSI_HAS_QUESTION = true;
                    }
                    0x20..=0x2F => {
                        // Intermediate bytes (space, !, ", etc.) — ignore
                    }
                    0x40..=0x7E => {
                        // Final byte — dispatch CSI sequence
                        if CSI_PARAM_COUNT == 0 && CSI_PARAMS[0] == 0 {
                            // No params were collected, param count is 0
                        } else {
                            CSI_PARAM_COUNT += 1;
                        }
                        csi_dispatch(c);
                        PARSE_STATE = ParseState::Normal;
                    }
                    _ => {
                        // Invalid byte in CSI — abort
                        PARSE_STATE = ParseState::Normal;
                    }
                }
            }
            ParseState::Osc => {
                // OSC/DCS/APC string: skip everything until BEL (\x07) or ST (\033\\)
                match c {
                    0x07 => {
                        // BEL — end of OSC string
                        PARSE_STATE = ParseState::Normal;
                    }
                    0x1B => {
                        // ESC — might be start of ST (\033\\)
                        // Just go to Escape state; if next byte is \, it will be
                        // consumed as unknown escape and return to Normal
                        PARSE_STATE = ParseState::Escape;
                    }
                    _ => {
                        // Skip all other bytes in OSC string
                    }
                }
            }
        }
    }
}

/// Initialize the VGA text mode driver.
pub fn init() {
    unsafe {
        outb(CRT_CTRL, 0x0A);
        outb(CRT_DATA, 0x00);
        outb(CRT_CTRL, 0x0B);
        outb(CRT_DATA, 0x0F);
    }
    clear_screen();
    VGA_INITIALIZED.store(true, Ordering::Relaxed);
    crate::console_println!("[vga] Text mode 80x25 initialized (ANSI escape support)");
}

/// Get the current screen dimensions (cols, rows).
pub fn screen_size() -> (usize, usize) {
    (COLS, ROWS)
}

/// Get current cursor position (col, row) — 0-based.
pub fn cursor_pos() -> (usize, usize) {
    unsafe { (CURSOR_COL, CURSOR_ROW) }
}

/// Flush hardware cursor to match the internal cursor position.
/// Call after batch writes to ensure the hardware cursor is visible.
pub fn flush_cursor() {
    unsafe { update_hardware_cursor() }
}
