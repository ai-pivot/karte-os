// kernel/src/driver/uart.rs
// QEMU virt machine UART is ns16550a compatible at 0x1000_0000

use core::fmt;
use core::option::Option::{self, Some, None};
use core::result::Result::Ok;

#[allow(unused)]
const UART_BASE: usize = 0x1000_0000;

// Register offsets
const THR: usize = 0; // Transmit Holding Register (write)
const RBR: usize = 0; // Receive Buffer Register (read)
const IER: usize = 1; // Interrupt Enable Register
const FCR: usize = 2; // FIFO Control Register (write)
#[allow(unused)]
const ISR: usize = 2; // Interrupt Status Register (read)
const LCR: usize = 3; // Line Control Register
const MCR: usize = 4; // Modem Control Register
const LSR: usize = 5; // Line Status Register

// LSR bits
const LSR_TX_EMPTY: u8 = 0x20;
const LSR_DATA_READY: u8 = 0x01;

// LCR bits
const LCR_8N1: u8 = 0x03; // 8 data bits, no parity, 1 stop bit
const LCR_DLAB: u8 = 0x80; // Divisor Latch Access Bit

pub struct Uart {
    base: usize,
}

impl Uart {
    pub fn new(base: usize) -> Self {
        Self { base }
    }

    pub fn init(&self) {
        // Disable interrupts
        self.write_reg(IER, 0x00);

        // Set baud rate (divisor = 3 for 115200 baud; QEMU ignores this but we set it anyway)
        self.write_reg(LCR, LCR_DLAB);
        self.write_reg(0, 0x03); // DLL
        self.write_reg(1, 0x00); // DLH

        // 8N1 mode
        self.write_reg(LCR, LCR_8N1);

        // Enable FIFO, clear them, 14-byte threshold
        self.write_reg(FCR, 0xC7);

        // IRQs enabled, RTS/DSR set
        self.write_reg(MCR, 0x0B);

        // Enable receiver interrupt
        self.write_reg(IER, 0x01);
    }

    fn read_reg(&self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u8) }
    }

    fn write_reg(&self, offset: usize, value: u8) {
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u8, value) }
    }

    pub fn putc(&self, c: u8) {
        // Wait until TX buffer is empty
        while self.read_reg(LSR) & LSR_TX_EMPTY == 0 {}
        self.write_reg(THR, c);
    }

    pub fn getc(&self) -> Option<u8> {
        if self.read_reg(LSR) & LSR_DATA_READY != 0 {
            Some(self.read_reg(RBR))
        } else {
            None
        }
    }

    pub fn puts(&self, s: &str) {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.putc(b'\r');
            }
            self.putc(byte);
        }
    }
}

impl fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.puts(s);
        Ok(())
    }
}
