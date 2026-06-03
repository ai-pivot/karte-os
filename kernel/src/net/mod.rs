//! Network protocol stack module.
//!
//! Uses smoltcp as the TCP/IP stack, wrapped around the VirtIO Net driver.
//! Provides:
//! - Ethernet + ARP + IPv4 + ICMP (ping)
//! - UDP sockets
//! - TCP sockets
//! - Syscall interface for user programs

pub mod device;
pub mod iface;

pub use device::NetDevice;
pub use iface::NetStack;
