//! COM1 UART (16550-compatible) driver using x86_64 port I/O.
//!
//! On x86_64 QEMU, the serial port is at I/O port 0x3F8 (COM1).
//! Unlike RISC-V which uses memory-mapped I/O at 0x1000_0000,
//! x86_64 uses port-mapped I/O via the `in`/`out` instructions.
//!
//! The `x86_64` crate provides `Port<u8>` for direct port I/O.

use x86_64::instructions::port::Port;

/// I/O port addresses for COM1 (0x3F8).
const COM1_BASE: u16 = 0x3F8;

/// COM1 UART driver using x86_64 port I/O.
pub struct ComPort {
    base: u16,
}

impl ComPort {
    /// Create a new COM port driver for the given I/O base address.
    pub const fn new(base: u16) -> Self {
        Self { base }
    }

    /// Initialize the UART: 115200 baud, 8N1, FIFO enabled, interrupts enabled.
    pub fn init(&mut self) {
        unsafe {
            let mut data = Port::<u8>::new(self.base);
            let mut int_enable = Port::<u8>::new(self.base + 1);
            let mut fifo_ctrl = Port::<u8>::new(self.base + 2);
            let mut line_ctrl = Port::<u8>::new(self.base + 3);
            let mut modem_ctrl = Port::<u8>::new(self.base + 4);
            let mut line_status = Port::<u8>::new(self.base + 5);

            // Disable all interrupts first
            int_enable.write(0x00);

            // Enable DLAB (set baud rate divisor)
            line_ctrl.write(0x80);
            // Set divisor to 1 (115200 baud)
            data.write(0x01);
            int_enable.write(0x00); // DLH

            // 8 bits, no parity, one stop bit (8N1), disable DLAB
            line_ctrl.write(0x03);

            // Enable FIFO, clear them, 14-byte threshold
            fifo_ctrl.write(0xC7);

            // IRQs enabled, RTS/DSR set
            modem_ctrl.write(0x0B);

            // Enable receiver data available interrupt
            int_enable.write(0x01);

            // Verify: read LSR to confirm UART is responsive
            let lsr = line_status.read();
            // Read data register to drain any stale bytes
            if lsr & 0x01 != 0 {
                let _ = data.read();
            }
        }
    }

    /// Write a byte to the UART (blocking until TX buffer is empty).
    pub fn put_char(&mut self, c: u8) {
        unsafe {
            let mut lsr = Port::<u8>::new(self.base + 5);
            let mut data = Port::<u8>::new(self.base);
            // Wait until THR is empty (bit 5 of LSR), but with a timeout.
            let mut timeout = 1_000_000u32;
            while lsr.read() & 0x20 == 0 {
                timeout -= 1;
                if timeout == 0 {
                    return; // Drop character to avoid deadlock
                }
                core::hint::spin_loop();
            }
            data.write(c);
        }
    }

    /// Write a batch of bytes to the UART (Linux-style).
    /// Waits for TX FIFO to drain once per 16-byte chunk, not per byte.
    /// Per-byte spin with inb() on real hardware costs hundreds of cycles;
    /// batching reduces this by 16x and is how the 8250 serial driver works.
    pub fn write_batch(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        unsafe {
            let mut lsr = Port::<u8>::new(self.base + 5);
            let mut thr = Port::<u8>::new(self.base);

            for chunk in data.chunks(16) {
                // Drain before each 16-byte burst
                let mut tout: u32 = 0xFFFF;
                while lsr.read() & 0x20 == 0 {
                    tout -= 1;
                    if tout == 0 {
                        break;
                    }
                }
                for &byte in chunk {
                    thr.write(byte);
                }
            }
            // Final drain
            let mut tout: u32 = 0xFFFF;
            while lsr.read() & 0x20 == 0 {
                tout -= 1;
                if tout == 0 {
                    break;
                }
            }
        }
    }

    /// Check if there is data available to read.
    pub fn has_data(&mut self) -> bool {
        unsafe {
            let mut lsr = Port::<u8>::new(self.base + 5);
            lsr.read() & 0x01 != 0
        }
    }

    /// Read a byte from the UART (non-blocking).
    /// Returns `None` if no data is available.
    pub fn get_char(&mut self) -> Option<u8> {
        if self.has_data() {
            unsafe {
                let mut data = Port::<u8>::new(self.base);
                Some(data.read())
            }
        } else {
            None
        }
    }
}

/// Global COM1 instance (protected by a spin lock for concurrent access).
static COM1: spin::Mutex<ComPort> = spin::Mutex::new(ComPort::new(COM1_BASE));

/// Whether a real UART hardware is present at COM1_BASE.
/// Determined once during init_uart(); false on systems without serial hardware.
static UART_PRESENT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Initialize COM1 UART.
pub fn init_uart() {
    let present = uart_probe();
    UART_PRESENT.store(present, core::sync::atomic::Ordering::Relaxed);
    if present {
        COM1.lock().init();
    }
}

/// Probe whether a real UART is present at COM1_BASE.
/// Standard 8250/16550 probe: write 0 to IER (Interrupt Enable Register at base+1),
/// then read it back. On a real UART the upper 4 bits are always 0. If reads back
/// all-ones (0xFF), the port is unconnected (open bus).
fn uart_probe() -> bool {
    unsafe {
        let ier_port: u16 = COM1_BASE + 1;
        // Read current IER
        let ier: u8;
        core::arch::asm!("in al, dx", out("al") ier, in("dx") ier_port);
        // Open bus usually reads 0xFF; upper nibble must be 0 on a real UART.
        if ier == 0xFF || ier & 0xF0 != 0 {
            return false;
        }
        // Write 0 and read back — must stay 0 in lower nibble
        core::arch::asm!("out dx, al", in("dx") ier_port, in("al") 0u8);
        let ier2: u8;
        core::arch::asm!("in al, dx", out("al") ier2, in("dx") ier_port);
        if ier2 == 0xFF {
            return false;
        }
        if ier2 & 0xF0 != 0 || ier2 != 0 {
            // After writing 0, IER should read as 0 on a real UART.
            // But some hardware might have RX interrupt enabled by firmware.
            // Accept any value where upper nibble is 0.
            return ier2 & 0xF0 == 0;
        }
        // Scratch register loopback test. A real 16550 echoes SCR writes;
        // random LPC/open-bus decodes do not.
        let scr_port: u16 = COM1_BASE + 7;
        core::arch::asm!("out dx, al", in("dx") scr_port, in("al") 0x5Au8);
        let scr: u8;
        core::arch::asm!("in al, dx", out("al") scr, in("dx") scr_port);
        if scr != 0x5A {
            return false;
        }
        core::arch::asm!("out dx, al", in("dx") scr_port, in("al") 0u8);
        true
    }
}

/// Write a byte to COM1 (console output).
pub fn putchar(c: u8) {
    if !UART_PRESENT.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    COM1.lock().put_char(c);
}

/// Write a batch to COM1 if a real UART is present.
pub fn write_batch(data: &[u8]) {
    if !UART_PRESENT.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    COM1.lock().write_batch(data);
}

/// Read a byte from COM1 (non-blocking).
/// Returns `None` if no data is available.
pub fn getchar() -> Option<u8> {
    if !UART_PRESENT.load(core::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    unsafe {
        // Read LSR directly via inline assembly to bypass any Port abstraction issues
        let lsr: u8;
        core::arch::asm!("in al, dx", out("al") lsr, in("dx") COM1_BASE + 5u16);
        if lsr == 0xFF {
            return None;
        }
        if lsr & 0x01 != 0 {
            let data: u8;
            core::arch::asm!("in al, dx", out("al") data, in("dx") COM1_BASE);
            if data == 0xFF {
                return None;
            }
            Some(data)
        } else {
            None
        }
    }
}

/// Check if COM1 has incoming data.
pub fn has_data() -> bool {
    if !UART_PRESENT.load(core::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    unsafe {
        let lsr: u8;
        core::arch::asm!("in al, dx", out("al") lsr, in("dx") COM1_BASE + 5u16);
        lsr & 0x01 != 0
    }
}
