//! USB hub support — root hub port events and external hub enumeration.
//!
//! Phase 1 (current): root hub port status change polling is handled in
//! `xhci.rs::enumerate_devices`. This module will host the external hub
//! driver (Get Port Status / Set Port Feature requests over the hub's
//! interrupt endpoint) once multi-device enumeration is wired up.

#![allow(dead_code)]

pub const HUB_CLASS_REQ_GET_STATE: u8 = 0x0A;
pub const HUB_CLASS_REQ_SET_FEATURE: u8 = 0x03;
pub const HUB_CLASS_REQ_CLEAR_FEATURE: u8 = 0x01;
pub const HUB_CLASS_REQ_GET_DESC: u8 = 0x06;

pub const HUB_FEATURE_PORT_CONNECTION: u16 = 0;
pub const HUB_FEATURE_PORT_ENABLE: u16 = 1;
pub const HUB_FEATURE_PORT_RESET: u16 = 4;
pub const HUB_FEATURE_PORT_POWER: u16 = 8;
pub const HUB_FEATURE_C_PORT_CONNECTION: u16 = 16;
pub const HUB_FEATURE_C_PORT_RESET: u16 = 20;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct HubDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_nbr_ports: u8,
    pub w_hub_characteristics: u16,
    pub b_pwr_on_2_pwr_good: u8,
    pub b_hub_control_current: u8,
    pub device_removable: [u8; 0], // variable length
}

/// Placeholder for external hub enumeration. Returns Ok(()) with no ports
/// for now; root-hub enumeration lives in `xhci.rs`.
pub fn enumerate_external_hub(_slot: u8) -> Result<u8, &'static str> {
    Err("external hub enumeration not yet implemented")
}
