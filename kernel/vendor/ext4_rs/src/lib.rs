#![no_std]
#![allow(unused)]

extern crate alloc;

pub mod prelude;
pub mod utils;

pub use prelude::*;
pub use utils::*;

pub mod ext4_defs;
pub mod ext4_impls;

pub mod fuse_interface;
pub mod simple_interface;

// Re-export key items at the crate root.
// Individual items are imported by consumers from their submodules
// (e.g., ext4_rs::ext4_impls::ext4::Ext4) to avoid glob conflicts.
pub use ext4_defs::block::*;
pub use ext4_defs::consts::*;
pub use ext4_impls::ext4::*;
pub use simple_interface::*;
