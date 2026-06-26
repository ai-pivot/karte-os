//! USB core — architecture-independent types, descriptors, and parser.
//!
//! This module defines the USB device/endpoint model used by host controller
//! drivers (currently xHCI on x86_64). It contains no MMIO and can be unit
//! tested on any target.

#![allow(dead_code)]

#[cfg(target_arch = "x86_64")]
pub mod hid;
#[cfg(target_arch = "x86_64")]
pub mod hub;
#[cfg(target_arch = "x86_64")]
pub mod xhci;

// Box is provided by alloc; we leak descriptor buffers into 'static lifetime
// because descriptors are parsed at most a handful of times during boot and
// the parsed views must outlive the temporary DMA buffer they were read into.
use alloc::boxed::Box;

// ─── USB standard requests ─────────────────────────────────────────────

pub const USB_REQ_GET_STATUS: u8 = 0x00;
pub const USB_REQ_CLEAR_FEATURE: u8 = 0x01;
pub const USB_REQ_SET_FEATURE: u8 = 0x03;
pub const USB_REQ_SET_ADDRESS: u8 = 0x05;
pub const USB_REQ_GET_DESCRIPTOR: u8 = 0x06;
pub const USB_REQ_SET_DESCRIPTOR: u8 = 0x07;
pub const USB_REQ_GET_CONFIGURATION: u8 = 0x08;
pub const USB_REQ_SET_CONFIGURATION: u8 = 0x09;
pub const USB_REQ_GET_INTERFACE: u8 = 0x0A;
pub const USB_REQ_SET_INTERFACE: u8 = 0x0B;
pub const USB_REQ_SYNCH_FRAME: u8 = 0x0C;

// HID class requests
pub const HID_REQ_GET_REPORT: u8 = 0x01;
pub const HID_REQ_SET_REPORT: u8 = 0x09;
pub const HID_REQ_SET_IDLE: u8 = 0x0A;
pub const HID_REQ_SET_PROTOCOL: u8 = 0x0B;

// Request type bit fields (bmRequestType)
pub const USB_DIR_OUT: u8 = 0x00;
pub const USB_DIR_IN: u8 = 0x80;
pub const USB_TYPE_STANDARD: u8 = 0x00;
pub const USB_TYPE_CLASS: u8 = 0x20;
pub const USB_TYPE_VENDOR: u8 = 0x40;
pub const USB_RECIP_DEVICE: u8 = 0x00;
pub const USB_RECIP_INTERFACE: u8 = 0x01;
pub const USB_RECIP_ENDPOINT: u8 = 0x02;

// Descriptor types
pub const USB_DESC_DEVICE: u8 = 0x01;
pub const USB_DESC_CONFIGURATION: u8 = 0x02;
pub const USB_DESC_STRING: u8 = 0x03;
pub const USB_DESC_INTERFACE: u8 = 0x04;
pub const USB_DESC_ENDPOINT: u8 = 0x05;
pub const USB_DESC_INTERFACE_POWER: u8 = 0x06;
pub const USB_DESC_HID: u8 = 0x21;
pub const USB_DESC_HID_REPORT: u8 = 0x22;
pub const USB_DESC_HID_PHYSICAL: u8 = 0x23;

// USB classes
pub const USB_CLASS_PER_INTERFACE: u8 = 0x00;
pub const USB_CLASS_AUDIO: u8 = 0x01;
pub const USB_CLASS_COMM: u8 = 0x02;
pub const USB_CLASS_HID: u8 = 0x03;
pub const USB_CLASS_PHYSICAL: u8 = 0x05;
pub const USB_CLASS_HUB: u8 = 0x09;
pub const USB_CLASS_MASS_STORAGE: u8 = 0x08;

// Endpoint direction
pub const USB_ENDPOINT_DIR_OUT: u8 = 0x00;
pub const USB_ENDPOINT_DIR_IN: u8 = 0x80;
pub const USB_ENDPOINT_NUMBER_MASK: u8 = 0x0F;

// Endpoint transfer type
pub const USB_ENDPOINT_XFER_CONTROL: u8 = 0x00;
pub const USB_ENDPOINT_XFER_ISOC: u8 = 0x01;
pub const USB_ENDPOINT_XFER_BULK: u8 = 0x02;
pub const USB_ENDPOINT_XFER_INT: u8 = 0x03;

// USB speeds (matches xHCI PORTSC speed field encoding)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbSpeed {
    Full = 0,  // USB 1.1/2.0 full-speed (12 Mbps)
    Low = 1,   // USB 1.1 low-speed (1.5 Mbps)
    High = 2,  // USB 2.0 high-speed (480 Mbps)
    Super = 3, // USB 3.0 super-speed (5 Gbps)
    Unknown = 4,
}

impl UsbSpeed {
    pub fn from_xhci_code(code: u32) -> Self {
        match code {
            1 => UsbSpeed::Full,
            2 => UsbSpeed::Low,
            3 => UsbSpeed::High,
            4 => UsbSpeed::Super,
            _ => UsbSpeed::Unknown,
        }
    }

    /// Return the xHCI hardware speed code used in the Slot Context speed field
    /// (dword 1, bits 4:7) and the PORTSC speed field. This is distinct from the
    /// enum discriminant: hardware codes are 1=Full, 2=Low, 3=High, 4=Super.
    pub fn xhci_code(self) -> u32 {
        match self {
            UsbSpeed::Full => 1,
            UsbSpeed::Low => 2,
            UsbSpeed::High => 3,
            UsbSpeed::Super => 4,
            UsbSpeed::Unknown => 0,
        }
    }
}

// ─── Setup packet (8 bytes, sent over control endpoint) ────────────────

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct SetupPacket {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}

impl SetupPacket {
    pub const fn new(
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        w_length: u16,
    ) -> Self {
        Self {
            bm_request_type,
            b_request,
            w_value,
            w_index,
            w_length,
        }
    }

    /// Encode as the 64-bit `parameter` field of an xHCI Setup TRB.
    pub fn encode_trb_parameter(&self) -> u64 {
        let mut p: u64 = 0;
        p |= self.bm_request_type as u64;
        p |= (self.b_request as u64) << 8;
        p |= (self.w_value as u64) << 16;
        p |= (self.w_index as u64) << 32;
        p |= (self.w_length as u64) << 48;
        p
    }
}

// ─── Descriptor structures (all little-endian, packed) ─────────────────

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct DeviceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub bcd_usb: u16,
    pub b_device_class: u8,
    pub b_device_subclass: u8,
    pub b_device_protocol: u8,
    pub b_max_packet_size0: u8,
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_device: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial: u8,
    pub b_num_configurations: u8,
}

impl DeviceDescriptor {
    /// Parse from a raw byte slice. Returns None if too short or wrong type.
    pub fn parse(data: &[u8]) -> Option<&'static Self> {
        if data.len() < 18 || data[1] != USB_DESC_DEVICE {
            return None;
        }
        // SAFETY: DeviceDescriptor is repr(C, packed) with no padding; the
        // raw bytes are copied into a 'static leaked box so the caller does
        // not hold a borrow into the possibly-temporary input slice.
        let mut buf = alloc::boxed::Box::new(unsafe { core::mem::zeroed::<Self>() });
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                &mut *buf as *mut Self as *mut u8,
                core::mem::size_of::<Self>(),
            );
        }
        Some(Box::leak(buf))
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ConfigDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub w_total_length: u16,
    pub b_num_interfaces: u8,
    pub b_configuration_value: u8,
    pub i_configuration: u8,
    pub bm_attributes: u8,
    pub b_max_power: u8,
}

impl ConfigDescriptor {
    pub fn parse(data: &[u8]) -> Option<&'static Self> {
        if data.len() < 9 || data[1] != USB_DESC_CONFIGURATION {
            return None;
        }
        let mut buf = alloc::boxed::Box::new(unsafe { core::mem::zeroed::<Self>() });
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                &mut *buf as *mut Self as *mut u8,
                core::mem::size_of::<Self>(),
            );
        }
        Some(Box::leak(buf))
    }

    pub fn total_length(&self) -> usize {
        self.w_total_length as usize
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct InterfaceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_interface_number: u8,
    pub b_alternate_setting: u8,
    pub b_num_endpoints: u8,
    pub b_interface_class: u8,
    pub b_interface_subclass: u8,
    pub b_interface_protocol: u8,
    pub i_interface: u8,
}

impl InterfaceDescriptor {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 9 || data[1] != USB_DESC_INTERFACE {
            return None;
        }
        let mut d = unsafe { core::mem::zeroed::<Self>() };
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                &mut d as *mut Self as *mut u8,
                core::mem::size_of::<Self>(),
            );
        }
        Some(d)
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct EndpointDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_endpoint_address: u8,
    pub bm_attributes: u8,
    pub w_max_packet_size: u16,
    pub b_interval: u8,
}

impl EndpointDescriptor {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 7 || data[1] != USB_DESC_ENDPOINT {
            return None;
        }
        let mut d = unsafe { core::mem::zeroed::<Self>() };
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                &mut d as *mut Self as *mut u8,
                core::mem::size_of::<Self>(),
            );
        }
        Some(d)
    }

    pub fn direction_in(&self) -> bool {
        self.b_endpoint_address & USB_ENDPOINT_DIR_IN != 0
    }

    pub fn number(&self) -> u8 {
        self.b_endpoint_address & USB_ENDPOINT_NUMBER_MASK
    }

    pub fn transfer_type(&self) -> u8 {
        self.bm_attributes & 0x03
    }

    pub fn max_packet_size(&self) -> u16 {
        self.w_max_packet_size
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct HidDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub bcd_hid: u16,
    pub b_country_code: u8,
    pub b_num_descriptors: u8,
    pub b_report_descriptor_type: u8,
    pub w_descriptor_length: u16,
}

impl HidDescriptor {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 9 || data[1] != USB_DESC_HID {
            return None;
        }
        let mut d = unsafe { core::mem::zeroed::<Self>() };
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                &mut d as *mut Self as *mut u8,
                core::mem::size_of::<Self>(),
            );
        }
        Some(d)
    }
}

// ─── Configuration descriptor walker ───────────────────────────────────

/// Walks a full configuration descriptor blob and yields (offset, header)
/// for each sub-descriptor so callers can locate interfaces, endpoints, and
/// HID descriptors without manual pointer arithmetic.
pub fn walk_config_descriptors<'a>(data: &'a [u8]) -> ConfigDescriptorIter<'a> {
    ConfigDescriptorIter { data, pos: 0 }
}

pub struct ConfigDescriptorIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for ConfigDescriptorIter<'a> {
    type Item = (&'a [u8], u8, u8);

    /// Yields (slice, b_length, b_descriptor_type) for each descriptor.
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos + 2 > self.data.len() {
            return None;
        }
        let len = self.data[self.pos] as usize;
        let typ = self.data[self.pos + 1];
        if len < 2 || self.pos + len > self.data.len() {
            return None;
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Some((slice, len as u8, typ))
    }
}

/// Parsed view of a configuration descriptor: the config header plus a list
/// of (interface, [endpoints], optional HID descriptor) groups.
pub struct ParsedConfiguration {
    pub config: &'static ConfigDescriptor,
    pub interfaces: alloc::vec::Vec<ParsedInterface>,
}

pub struct ParsedInterface {
    pub iface: InterfaceDescriptor,
    pub hid: Option<HidDescriptor>,
    pub endpoints: alloc::vec::Vec<EndpointDescriptor>,
}

impl ParsedConfiguration {
    /// Parse a full configuration descriptor blob (wTotalLength bytes).
    pub fn parse(blob: &[u8]) -> Option<Self> {
        let config = ConfigDescriptor::parse(blob)?;
        let total = config.total_length().min(blob.len());
        let mut interfaces = alloc::vec::Vec::new();
        let mut current: Option<ParsedInterface> = None;

        for (slice, _len, typ) in walk_config_descriptors(&blob[..total]) {
            match typ {
                USB_DESC_INTERFACE => {
                    if let Some(iface) = InterfaceDescriptor::parse(slice) {
                        if let Some(prev) = current.take() {
                            interfaces.push(prev);
                        }
                        current = Some(ParsedInterface {
                            iface,
                            hid: None,
                            endpoints: alloc::vec::Vec::new(),
                        });
                    }
                }
                USB_DESC_ENDPOINT => {
                    if let Some(ep) = EndpointDescriptor::parse(slice) {
                        if let Some(pi) = current.as_mut() {
                            pi.endpoints.push(ep);
                        }
                    }
                }
                USB_DESC_HID => {
                    if let Some(hid) = HidDescriptor::parse(slice) {
                        if let Some(pi) = current.as_mut() {
                            pi.hid = Some(hid);
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(prev) = current.take() {
            interfaces.push(prev);
        }
        Some(ParsedConfiguration { config, interfaces })
    }

    /// Find the first standard HID boot keyboard interface and return its
    /// interrupt IN endpoint. HID protocol 0 is intentionally not accepted here:
    /// those devices must be checked via their HID Report Descriptor.
    pub fn find_hid_keyboard(&self) -> Option<(&ParsedInterface, &EndpointDescriptor)> {
        for pi in &self.interfaces {
            if pi.iface.b_interface_class == USB_CLASS_HID
                && pi.iface.b_interface_subclass == 0x01
                && pi.iface.b_interface_protocol == 0x01
            {
                for ep in &pi.endpoints {
                    if ep.direction_in() && ep.transfer_type() == USB_ENDPOINT_XFER_INT {
                        return Some((pi, ep));
                    }
                }
            }
        }
        None
    }
}

/// Return true when a HID Report Descriptor advertises keyboard input usages.
/// This is deliberately small: enough to distinguish real keyboard reports
/// from mouse/consumer-control `protocol=0` HID devices.
pub fn hid_report_has_keyboard_usage(desc: &[u8]) -> bool {
    let mut i = 0usize;
    let mut usage_page = 0u32;
    while i < desc.len() {
        let prefix = desc[i];
        i += 1;
        if prefix == 0xFE {
            if i + 2 > desc.len() {
                return false;
            }
            let len = desc[i] as usize;
            i += 2;
            if i + len > desc.len() {
                return false;
            }
            i += len;
            continue;
        }

        let size = match prefix & 0x03 {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        if i + size > desc.len() {
            return false;
        }
        let mut value = 0u32;
        for b in 0..size {
            value |= (desc[i + b] as u32) << (8 * b);
        }
        i += size;

        let item_type = (prefix >> 2) & 0x03;
        let item_tag = (prefix >> 4) & 0x0F;
        match (item_type, item_tag) {
            // Global: Usage Page
            (1, 0x0) => usage_page = value,
            // Local: Usage. Generic Desktop/Keyboard application or any
            // Keyboard/Keypad page usage means the report can produce keys.
            (2, 0x0) => {
                if (usage_page == 0x01 && value == 0x06) || usage_page == 0x07 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

// ─── USB device model ──────────────────────────────────────────────────

/// A USB device known to the host controller. One slot per device.
pub struct UsbDevice {
    pub slot_id: u8,
    pub address: u8,
    pub speed: UsbSpeed,
    pub vendor_id: u16,
    pub product_id: u16,
    pub class: u8,
    pub config_value: u8,
    pub max_packet_size0: u16,
}

impl UsbDevice {
    pub fn new(slot_id: u8, speed: UsbSpeed) -> Self {
        Self {
            slot_id,
            address: 0,
            speed,
            vendor_id: 0,
            product_id: 0,
            class: 0,
            config_value: 0,
            max_packet_size0: 8,
        }
    }
}
