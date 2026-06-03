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

// ─── ANSI Escape Sequence Parser State ──────────────────────────────────

/// CSI parameter buffer max
const CSI_MAX_PARAMS: usize = 16;

#[derive(Clone, Copy, PartialEq)]
enum ParseState {
    Normal,
    Escape, // Received ESC (\x1b)
    Csi,    // Received ESC [ — collecting parameters
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

/// Saved cursor position
static mut SAVED_COL: usize = 0;
static mut SAVED_ROW: usize = 0;

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

    if REVERSE {
        // Swap foreground and background (limited to 7 bits each)
        (bg & 0x0F) << 4 | (fg & 0x07)
    } else {
        (bg & 0x0F) << 4 | (fg & 0x0F)
    }
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
            for i in 0..CSI_PARAM_COUNT {
                let p = CSI_PARAMS[i];
                match p {
                    0 => {
                        // Reset
                        CURRENT_FG = 7;
                        CURRENT_BG = 0;
                        BOLD = false;
                        REVERSE = false;
                    }
                    1 => {
                        // Bold (bright)
                        BOLD = true;
                    }
                    7 => {
                        // Reverse video
                        REVERSE = true;
                    }
                    22 => {
                        // Normal intensity (not bold)
                        BOLD = false;
                    }
                    27 => {
                        // Not reverse
                        REVERSE = false;
                    }
                    30..=37 => {
                        // Set foreground color (ANSI 0-7)
                        CURRENT_FG = (p - 30) as u8;
                    }
                    38 => {
                        // Extended foreground — skip next param if 5 (256-color)
                        // For now, just ignore
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
                        // Extended background — skip
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
            }
        }

        // ── Cursor show/hide ──
        b'h' => {
            if CSI_HAS_QUESTION {
                let mode = csi_param(0, 0);
                if mode == 25 {
                    // Show cursor
                    CURSOR_VISIBLE = true;
                }
            }
        }
        b'l' => {
            if CSI_HAS_QUESTION {
                let mode = csi_param(0, 0);
                if mode == 25 {
                    // Hide cursor
                    CURSOR_VISIBLE = false;
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

        // ── Scroll regions (basic support) ──
        b'r' => {
            // DECSTBM — Set Scrolling Region: \033[top;bottomr
            // For now, ignore (full implementation needs scroll region tracking)
            // TUI programs that use this (vim, less) will work in limited mode
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
pub fn putchar(c: u8) {
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
                    }
                    b'\r' => {
                        CURSOR_COL = 0;
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
                    }
                    b'\x07' => {
                        // BEL — beep (no-op for now, could use PC speaker)
                    }
                    _ => {
                        if c >= 0x20 {
                            let attr = current_attr();
                            write_char(CURSOR_COL, CURSOR_ROW, c, attr);
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
                        CURSOR_VISIBLE = true;
                        PARSE_STATE = ParseState::Normal;
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
