#[cfg(target_arch = "riscv64")]
pub mod virtio;

#[cfg(target_arch = "riscv64")]
pub mod net;

pub mod block;
pub mod ext4;
pub mod fat32;
pub mod fs;
pub mod pipe;
pub mod ramfs;
pub mod tty;
pub mod uart;
pub mod vfs;
