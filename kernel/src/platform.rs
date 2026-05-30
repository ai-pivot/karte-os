//! Platform-specific constants for each architecture.
//!
//! Each architecture provides its own set of constants for MMIO base addresses,
//! memory layout, and other hardware-specific values.

#[cfg(target_arch = "riscv64")]
pub mod riscv64 {
    pub const UART_BASE: usize = 0x1000_0000;
    pub const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
    pub const VIRTIO_MMIO_STRIDE: usize = 0x1000;
    pub const PLIC_BASE: usize = 0x0C00_0000;
    pub const KERNEL_LOAD_ADDR: usize = 0x8020_0000;
    pub const MEMORY_SIZE: usize = 128 * 1024 * 1024;
    pub const USER_STACK_TOP: usize = 0x8000_0000;
    pub const USER_CODE_BASE: usize = 0x1000;
}
