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
    /// Connected UDP: remote address (set by connect() on SOCK_DGRAM)
    pub remote_ip: Option<[u8; 4]>,
    pub remote_port: Option<u16>,
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
    }

    /// Poll the network stack (called from timer interrupt).
    pub fn poll() {
        // Use try_lock: this is called from timer ISR.
        // If a syscall holds the lock, skip this tick.
        let mut guard = match NET_STACK.try_lock() {
            Some(g) => g,
            None => return,
        };
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
            remote_ip: None,
            remote_port: None,
        });

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

    /// Connect a socket to a remote address.
    /// For TCP: initiates SYN handshake. For UDP: stores default destination.
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

        match meta.socket_type {
            SocketType::Tcp => {
                let remote_addr = IpAddress::v4(ip[0], ip[1], ip[2], ip[3]);
                let cx = stack.iface.context();
                let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
                match sock.connect(cx, (remote_addr, port), 0) {
                    Ok(()) => {
                        meta.state = SocketState::Connecting;
                        0
                    }
                    Err(_) => -1,
                }
            }
            SocketType::Udp => {
                // Connected UDP: store the remote address. Bind to ephemeral port
                // if not already bound, so smoltcp can send.
                let endpoint = IpListenEndpoint {
                    addr: None,
                    port: 0,
                };
                let sock = stack.socket_set.get_mut::<udp::Socket>(meta.handle);
                if !sock.is_open() {
                    let _ = sock.bind(endpoint);
                }
                meta.remote_ip = Some(ip);
                meta.remote_port = Some(port);
                meta.state = SocketState::Connected;
                0
            }
            _ => -1,
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
                // Use explicit destination if provided, else fall back to connected address
                let (ip_addr, dst_port) = match (ip, port) {
                    (Some([a, b, c, d]), Some(p)) => (IpAddress::v4(a, b, c, d), p),
                    _ => match (meta.remote_ip, meta.remote_port) {
                        (Some(rip), Some(rport)) => {
                            (IpAddress::v4(rip[0], rip[1], rip[2], rip[3]), rport)
                        }
                        _ => return -1,
                    },
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

    pub fn close_socket(fd: usize) -> isize {
        Self::close(fd)
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

    /// Check if a socket has data available to read (for epoll).
    pub fn can_recv(fd: usize) -> bool {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };
        let meta = match stack.socket_metas.get(fd).and_then(|s| s.as_ref()) {
            Some(m) => m,
            None => return false,
        };
        match meta.socket_type {
            SocketType::Tcp => {
                let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
                sock.can_recv()
            }
            SocketType::Udp => {
                let sock = stack.socket_set.get_mut::<udp::Socket>(meta.handle);
                sock.can_recv()
            }
            _ => false,
        }
    }

    /// Check if a socket can accept outgoing data (for epoll).
    pub fn can_send(fd: usize) -> bool {
        let mut guard = NET_STACK.lock();
        let stack = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };
        let meta = match stack.socket_metas.get(fd).and_then(|s| s.as_ref()) {
            Some(m) => m,
            None => return false,
        };
        match meta.socket_type {
            SocketType::Tcp => {
                let sock = stack.socket_set.get_mut::<tcp::Socket>(meta.handle);
                sock.can_send()
            }
            SocketType::Udp => {
                let sock = stack.socket_set.get_mut::<udp::Socket>(meta.handle);
                sock.can_send()
            }
            _ => true,
        }
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
}
