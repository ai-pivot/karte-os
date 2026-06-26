#[cfg(target_arch = "riscv64")]
pub mod net;

#[cfg(target_arch = "x86_64")]
pub mod vga;

#[cfg(target_arch = "x86_64")]
pub mod keyboard;

#[cfg(target_arch = "x86_64")]
pub mod ahci;

// The legacy `driver/xhci.rs` prototype has been replaced by the full
// `driver::usb::xhci` implementation. The old file is removed from the
// module tree to avoid duplicate symbols; callers use `driver::usb::xhci`.
#[cfg(target_arch = "x86_64")]
pub mod usb;

#[cfg(target_arch = "x86_64")]
pub mod nvme;

pub mod p9;

pub mod virtio;

pub mod block;
pub mod ext4;
pub mod fat32;
pub mod fs;
pub mod pipe;
pub mod ramfs;
pub mod tty;
pub mod uart;
pub mod vfs;
