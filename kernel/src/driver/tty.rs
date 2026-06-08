//! TTY subsystem — interrupt-driven terminal input with lock-free ring buffer.
//!
//! Supports two input modes:
//! - **Canonical mode** (cooked): Line editing with echo, backspace, Ctrl+C.
//!   Completed lines delivered to sys_read. Default mode.
//! - **Raw mode**: No echo, no line editing. Every keystroke delivered immediately.
//!   Required for TUI applications (vim, htop, etc.)
//!
//! Mode is controlled via `set_raw_mode()` / `set_canonical_mode()` from sys_ioctl.
//!
//! Architecture:
//!   UART/keyboard ──► timer/IRQ handler ──► feed_byte() ──► ring buffer
//!   sys_read(fd=0) ──► read() ──► buffer empty? ──► block task
//!                                               └─► woken by feed_byte()

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Ring buffer capacity (power of 2 for fast modulo)
const TTY_BUF_SIZE: usize = 4096;
const TTY_BUF_MASK: usize = TTY_BUF_SIZE - 1;

/// Line editing buffer capacity
const TTY_LINE_SIZE: usize = 512;

/// Signal flag bits (stored in Process::pending_signals)
pub const SIGINT: u64 = 1 << 2;

// ─── Lock-free SPSC ring buffer ──────────────────────────────────────────

struct RingBuffer {
    data: UnsafeCell<[u8; TTY_BUF_SIZE]>,
    /// Write position — ONLY written by producer (interrupt handler)
    head: AtomicUsize,
    /// Read position — ONLY written by consumer (sys_read)
    tail: AtomicUsize,
}

// SAFETY: head is only written by the interrupt handler (feed_byte),
// tail is only written by sys_read (read). They never execute concurrently
// because trap handler runs with SIE=0. UnsafeCell interior mutability is
// protected by the atomic index ordering.
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    const fn new() -> Self {
        Self {
            data: UnsafeCell::new([0u8; TTY_BUF_SIZE]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn push(&self, byte: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let next_head = (head + 1) & TTY_BUF_MASK;
        if next_head == tail {
            return false;
        }
        unsafe {
            (*self.data.get())[head] = byte;
        }
        self.head.store(next_head, Ordering::Release);
        true
    }

    #[inline]
    fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let byte = unsafe { (*self.data.get())[tail] };
        self.tail
            .store((tail + 1) & TTY_BUF_MASK, Ordering::Release);
        Some(byte)
    }

    #[inline]
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Relaxed) == self.head.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);
        (head.wrapping_sub(tail)) & TTY_BUF_MASK
    }
}

// ─── Line editor state ──────────────────────────────────────────────────

struct LineEditor {
    /// Current line being composed (not yet terminated by Enter)
    buf: [u8; TTY_LINE_SIZE],
    /// Number of characters in the current line
    len: usize,
}

impl LineEditor {
    const fn new() -> Self {
        Self {
            buf: [0; TTY_LINE_SIZE],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, c: u8) -> bool {
        if self.len >= TTY_LINE_SIZE - 1 {
            return false;
        }
        self.buf[self.len] = c;
        self.len += 1;
        true
    }

    fn pop(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        self.len -= 1;
        true
    }
}

// ─── TTY mode ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum TtyMode {
    /// Canonical (cooked) mode: line editing, echo, signals
    Canonical,
    /// Raw mode: no echo, no line editing, immediate delivery
    Raw,
}

// ─── Global state ───────────────────────────────────────────────────────

/// TTY input ring buffer (completed lines / raw bytes ready for sys_read)
static TTY_INPUT: RingBuffer = RingBuffer::new();

/// Check if TTY has input data available (for epoll EPOLLIN on stdin)
pub fn has_input() -> bool {
    TTY_INPUT.len() > 0
}

/// Line editor (only used in canonical mode)
static TTY_LINE: LineEditorData = LineEditorData {
    inner: UnsafeCell::new(LineEditor::new()),
};

struct LineEditorData {
    inner: UnsafeCell<LineEditor>,
}
unsafe impl Sync for LineEditorData {}

/// Current TTY mode
static TTY_MODE: AtomicBool = AtomicBool::new(false); // false = Canonical, true = Raw

/// Echo enabled flag (can be toggled independently)
static TTY_ECHO: AtomicBool = AtomicBool::new(true);

/// Process index blocked on stdin read (usize::MAX = no waiter)
static TTY_WAITING: AtomicUsize = AtomicUsize::new(usize::MAX);

// ─── Public API ─────────────────────────────────────────────────────────

/// Initialize TTY subsystem.
pub fn init() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sie::set_sext();
    }
}

/// Set TTY input mode.
pub fn set_mode(mode: TtyMode) {
    match mode {
        TtyMode::Canonical => {
            TTY_MODE.store(false, Ordering::Relaxed);
            TTY_ECHO.store(true, Ordering::Relaxed);
        }
        TtyMode::Raw => {
            TTY_MODE.store(true, Ordering::Relaxed);
            TTY_ECHO.store(false, Ordering::Relaxed);
            // Flush any pending line editor content
            let line = unsafe { &mut *TTY_LINE.inner.get() };
            line.clear();
        }
    }
}

/// Get current TTY mode.
pub fn get_mode() -> TtyMode {
    if TTY_MODE.load(Ordering::Relaxed) {
        TtyMode::Raw
    } else {
        TtyMode::Canonical
    }
}

/// Set echo on/off independently.
pub fn set_echo(enabled: bool) {
    TTY_ECHO.store(enabled, Ordering::Relaxed);
}

/// Poll UART RX FIFO and feed all available bytes into the TTY.
/// Called from timer interrupt handler.
pub fn poll_uart() {
    #[cfg(target_arch = "x86_64")]
    {
        while let Some(c) = crate::arch::uart::getchar() {
            on_char(c);
        }
        return;
    }
    #[cfg(target_arch = "riscv64")]
    {
        let uart = crate::driver::uart::Uart::new(0x1000_0000);
        while let Some(c) = uart.getc() {
            on_char(c);
        }
    }
}

/// Feed a single character into the TTY input.
/// Public interface for keyboard and other input drivers.
pub fn feed_byte(c: u8) {
    on_char(c);
}

/// Process a single character from input.
fn on_char(c: u8) {
    let is_raw = TTY_MODE.load(Ordering::Relaxed);

    if is_raw {
        // ── Raw mode: pass through immediately, no echo ──
        on_char_raw(c);
    } else {
        // ── Canonical mode: line editing + echo ──
        on_char_canonical(c);
    }
}

/// Raw mode character handler.
/// Delivers every byte immediately to the ring buffer.
fn on_char_raw(c: u8) {
    // In raw mode, deliver the byte immediately
    if TTY_INPUT.push(c) {
        // Wake blocked reader
        let waiting = TTY_WAITING.swap(usize::MAX, Ordering::AcqRel);
        if waiting != usize::MAX {
            crate::sched::wake_task(waiting);
        }
    }
    // No echo in raw mode
}

/// Canonical mode character handler.
/// Handles line editing: echo, backspace, Ctrl+C, Enter.
fn on_char_canonical(c: u8) {
    let line = unsafe { &mut *TTY_LINE.inner.get() };

    match c {
        // Ctrl+C — send SIGINT, discard current line
        0x03 => {
            echo(b"^C\r\n");
            line.clear();
            TTY_INPUT.push(b'\n');
            let waiting = TTY_WAITING.swap(usize::MAX, Ordering::AcqRel);
            if waiting != usize::MAX {
                crate::sched::wake_task(waiting);
            }
        }
        // Backspace (BS) or Delete (DEL)
        0x08 | 0x7F => {
            if line.pop() {
                echo(&[0x08, b' ', 0x08]);
            }
        }
        // Enter — CR or LF: commit the line
        b'\r' | b'\n' => {
            echo(b"\r\n");
            let len = line.len;
            for i in 0..len {
                if !TTY_INPUT.push(line.buf[i]) {
                    break;
                }
            }
            TTY_INPUT.push(b'\n');
            line.clear();

            let waiting = TTY_WAITING.swap(usize::MAX, Ordering::AcqRel);
            if waiting != usize::MAX {
                crate::sched::wake_task(waiting);
            }
        }
        // Regular printable ASCII
        0x20..=0x7E => {
            if line.push(c) {
                echo(&[c]);
            }
        }
        // Ctrl+D — EOF on empty line
        0x04 => {
            if line.len == 0 {
                // Push nothing — sys_read returns 0 (EOF)
                let waiting = TTY_WAITING.swap(usize::MAX, Ordering::AcqRel);
                if waiting != usize::MAX {
                    crate::sched::wake_task(waiting);
                }
            }
        }
        // Ctrl+L — clear screen (convenience)
        0x0C => {
            echo(b"\x1b[2J\x1b[H");
        }
        // Ctrl+U — kill line
        0x15 => {
            // Erase the entire line visually
            for _ in 0..line.len {
                echo(&[0x08, b' ', 0x08]);
            }
            line.clear();
        }
        // Ctrl+W — delete word
        0x17 => {
            // Trim trailing spaces, then delete word
            while line.len > 0 && line.buf[line.len - 1] == b' ' {
                line.pop();
                echo(&[0x08, b' ', 0x08]);
            }
            while line.len > 0 && line.buf[line.len - 1] != b' ' {
                line.pop();
                echo(&[0x08, b' ', 0x08]);
            }
        }
        // Ignore other control characters
        _ => {}
    }
}

/// Read from TTY input (sys_read for fd=0).
///
/// In canonical mode: returns complete lines (ending with \n).
/// In raw mode: returns whatever bytes are available immediately.
/// Returns 0 if no data is available (non-blocking).
pub fn read(buf: usize, len: usize) -> isize {
    if len == 0 || buf == 0 {
        return 0;
    }
    read_available(buf, len)
}

// ─── Internal helpers ────────────────────────────────────────────────────

/// Copy available bytes from ring buffer to user buffer.
fn read_available(buf: usize, len: usize) -> isize {
    let mut count: isize = 0;
    for i in 0..len {
        match TTY_INPUT.pop() {
            Some(c) => {
                unsafe { core::ptr::write_volatile((buf + i) as *mut u8, c) };
                count += 1;
            }
            None => break,
        }
    }
    count
}

/// Echo bytes to UART + VGA output.
fn echo(bytes: &[u8]) {
    #[cfg(target_arch = "riscv64")]
    {
        let uart = crate::driver::uart::Uart::new(0x1000_0000);
        for &b in bytes {
            uart.putc(b);
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        for &b in bytes {
            crate::arch::uart::putchar(b);
            crate::driver::vga::putchar(b);
        }
    }
}
