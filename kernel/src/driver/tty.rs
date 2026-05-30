//! TTY subsystem — interrupt-driven terminal input with lock-free ring buffer.
//!
//! Architecture:
//!   UART RX ──► timer/PLIC interrupt ──► feed_byte() ──► ring buffer
//!   sys_read(fd=0) ──► read() ──► buffer empty? ──► block task
//!                                               └─► woken by feed_byte()
//!
//! Safety: producer (feed_byte) runs in interrupt handler, consumer (read)
//! runs in syscall dispatch. Both execute in trap-handler context with SIE=0,
//! so they never overlap. Atomics are used for the ring buffer indices anyway
//! to enforce memory ordering and for future SMP safety.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Ring buffer capacity (power of 2 for fast modulo)
const TTY_BUF_SIZE: usize = 4096;
const TTY_BUF_MASK: usize = TTY_BUF_SIZE - 1;

/// Line editing buffer capacity
const TTY_LINE_SIZE: usize = 256;

/// Signal flag bits (stored in Process::pending_signals)
pub const SIGINT: u64 = 1 << 2;

// ─── Lock-free SPSC ring buffer ──────────────────────────────────────

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
    fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Relaxed) == self.head.load(Ordering::Acquire)
    }

    /// Number of bytes in the buffer
    #[allow(dead_code)]
    fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);
        (head.wrapping_sub(tail)) & TTY_BUF_MASK
    }
}

// ─── Line editor state ───────────────────────────────────────────────

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

// ─── Global state ────────────────────────────────────────────────────

/// TTY input ring buffer (completed lines ready for sys_read)
static TTY_INPUT: RingBuffer = RingBuffer::new();

/// Line editor (current line being composed)
static TTY_LINE: LineEditorData = LineEditorData {
    inner: UnsafeCell::new(LineEditor::new()),
};

struct LineEditorData {
    inner: UnsafeCell<LineEditor>,
}
unsafe impl Sync for LineEditorData {}

/// Process index blocked on stdin read (usize::MAX = no waiter)
static TTY_WAITING: AtomicUsize = AtomicUsize::new(usize::MAX);

// ─── Public API ──────────────────────────────────────────────────────

/// Initialize TTY subsystem. Called during kernel init after PLIC init.
pub fn init() {
    // Enable supervisor external interrupts (PLIC → UART IRQ 10)
    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sie::set_sext();
    }
}

/// Poll UART RX FIFO and feed all available bytes into the TTY line editor.
/// Called from timer interrupt handler.
pub fn poll_uart() {
    #[cfg(target_arch = "riscv64")]
    let uart = crate::driver::uart::Uart::new(0x1000_0000);
    #[cfg(target_arch = "x86_64")]
    {
        while let Some(c) = crate::arch::uart::getchar() {
            on_char(c);
        }
        return;
    }
    #[cfg(target_arch = "riscv64")]
    while let Some(c) = uart.getc() {
        on_char(c);
    }
}

/// Feed a single character into the TTY line editor.
///
/// Public interface for keyboard and other input drivers to inject characters.
/// Same processing as UART input (line editing, echo, ring buffer).
pub fn feed_byte(c: u8) {
    on_char(c);
}

/// Process a single character from UART.
/// Handles canonical-mode line editing: echo, backspace, Ctrl+C, Enter.
/// Completed lines are pushed into the ring buffer.
fn on_char(c: u8) {
    let line = unsafe { &mut *TTY_LINE.inner.get() };

    match c {
        // Ctrl+C — send SIGINT, discard current line, push empty line
        0x03 => {
            echo(b"^C\r\n");
            line.clear();
            // Push a bare newline so sys_read returns immediately with an
            // empty line.  The shell will trim it, see an empty command,
            // and re-print the prompt.
            TTY_INPUT.push(b'\n');
            // Wake blocked reader (same as Enter handling)
            let waiting = TTY_WAITING.swap(usize::MAX, Ordering::AcqRel);
            if waiting != usize::MAX {
                crate::sched::wake_task(waiting);
            }
        }
        // Backspace (BS) or Delete (DEL)
        0x08 | 0x7F => {
            if line.pop() {
                // Erase character on terminal: BS + space + BS
                echo(&[0x08, b' ', 0x08]);
            }
        }
        // Enter — CR or LF: commit the line to the ring buffer
        b'\r' | b'\n' => {
            echo(b"\r\n");
            // Push the entire line into the ring buffer
            let len = line.len;
            for i in 0..len {
                let byte = line.buf[i];
                if !TTY_INPUT.push(byte) {
                    break; // Ring buffer full
                }
            }
            // Append newline
            TTY_INPUT.push(b'\n');
            line.clear();

            // Wake blocked reader
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
        // Ignore other control characters
        _ => {}
    }
}

/// Read from TTY input (sys_read for fd=0).
///
/// In canonical mode, returns complete lines (ending with \n).
/// If no data is available, returns 0 (non-blocking).
/// The caller (shell) should retry after a brief yield.
///
/// Data arrives via timer interrupt polling UART RX (every 10ms),
/// which feeds characters through the TTY line editor.
pub fn read(buf: usize, len: usize) -> isize {
    if len == 0 || buf == 0 {
        return 0;
    }

    read_available(buf, len)
}

// ─── Internal helpers ────────────────────────────────────────────────

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

/// Echo bytes directly to UART (raw MMIO, no SpinLock, safe from interrupt context).
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
            // Also echo to VGA for local display
            crate::driver::vga::putchar(b);
        }
    }
}
