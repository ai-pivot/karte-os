# Network Stack

## Overview

KarteOS uses **smoltcp 0.12** as its TCP/IP protocol stack, providing:
- Ethernet + ARP + IPv4 + ICMP (ping)
- UDP sockets
- TCP sockets (connect, listen, accept, send, recv)
- Network syscall interface for user programs

## Architecture

```
User Program (ecall / int 0x80)
    ↓ sys_socket / sys_bind / sys_connect / sys_sendto / sys_recvfrom
Syscall Layer (syscall/mod.rs, syscall/linux.rs)
    ↓ NetStack::create_socket / send / recv / bind / connect / close / poll
Network Interface (net/iface.rs)
    ↓ smoltcp Interface + SocketSet + SocketHandle
smoltcp Protocol Stack
    ↓ Device trait (receive / transmit)
Device Adapter (net/device.rs)
    ↓ VirtRxToken / VirtTxToken → NET_STATE buffer
VirtIO Net Driver (driver/net.rs)
    ↓ MMIO registers at 0x10007000 (QEMU virt slot 6)
QEMU user-mode networking (10.0.2.15/24, gw 10.0.2.2)
```

## File Layout

| File | Description |
|------|-------------|
| `kernel/src/net/mod.rs` | Module organization |
| `kernel/src/net/device.rs` | smoltcp `Device` trait implementation wrapping VirtIO Net |
| `kernel/src/net/iface.rs` | `NetStack` — global network state (Interface, SocketSet, socket metadata) |
| `kernel/src/driver/net.rs` | VirtIO Net MMIO driver (probe, init, send_raw, recv_raw) |

## Network Configuration

- **IP address**: 10.0.2.15/24 (QEMU user-mode default)
- **Gateway**: 10.0.2.2
- **MAC**: auto-detected from VirtIO device (typically 52:54:00:12:34:56)
- **Port forwarding**: QEMU forwards host :2323 → guest :23 (TCP/UDP)

## QEMU Configuration

```makefile
-netdev user,id=net0,hostfwd=tcp::2323-:23,hostfwd=udp::2323-:23 \
-device virtio-net-device,netdev=net0
```

## VirtIO Net Device (QEMU virt)

- **Slot**: 6 (base 0x10007000, stride 0x1000)
- **Device ID**: 1 (VIRTIO_ID_NET)
- **MMIO version**: 1 (NOT version 2 — do not filter by version!)
- **Queue 0**: RX (1024 max descriptors, 8 configured)
- **Queue 1**: TX (1024 max descriptors, 8 configured)

## Initialization Timing

**CRITICAL**: Network initialization must happen AFTER user program loading.
The `init_net_device()` + `NetStack::init()` setup allocates ~25KB DMA buffers
that can interfere with VMM page table frame allocation during user process creation.
In `kmain()`, network init is placed after `process::add_process()`.

## smoltcp Integration

### Device Trait Adapter

`net/device.rs` implements smoltcp's `phy::Device` trait:
- `VirtRxToken` — reads from static NET_STATE buffer, marks as consumed
- `VirtTxToken` — writes to stack buffer, then calls `send_raw()` via VirtIO Net
- `NetDevice::receive_packet()` — polls VirtIO for incoming packets, buffers them
- All state protected by `spin::Mutex<NetState>`

### Interface and SocketSet

`net/iface.rs` manages:
- `NetStack::init(mac)` — creates Interface with IP config, SocketSet with 16 slots
- `NetStack::poll()` — called from timer interrupt (~10ms interval)
- Socket lifecycle: create → bind → (listen/accept for TCP) → send/recv → close

### Socket Types

| Type | smoltcp Socket | Buffer Sizes |
|------|---------------|-------------|
| TCP  | `tcp::Socket` | RX: 4KB, TX: 4KB |
| UDP  | `udp::Socket` | RX: 4KB (4 packets), TX: 4KB (4 packets) |
| ICMP | `icmp::Socket` | RX: 4KB (4 packets), TX: 4KB (4 packets) |

## Network Syscalls

| Number | Name | Args | Description |
|--------|------|------|-------------|
| 70 | sys_socket | (domain, type, protocol) | domain=2(AF_INET), type=1(TCP)/2(UDP)/3(ICMP) |
| 71 | sys_bind | (fd, addr_ptr, addr_len) | Bind to sockaddr_in (port extraction) |
| 72 | sys_connect | (fd, addr_ptr, addr_len) | Connect TCP to remote |
| 73 | sys_listen | (fd, backlog) | Listen on bound TCP socket |
| 74 | sys_accept | (fd) | Accept incoming TCP connection |
| 75 | sys_sendto | (fd, buf, len, flags, addr_ptr, addr_len) | Send data (UDP needs dest addr) |
| 76 | sys_recvfrom | (fd, buf, len) | Receive data |
| 77 | sys_shutdown | (fd) | Close/shutdown socket |

### sockaddr_in Layout (16 bytes)

```
Offset  Size  Field
0       2     family (2 = AF_INET, little-endian)
2       2     port (big-endian / network byte order)
4       4     IPv4 address (big-endian / network byte order)
8       8     zero padding
```

### Linux Compatibility

Network syscalls are also mapped in the Linux compatibility layer (`syscall/linux.rs`):
- L_SOCKET (198) → SYS_SOCKET (70)
- L_BIND (200) → SYS_BIND (71)
- L_LISTEN (201) → SYS_LISTEN (73)
- L_ACCEPT (202) → SYS_ACCEPT (74)
- L_CONNECT (203) → SYS_CONNECT (72)
- L_SENDTO (206) → SYS_SENDTO (75)
- L_RECVFROM (207) → SYS_RECVFROM (76)
- L_SHUTDOWN (210) → SYS_SHUTDOWN (77)

## Timer Integration

Network polling is integrated into the timer interrupt handler:
- `tick_uptime()` — increments a monotonic millisecond counter (AtomicU64)
- `NetStack::poll()` — called from timer ISR (only if initialized), non-blocking
- `uptime_ms()` — provides `smoltcp::time::Instant` for the protocol stack

## Socket Fd Management

Network sockets use the `NetStack::socket_metas` array (16 slots):
- fd = index into the metadata array (0..15)
- Each entry contains: `SocketHandle`, `SocketType`, `SocketState`
- Socket handles map to smoltcp's internal `SocketSet`
- Close removes the socket from both the metadata array and `SocketSet`

## Threading/Safety

- `NET_STACK` is a `spin::Mutex<Option<NetStack>>` — all operations acquire the lock
- Network polling from timer ISR is safe because `spin::Mutex` disables interrupts
- Network syscalls from user programs hold the mutex for the entire operation

## Limitations

1. **No DHCP**: Static IP configuration only (10.0.2.15)
2. **No DNS**: Socket DNS feature included but no resolver configured
3. **Single-buffer RX**: Only one packet buffered at a time; packets arriving during processing are dropped
4. **No async accept**: TCP accept is simplified (returns same fd when connected)
5. **16 socket limit**: Maximum 16 simultaneous sockets across all types
6. **No IPv6**: Only IPv4 is configured
7. **TCP connect needs Context**: `tcp::Socket::connect()` requires `Interface::context()` for source address selection
