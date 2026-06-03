//! smoltcp Device trait implementation wrapping VirtIO Net.
//!
//! This adapter bridges the VirtIO Net MMIO driver (raw packet I/O)
//! with smoltcp's token-based Device interface.

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

// ---------------------------------------------------------------------------
// Packet buffer (single-buffer design, sufficient for polling-based stack)
// ---------------------------------------------------------------------------

/// Maximum Ethernet frame size (standard MTU)
const MTU: usize = 1500;
const MAX_FRAME: usize = MTU + 18; // extra margin for header + FCS

/// Shared state between the VirtIO Net driver and smoltcp tokens.
struct NetState {
    /// Received packet waiting to be consumed by smoltcp.
    rx_buf: [u8; MAX_FRAME],
    rx_len: usize,
}

impl NetState {
    const fn new() -> Self {
        Self {
            rx_buf: [0u8; MAX_FRAME],
            rx_len: 0,
        }
    }
}

/// Global state protected by a spinlock.
static NET_STATE: spin::Mutex<NetState> = spin::Mutex::new(NetState::new());

// ---------------------------------------------------------------------------
// RxToken / TxToken
// ---------------------------------------------------------------------------

/// Token for consuming a received packet.
pub struct VirtRxToken;

impl RxToken for VirtRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let mut state = NET_STATE.lock();
        let result = if state.rx_len > 0 {
            f(&state.rx_buf[..state.rx_len])
        } else {
            f(&[])
        };
        // Mark packet as consumed
        state.rx_len = 0;
        result
    }
}

/// Token for producing a packet to transmit.
pub struct VirtTxToken;

impl TxToken for VirtTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let write_len = core::cmp::min(len, MAX_FRAME);

        // Use a stack buffer to avoid holding the lock while calling f()
        let mut buf = [0u8; MAX_FRAME];
        let result = f(&mut buf[..write_len]);

        // Actually transmit via VirtIO Net driver
        if write_len > 0 {
            crate::driver::net::send_raw(&buf[..write_len]);
        }

        result
    }
}

// ---------------------------------------------------------------------------
// NetDevice — implements smoltcp::phy::Device
// ---------------------------------------------------------------------------

/// smoltcp Device adapter wrapping the VirtIO Net driver.
pub struct NetDevice {
    mac: [u8; 6],
}

impl NetDevice {
    /// Create a new NetDevice with the given MAC address.
    pub fn new(mac: [u8; 6]) -> Self {
        Self { mac }
    }

    /// Poll the VirtIO Net driver for received packets and buffer them.
    /// Should be called before smoltcp's Interface::poll().
    pub fn receive_packet(&mut self) {
        let state = NET_STATE.lock();

        // Don't overwrite an unconsumed packet
        if state.rx_len > 0 {
            return;
        }

        drop(state); // Release lock before calling recv_raw (which also locks)

        let mut buf = [0u8; MAX_FRAME];
        match crate::driver::net::recv_raw(&mut buf) {
            Some(len) => {
                let mut state = NET_STATE.lock();
                state.rx_buf[..len].copy_from_slice(&buf[..len]);
                state.rx_len = len;
            }
            None => {}
        }
    }

    /// Returns the MAC address.
    pub fn mac_addr(&self) -> [u8; 6] {
        self.mac
    }
}

impl Device for NetDevice {
    type RxToken<'a> = VirtRxToken;
    type TxToken<'a> = VirtTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let state = NET_STATE.lock();
        if state.rx_len > 0 {
            // Drop lock before returning tokens (tokens will re-acquire)
            drop(state);
            Some((VirtRxToken, VirtTxToken))
        } else {
            None
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(VirtTxToken)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = MTU;
        caps.max_burst_size = Some(1);
        caps
    }
}
