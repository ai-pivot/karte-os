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

            // Disable all interrupts
            int_enable.write(0x00);

            // Enable DLAB (set baud rate divisor)
            line_ctrl.write(0x80);
            // Set divisor to 1 (115200 baud) — QEMU ignores this but we set it
            data.write(0x01); // DLL (divisor latch low)
            int_enable.write(0x00); // DLH (divisor latch high)

            // 8 bits, no parity, one stop bit (8N1), disable DLAB
            line_ctrl.write(0x03);

            // Enable FIFO, clear them, 14-byte threshold
            fifo_ctrl.write(0xC7);

            // IRQs enabled, RTS/DSR set
            modem_ctrl.write(0x0B);

            // Enable receiver data available interrupt
            int_enable.write(0x01);
        }
    }

    /// Write a byte to the UART (blocking until TX buffer is empty).
    pub fn put_char(&mut self, c: u8) {
        unsafe {
            let mut lsr = Port::<u8>::new(self.base + 5);
            let mut data = Port::<u8>::new(self.base);
            // Wait until THR is empty (bit 5 of LSR)
            while lsr.read() & 0x20 == 0 {}
            data.write(c);
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

/// Initialize COM1 UART.
pub fn init_uart() {
    COM1.lock().init();
}

/// Write a byte to COM1 (console output).
pub fn putchar(c: u8) {
    COM1.lock().put_char(c);
}

/// Read a byte from COM1 (non-blocking).
pub fn getchar() -> Option<u8> {
    COM1.lock().get_char()
}

/// Check if COM1 has incoming data.
pub fn has_data() -> bool {
    COM1.lock().has_data()
}
