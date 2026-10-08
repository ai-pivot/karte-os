//! Network interface and socket management.
//!
//! Manages the smoltcp Interface, SocketSet, and provides a high-level API
//! for network operations (polling, socket creation, data transfer).

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{icmp, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};

use super::device::NetDevice;

// ── DRT net integration (P2.5 dual-machine demo) ──
use crate::drt::DRT_PORT;
static DRT_FD: core::sync::atomic::AtomicIsize = core::sync::atomic::AtomicIsize::new(-1);
static ANNOUNCE_NEXT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static ANNOUNCE_SEQ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static EPHEMERAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[cfg(feature = "net_node_b")]
pub const NODE_PREFIX: &str = "b";
#[cfg(not(feature = "net_node_b"))]
pub const NODE_PREFIX: &str = "a";

/// This node's identity on the fabric (registered by peers as a remote device).
pub const fn self_node_id() -> &'static str {
    if NODE_PREFIX.as_bytes()[0] == b'b' {
        "b-node"
    } else {
        "a-node"
    }
}

fn drt_fd() -> isize {
    DRT_FD.load(core::sync::atomic::Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of simultaneous sockets.
const MAX_SOCKETS: usize = 16;

/// TCP receive buffer size per socket.
const TCP_RX_BUF_SIZE: usize = 4096;
/// TCP transmit buffer size per socket.
const TCP_TX_BUF_SIZE: usize = 4096;
/// UDP receive buffer size per socket.
const UDP_RX_BUF_SIZE: usize = 4096;
/// UDP transmit buffer size per socket.
const UDP_TX_BUF_SIZE: usize = 4096;
/// ICMP receive buffer size per socket.
const ICMP_RX_BUF_SIZE: usize = 4096;
/// ICMP transmit buffer size per socket.
const ICMP_TX_BUF_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// Socket type enumeration
// ---------------------------------------------------------------------------

/// Types of sockets supported by the network stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Tcp,
    Udp,
    Icmp,
}

/// Socket state tracked alongside the smoltcp socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Created,
    Bound,
    Listening,
    Connecting,
    Connected,
    Closed,
}

// ---------------------------------------------------------------------------
// Socket metadata (tracked outside smoltcp)
// ---------------------------------------------------------------------------

/// Metadata for a tracked socket.
#[derive(Clone)]
pub struct SocketMeta {
    pub handle: SocketHandle,
    pub socket_type: SocketType,
    pub state: SocketState,
}

// ---------------------------------------------------------------------------
// NetStack — global network state
// ---------------------------------------------------------------------------

pub struct NetStack {
    device: NetDevice,
    iface: Interface,
    socket_set: SocketSet<'static>,
    socket_metas: Vec<Option<SocketMeta>>,
}

static NET_STACK: spin::Mutex<Option<NetStack>> = spin::Mutex::new(None);

impl NetStack {
    /// Initialize the network stack.
    pub fn init(mac: [u8; 6]) {
        let hw_addr = HardwareAddress::Ethernet(EthernetAddress::from_bytes(&mac));
        let mut device = NetDevice::new(mac);

        let config = Config::new(hw_addr);
        let mut iface = Interface::new(config, &mut device, Instant::ZERO);

        // Configure IPv4: 10.0.2.15/24 (QEMU user-mode default)
        iface.update_ip_addrs(|ip_addrs| {
            ip_addrs
                .push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24))
                .unwrap();
        });

        // Add default route via gateway 10.0.2.2
        iface
            .routes_mut()
            .add_default_ipv4_route(smoltcp::wire::Ipv4Address::new(10, 0, 2, 2))
            .unwrap();

        crate::console_println!("[net] Interface configured: ip=10.0.2.15/24, gw=10.0.2.2");

        // Create socket storage (cannot use vec![] since SocketStorage doesn't impl Clone)
        let mut socket_storage: Vec<SocketStorage<'static>> = Vec::with_capacity(MAX_SOCKETS);
        for _ in 0..MAX_SOCKETS {
            socket_storage.push(SocketStorage::EMPTY);
        }
        let socket_set = SocketSet::new(socket_storage);

        let stack = NetStack {
            device,
            iface,
            socket_set,
            socket_metas: vec![None; MAX_SOCKETS],
        };

        *NET_STACK.lock() = Some(stack);
        crate::console_println!("[net] Network stack initialized");
    }

    /// Poll the network stack (called from timer interrupt).
    pub fn poll() {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Receive packets from VirtIO Net
        stack.device.receive_packet();

        let timestamp = Instant::from_millis(crate::arch::platform::uptime_ms() as i64);

        // Poll the smoltcp interface
        let _ = stack
            .iface
            .poll(timestamp, &mut stack.device, &mut stack.socket_set);

        // Update socket metadata states
        for slot in stack.socket_metas.iter_mut() {
            let meta = match slot {
                Some(m) => m,
                None => continue,
            };
            match meta.socket_type {
                SocketType::Tcp => {
                    let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
                    match sock.state() {
                        tcp::State::Established => meta.state = SocketState::Connected,
                        tcp::State::Listen => meta.state = SocketState::Listening,
                        tcp::State::SynSent => meta.state = SocketState::Connecting,
                        tcp::State::Closed => meta.state = SocketState::Closed,
                        _ => {}
                    }
                }
                SocketType::Udp => {
                    let sock = stack.socket_set.get_mut::<udp::Socket>(meta.handle);
                    if meta.state == SocketState::Created && sock.is_open() {
                        meta.state = SocketState::Bound;
                    }
                }
                _ => {}
            }
        }
    }

    /// Create a new socket and return its fd.
    pub fn create_socket(socket_type: SocketType) -> isize {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return -1,
        };

        let fd = match stack.socket_metas.iter().position(|s| s.is_none()) {
            Some(i) => i,
            None => return -1,
        };

        let handle = match socket_type {
            SocketType::Tcp => {
                let rx = tcp::SocketBuffer::new(vec![0u8; TCP_RX_BUF_SIZE]);
                let tx = tcp::SocketBuffer::new(vec![0u8; TCP_TX_BUF_SIZE]);
                stack.socket_set.add(tcp::Socket::new(rx, tx))
            }
            SocketType::Udp => {
                let rx = udp::PacketBuffer::new(
                    vec![udp::PacketMetadata::EMPTY; 4],
                    vec![0u8; UDP_RX_BUF_SIZE],
                );
                let tx = udp::PacketBuffer::new(
                    vec![udp::PacketMetadata::EMPTY; 4],
                    vec![0u8; UDP_TX_BUF_SIZE],
                );
                stack.socket_set.add(udp::Socket::new(rx, tx))
            }
            SocketType::Icmp => {
                let rx = icmp::PacketBuffer::new(
                    vec![icmp::PacketMetadata::EMPTY; 4],
                    vec![0u8; ICMP_RX_BUF_SIZE],
                );
                let tx = icmp::PacketBuffer::new(
                    vec![icmp::PacketMetadata::EMPTY; 4],
                    vec![0u8; ICMP_TX_BUF_SIZE],
                );
                stack.socket_set.add(icmp::Socket::new(rx, tx))
            }
        };

        stack.socket_metas[fd] = Some(SocketMeta {
            handle,
            socket_type,
            state: SocketState::Created,
        });

        crate::console_println!(
            "[net] Created {:?} socket: fd={}, handle={:?}",
            socket_type,
            fd,
            handle
        );
        fd as isize
    }

    /// Bind a socket to a local port.
    pub fn bind(fd: usize, port: u16) -> isize {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return -1,
        };

        let meta = match stack.socket_metas.get_mut(fd).and_then(|s| s.as_mut()) {
            Some(m) => m,
            None => return -1,
        };

        let endpoint = IpListenEndpoint { addr: None, port };

        match meta.socket_type {
            SocketType::Udp => {
                let sock = stack.socket_set.get_mut::<udp::Socket>(meta.handle);
                match sock.bind(endpoint) {
                    Ok(()) => {
                        meta.state = SocketState::Bound;
                        0
                    }
                    Err(_) => -1,
                }
            }
            SocketType::Tcp => {
                let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
                match sock.listen(endpoint) {
                    Ok(()) => {
                        meta.state = SocketState::Listening;
                        0
                    }
                    Err(_) => -1,
                }
            }
            _ => -1,
        }
    }

    /// Connect a TCP socket to a remote address.
    pub fn connect(fd: usize, ip: [u8; 4], port: u16) -> isize {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return -1,
        };

        let meta = match stack.socket_metas.get_mut(fd).and_then(|s| s.as_mut()) {
            Some(m) => m,
            None => return -1,
        };

        if meta.socket_type != SocketType::Tcp {
            return -1;
        }

        let remote_addr = IpAddress::v4(ip[0], ip[1], ip[2], ip[3]);

        // smoltcp 0.12 rejects local_port==0 as Unaddressable — allocate an
        // ephemeral port here (49152..=65534).
        let local_port =
            49152 + (EPHEMERAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % 16000) as u16;

        // TCP connect needs Context from Interface
        let cx = stack.iface.context();
        let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
        match sock.connect(cx, (remote_addr, port), local_port) {
            Ok(()) => {
                meta.state = SocketState::Connecting;
                crate::console_println!(
                    "[net] TCP connecting to {}.{}.{}.{}:{}",
                    ip[0],
                    ip[1],
                    ip[2],
                    ip[3],
                    port
                );
                0
            }
            Err(e) => {
                crate::console_println!("[net] TCP connect err: {:?}", e);
                -1
            }
        }
    }

    /// Send data on a socket.
    pub fn send(fd: usize, data: &[u8], ip: Option<[u8; 4]>, port: Option<u16>) -> isize {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return -1,
        };

        let meta = match stack.socket_metas.get(fd).and_then(|s| s.as_ref()) {
            Some(m) => m,
            None => return -1,
        };

        match meta.socket_type {
            SocketType::Tcp => {
                let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
                if !sock.can_send() {
                    return -2;
                }
                match sock.send_slice(data) {
                    Ok(n) => n as isize,
                    Err(_) => -1,
                }
            }
            SocketType::Udp => {
                let ip_addr = match ip {
                    Some([a, b, c, d]) => IpAddress::v4(a, b, c, d),
                    None => return -1,
                };
                let dst_port = match port {
                    Some(p) => p,
                    None => return -1,
                };
                let sock = stack.socket_set.get_mut::<udp::Socket>(meta.handle);
                match sock.send_slice(data, (ip_addr, dst_port)) {
                    Ok(()) => data.len() as isize,
                    Err(_) => -1,
                }
            }
            SocketType::Icmp => {
                let ip_addr = match ip {
                    Some([a, b, c, d]) => IpAddress::v4(a, b, c, d),
                    None => return -1,
                };
                let sock = stack.socket_set.get_mut::<icmp::Socket>(meta.handle);
                match sock.send_slice(data, ip_addr) {
                    Ok(()) => data.len() as isize,
                    Err(_) => -1,
                }
            }
        }
    }

    /// Receive data from a socket.
    pub fn recv(fd: usize, buf: &mut [u8]) -> Result<(usize, Option<[u8; 4]>, Option<u16>), isize> {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return Err(-1),
        };

        let meta = match stack.socket_metas.get(fd).and_then(|s| s.as_ref()) {
            Some(m) => m,
            None => return Err(-1),
        };

        match meta.socket_type {
            SocketType::Tcp => {
                let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
                if !sock.can_recv() {
                    return Err(-2); // EAGAIN — no data available
                }
                match sock.recv_slice(buf) {
                    Ok(n) => {
                        if n == 0 {
                            // smoltcp returns 0 when the recv buffer is empty
                            // but can_recv() was true — this means EOF (remote closed)
                            return Err(-3); // EOF / connection reset
                        }
                        Ok((n, None, None))
                    }
                    Err(_) => Err(-1),
                }
            }
            SocketType::Udp => {
                let sock = stack.socket_set.get_mut::<udp::Socket>(meta.handle);
                if !sock.can_recv() {
                    return Err(-2);
                }
                match sock.recv_slice(buf) {
                    Ok((n, udp_meta)) => {
                        let src_ip = match udp_meta.endpoint.addr {
                            IpAddress::Ipv4(ip) => Some(ip.octets()),
                            _ => None,
                        };
                        Ok((n, src_ip, Some(udp_meta.endpoint.port)))
                    }
                    Err(_) => Err(-1),
                }
            }
            SocketType::Icmp => {
                let sock = stack.socket_set.get_mut::<icmp::Socket>(meta.handle);
                if !sock.can_recv() {
                    return Err(-2);
                }
                match sock.recv_slice(buf) {
                    Ok((n, src_addr)) => {
                        let src_ip = match src_addr {
                            IpAddress::Ipv4(ip) => Some(ip.octets()),
                            _ => None,
                        };
                        Ok((n, src_ip, None))
                    }
                    Err(_) => Err(-1),
                }
            }
        }
    }

    /// Close a socket.
    pub fn close(fd: usize) -> isize {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return -1,
        };

        // Check if already closed (double-close protection)
        let meta = match stack.socket_metas.get(fd) {
            Some(Some(m)) => m.clone(),
            Some(None) => return 0, // Already closed, idempotent
            None => return -1,      // Invalid fd
        };

        // Close the socket properly
        match meta.socket_type {
            SocketType::Tcp => {
                stack.socket_set.get_mut::<tcp::Socket>(meta.handle).close();
            }
            SocketType::Udp => {
                stack.socket_set.get_mut::<udp::Socket>(meta.handle).close();
            }
            SocketType::Icmp => {
                // ICMP socket doesn't have close(), just remove from set
            }
        }

        // Remove from socket set
        let _ = stack.socket_set.remove(meta.handle);
        stack.socket_metas[fd] = None;

        0
    }

    /// Shut down a socket (TCP only).
    pub fn shutdown(fd: usize) -> isize {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return -1,
        };

        let meta = match stack.socket_metas.get_mut(fd).and_then(|s| s.as_mut()) {
            Some(m) => m,
            None => return -1,
        };

        if meta.socket_type == SocketType::Tcp {
            stack.socket_set.get_mut::<tcp::Socket>(meta.handle).close();
            meta.state = SocketState::Closed;
        }

        0
    }

    /// Check if a TCP socket is connected.
    pub fn is_connected(fd: usize) -> bool {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let meta = match stack.socket_metas.get(fd).and_then(|s| s.as_ref()) {
            Some(m) => m,
            None => return false,
        };

        if meta.socket_type == SocketType::Tcp {
            let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
            return sock.state() == tcp::State::Established;
        }

        meta.state == SocketState::Connected
    }

    /// Get the socket type for a given fd.
    pub fn get_socket_type(fd: usize) -> Option<SocketType> {
        let guard = NET_STACK.lock();
        let stack = guard.as_ref()?;
        stack
            .socket_metas
            .get(fd)
            .and_then(|s| s.as_ref())
            .map(|m| m.socket_type)
    }

    /// Check if the network stack is initialized.
    pub fn is_initialized() -> bool {
        NET_STACK.lock().is_some()
    }

    // ── DRT UDP integration (P2.5 dual-machine demo, port 43110) ──

    /// Bind an internal UDP socket for DRT announce/heartbeat/recv.
    /// Called once after network init. Device ids get a node prefix so two
    /// machines don't collide (node A: "a-...", node B: "b-...").
    pub fn drt_net_init() {
        if drt_fd() >= 0 {
            return;
        }
        let fd = Self::create_socket(SocketType::Udp);
        if fd < 0 {
            crate::console_println!("[drt-net] socket create failed");
            return;
        }
        if Self::bind(fd as usize, DRT_PORT) != 0 {
            crate::console_println!("[drt-net] bind {} failed", DRT_PORT);
            return;
        }
        DRT_FD.store(fd, core::sync::atomic::Ordering::Relaxed);
        crate::console_println!(
            "[drt-net] bound udp/{} fd={} node_prefix={}",
            DRT_PORT,
            fd,
            NODE_PREFIX
        );
    }

    /// Periodic DRT tick: announce self, then dispatch any received KRT1
    /// frames into the DRT state machine. Non-blocking.
    pub fn drt_net_tick(now_ms: u64) {
        let fd = drt_fd();
        if fd < 0 {
            return;
        }
        // Announce self every ~1s (seq makes it idempotent at the DRT).
        if now_ms >= ANNOUNCE_NEXT.load(core::sync::atomic::Ordering::Relaxed) {
            ANNOUNCE_NEXT.store(now_ms + 1000, core::sync::atomic::Ordering::Relaxed);
            let seq = ANNOUNCE_SEQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let frame = crate::drt::Drt::make_wire(b'A', self_node_id(), seq);
            // Unicast to the peer (subnet-consistent, ARP-resolvable).
            #[cfg(feature = "net_node_b")]
            let peer = [10, 0, 2, 15];
            #[cfg(not(feature = "net_node_b"))]
            let peer = [10, 0, 2, 16];
            let r = Self::send(fd as usize, &frame, Some(peer), Some(DRT_PORT));
            if r < 0 && seq < 3 {
                crate::console_println!("[drt-net] announce send err={} seq={}", r, seq);
            }
        }
        // RX EtherType census every ~5s (SYN-ACK hunt: IPv4 count should rise
        // if the TCP handshake is making progress).
        if ANNOUNCE_SEQ.load(core::sync::atomic::Ordering::Relaxed) % 5 == 0 {
            let (arp, v4, other) = (
                crate::driver::net::RX_TYPE_ARP.load(core::sync::atomic::Ordering::Relaxed),
                crate::driver::net::RX_TYPE_IPV4.load(core::sync::atomic::Ordering::Relaxed),
                crate::driver::net::RX_TYPE_OTHER.load(core::sync::atomic::Ordering::Relaxed),
            );
            crate::console_println!("[rx] census arp={} ipv4={} other={}", arp, v4, other);
        }
        // Dispatch received frames (up to 4 per tick).
        let mut buf = [0u8; 256];
        for _ in 0..4 {
            match Self::recv(fd as usize, &mut buf) {
                Ok((n, _, _)) if n > 0 => {
                    crate::drt::handle_wire(&buf[..n], now_ms);
                }
                _ => break,
            }
        }
    }
}
