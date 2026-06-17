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

#[cfg(target_arch = "x86_64")]
pub mod x86_64 {
    /// Physical address where GRUB loads the kernel image (1 MB).
    pub const KERNEL_PHYS_BASE: usize = 0x10_0000;

    /// Base of the kernel's direct-map region in virtual address space.
    ///
    /// All physical memory is accessible at `DIRECT_MAP_BASE + phys_addr`.
    ///
    /// **Configurable**: This MUST be a canonical upper-half address within
    /// the top 2 GB of virtual address space (i.e., in the range
    /// `0xFFFF_FFFF_8000_0000`..`0xFFFF_FFFF_FFFF_FFFF`) to be compatible
    /// with the `kernel` code model (`-C code-model=kernel`), which uses
    /// sign-extended 32-bit offsets. This matches the convention used by
    /// Linux and other x86_64 kernels.
    ///
    /// The default places the direct map at the very start of the top-2GB
    /// window, giving the kernel the full 2 GB for code + data + direct map.
    pub const DIRECT_MAP_BASE: usize = 0xFFFF_FFFF_8000_0000;

    /// Virtual address where the kernel code and data reside.
    ///
    /// Computed at link time as `DIRECT_MAP_BASE + _kernel_phys_start`.
    /// This constant is an approximation — the linker script computes the
    /// exact value based on the boot section size. The boot code uses
    /// `movabs $kmain` which the linker resolves to the correct VMA.
    pub const KERNEL_VMA: usize = DIRECT_MAP_BASE + KERNEL_PHYS_BASE;

    /// Convert a physical address to its virtual alias in the direct map.
    #[inline]
    pub const fn phys_to_virt(paddr: usize) -> usize {
        paddr + DIRECT_MAP_BASE
    }

    /// Convert a direct-map virtual address back to its physical address.
    #[inline]
    pub const fn virt_to_phys(vaddr: usize) -> usize {
        vaddr - DIRECT_MAP_BASE
    }

    /// MMIO base addresses (identity-mapped in kernel page tables).
    pub const VGA_TEXT_BUFFER: usize = 0xB8000;
    pub const LAPIC_BASE: usize = 0xFEE0_0000;
    pub const IOAPIC_BASE: usize = 0xFEC0_0000;
    pub const UART_PORT: u16 = 0x3F8;
}
