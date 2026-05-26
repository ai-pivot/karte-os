// kernel/src/arch/plic.rs
// Platform-Level Interrupt Controller driver for QEMU virt machine

const PLIC_BASE: usize = 0x0C00_0000;

// PLIC register offsets
const PRIORITY_BASE: usize = 0x0000;
#[allow(unused)]
const PENDING_BASE: usize = 0x1000;
const ENABLE_BASE: usize = 0x2000;
const HART_BASE: usize = 0x200000;
const CLAIM_OFFSET: usize = 4;
const THRESHOLD_OFFSET: usize = 0;

/// Initialise PLIC for the given hart.
///
/// Sets threshold to 0 (accept all priorities) and enables the UART IRQ.
pub fn init(hart_id: usize) {
    set_threshold(hart_id, 0);
    // Enable UART interrupt (IRQ 10 on QEMU virt)
    enable(hart_id, 10, true);
}

fn read_reg(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((PLIC_BASE + offset) as *const u32) }
}

fn write_reg(offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((PLIC_BASE + offset) as *mut u32, value) }
}

/// Set the priority for an IRQ.
pub fn set_priority(irq: usize, priority: u32) {
    write_reg(PRIORITY_BASE + irq * 4, priority);
}

/// Enable or disable an IRQ for a given hart.
pub fn enable(hart_id: usize, irq: usize, enabled: bool) {
    let reg = ENABLE_BASE + hart_id * 0x80 + (irq / 32) * 4;
    let mut val = read_reg(reg);
    if enabled {
        val |= 1 << (irq % 32);
    } else {
        val &= !(1 << (irq % 32));
    }
    write_reg(reg, val);
}

/// Set the interrupt priority threshold for a given hart.
pub fn set_threshold(hart_id: usize, threshold: u32) {
    write_reg(HART_BASE + hart_id * 0x1000 + THRESHOLD_OFFSET, threshold);
}

/// Claim the highest-pending interrupt for a given hart.
pub fn claim(hart_id: usize) -> usize {
    read_reg(HART_BASE + hart_id * 0x1000 + CLAIM_OFFSET) as usize
}

/// Signal completion for an IRQ on a given hart.
pub fn complete(hart_id: usize, irq: usize) {
    write_reg(HART_BASE + hart_id * 0x1000 + CLAIM_OFFSET, irq as u32);
}

/// Top-level PLIC interrupt handler.
///
/// Claims the pending IRQ, dispatches, then completes it.
pub fn handle_interrupt(hart_id: usize) {
    let irq = claim(hart_id);
    if irq > 0 {
        match irq {
            10 => {
                // UART interrupt — read and echo
                if let Some(c) = crate::driver::uart::Uart::new(0x1000_0000).getc() {
                    crate::console_println!("[uart] received: {:02x}", c);
                }
            }
            _ => {
                crate::console_println!("[plic] Unhandled IRQ {}", irq);
            }
        }
        complete(hart_id, irq);
    }
}
